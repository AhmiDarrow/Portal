use super::*;

#[test]
fn a_search_stays_small_enough_for_a_worker_stack() {
    // Seven sources inline made this 225 KB, and moving it through the
    // handler overflowed a 2 MB worker stack. Boxed, it stays small.
    let dir = std::env::temp_dir().join(format!("portal-subs-size-{}", std::process::id()));
    let subtitles = Subtitles::open(&dir);
    let metadata = Metadata::open(&dir);
    let search = subtitles.search(&metadata, SubtitleSearch::default());
    assert!(
        std::mem::size_of_val(&search) < 16 * 1024,
        "{}",
        std::mem::size_of_val(&search)
    );
}

fn temp(name: &str) -> (Subtitles, PathBuf) {
    let dir = std::env::temp_dir().join(format!("portal-subtitles-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (Subtitles::open(&dir), dir)
}

fn peer(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse().unwrap(), 50000)
}

fn put(json: &str) -> SubtitleSettingsPut {
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn settings_change_only_from_loopback() {
    let (subtitles, dir) = temp("guard");
    assert_eq!(subtitles.public(), SubtitleSettings::default());
    for ip in ["192.168.1.20", "10.0.0.5", "::ffff:192.168.1.20", "0.0.0.0"] {
        let err = subtitles
            .apply(peer(ip), put(r#"{"hearing_impaired": true}"#))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }
    assert!(!dir.join("subtitles.json").exists());
    let saved = subtitles
        .apply(
            peer("::ffff:127.0.0.1"),
            put(r#"{"hearing_impaired": true, "languages": ["fr-FR", "en"]}"#),
        )
        .await
        .unwrap();
    assert!(saved.hearing_impaired);
    assert_eq!(saved.languages, vec!["fr", "en"]);
    assert_eq!(
        settings::load(&dir.join("subtitles.json")).languages,
        vec!["fr", "en"]
    );
    let err = subtitles
        .apply(peer("127.0.0.1"), put(r#"{"languages": ["klingon"]}"#))
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    let cleared = subtitles
        .apply(peer("::1"), put(r#"{"languages": []}"#))
        .await
        .unwrap();
    assert!(cleared.languages.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn keys_are_checked_before_saving_and_never_returned() {
    let (subtitles, dir) = temp("key");
    let err = subtitles
        .apply(
            peer("127.0.0.1"),
            put(r#"{"opensubtitles_key": "not a key"}"#),
        )
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    assert_eq!(err.1 .0["detail"], opensubtitles::KEY_REFUSED);

    // Signing in needs a key first.
    let err = subtitles
        .apply(
            peer("127.0.0.1"),
            put(r#"{"opensubtitles_username": "u", "opensubtitles_password": "p"}"#),
        )
        .await
        .unwrap_err();
    assert_eq!(err.1 .0["detail"], NEED_KEY_FIRST);

    settings::save(
        &dir.join("subtitles.json"),
        &settings::Stored {
            opensubtitles_key: Some("ABCDEFGHIJKLMNOPQRSTUVWXYZ012345".into()),
            opensubtitles_username: Some("someone".into()),
            opensubtitles_password: Some("hunter2".into()),
            ..settings::Stored::default()
        },
    )
    .unwrap();
    let subtitles = Subtitles::open(&dir);
    let public = subtitles.public();
    assert!(public.opensubtitles && public.opensubtitles_user);
    let json = serde_json::to_string(&public).unwrap();
    assert!(!json.contains("ABCDEFGH") && !json.contains("hunter2") && !json.contains("someone"));

    let cleared = subtitles
            .apply(
                peer("127.0.0.1"),
                put(r#"{"opensubtitles_key": null, "opensubtitles_username": null, "opensubtitles_password": ""}"#),
            )
            .await
            .unwrap();
    assert!(!cleared.opensubtitles && !cleared.opensubtitles_user);
    let stored = settings::load(&dir.join("subtitles.json"));
    assert_eq!(stored.opensubtitles_key, None);
    assert_eq!(stored.opensubtitles_password, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn subdl_and_subsource_keys_work_like_the_opensubtitles_key() {
    let (subtitles, dir) = temp("other-keys");
    for (json, detail) in [
        (r#"{"subdl_key": "not a key"}"#, subdl::KEY_REFUSED),
        (r#"{"subsource_key": "short"}"#, subsource::KEY_REFUSED),
    ] {
        let err = subtitles
            .apply(peer("127.0.0.1"), put(json))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert_eq!(err.1 .0["detail"], detail);
    }
    let err = subtitles
        .apply(peer("192.168.1.20"), put(r#"{"subdl_key": null}"#))
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::FORBIDDEN);

    settings::save(
        &dir.join("subtitles.json"),
        &settings::Stored {
            subdl_key: Some("SUBDLSECRET123".into()),
            subsource_key: Some("SUBSOURCESECRET123".into()),
            ..settings::Stored::default()
        },
    )
    .unwrap();
    let subtitles = Subtitles::open(&dir);
    let public = subtitles.public();
    assert!(public.subdl && public.subsource && !public.opensubtitles);
    let json = serde_json::to_string(&public).unwrap();
    assert!(!json.contains("SECRET"));

    // A field left out keeps its key; null or blank removes it.
    let kept = subtitles
        .apply(peer("127.0.0.1"), put(r#"{"hearing_impaired": true}"#))
        .await
        .unwrap();
    assert!(kept.subdl && kept.subsource);
    let cleared = subtitles
        .apply(
            peer("127.0.0.1"),
            put(r#"{"subdl_key": null, "subsource_key": "  "}"#),
        )
        .await
        .unwrap();
    assert!(!cleared.subdl && !cleared.subsource);
    let stored = settings::load(&dir.join("subtitles.json"));
    assert_eq!(stored.subdl_key, None);
    assert_eq!(stored.subsource_key, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn release_names_tell_the_episode() {
    let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
    assert_eq!(
        release_episode(&names(&["Show.S01E03.WEB", "Show.s01e02.WEB"]), (1, 2)),
        EpisodeNamed::This
    );
    assert_eq!(
        release_episode(&names(&["show 1x02 hdtv"]), (1, 2)),
        EpisodeNamed::This
    );
    assert_eq!(
        release_episode(&names(&["Show.S01E03.WEB"]), (1, 2)),
        EpisodeNamed::Other
    );
    assert_eq!(
        release_episode(&names(&["Show.S01.Complete", "Show 1080p"]), (1, 2)),
        EpisodeNamed::Nothing
    );
    assert_eq!(release_episode(&[], (1, 2)), EpisodeNamed::Nothing);
}

#[test]
fn ids_name_a_source_and_a_safe_rest() {
    assert_eq!(
        parse_id("gestdown:bb4199f2-e518-4e66-b3c6-0c49cf45650f").map(|(source, _)| source),
        Some(Source::Gestdown)
    );
    assert!(parse_id("unknown:the-movie-1994-1").is_none());
    assert_eq!(
        parse_id("opensubtitles:12658811").map(|(s, _)| s),
        Some(Source::OpenSubtitles)
    );
    assert_eq!(
        parse_id("subdl:1234-5678").map(|(s, _)| s),
        Some(Source::Subdl)
    );
    assert_eq!(
        parse_id("subsource:9001"),
        Some((Source::Subsource, "9001"))
    );
    assert_eq!(
        parse_id("tvsubtitles:1448"),
        Some((Source::TvSubtitles, "1448"))
    );
    assert_eq!(
        parse_id("subtitlecat:subs/124/Show S01E01-en.srt"),
        Some((Source::SubtitleCat, "subs/124/Show S01E01-en.srt"))
    );
    for bad in [
        "gestdown:../x",
        "yify:A/B",
        "opensubtitles:1a",
        "subdl:../x",
        "subdl:a/b.zip",
        "subsource:9a",
        "tvsubtitles:1a",
        "tvsubtitles:../1",
        "subtitlecat:subs/1/../x.srt",
        "subtitlecat:/subs/1/x.srt",
        "subtitlecat:subs/1/x.zip",
        "subtitlecat:https://evil/x.srt",
        "other:1",
        "nocolon",
        "",
    ] {
        assert_eq!(parse_id(bad), None, "{bad}");
    }
}

#[test]
fn offsets_are_seconds() {
    assert_eq!(parse_offset(None).unwrap(), 0.0);
    assert_eq!(parse_offset(Some("120")).unwrap(), 120.0);
    assert_eq!(parse_offset(Some("-1.5")).unwrap(), -1.5);
    assert!(parse_offset(Some("NaN")).is_err());
    assert!(parse_offset(Some("1e12")).is_err());
    assert!(parse_offset(Some("abc")).is_err());
}

#[test]
fn queries_tell_episodes_from_movies() {
    let episode = Query::from_search(SubtitleSearch {
        kind: Some(MediaKind::Episode),
        title: Some("Pilot".into()),
        show_title: Some("  Some Show ".into()),
        season: Some(1),
        episode: Some(1),
        imdb_id: Some("tt0000001".into()),
        tvdb_id: Some("78901".into()),
        tmdb_id: Some("x1622".into()),
        ..SubtitleSearch::default()
    });
    assert_eq!(episode.episode, Some((1, 1)));
    assert!(!episode.movie);
    assert_eq!(episode.show.as_deref(), Some("Some Show"));
    assert_eq!(episode.tmdb, None);
    assert_eq!(
        episode.seed().unwrap().title.as_deref(),
        Some("Some Show")
    );
    assert_eq!(episode.seed().unwrap().imdb_id, None);

    let movie = Query::from_search(SubtitleSearch {
        title: Some("Storm".into()),
        year: Some(1995),
        ..SubtitleSearch::default()
    });
    assert!(movie.movie && movie.episode.is_none());
    assert_eq!(movie.seed().unwrap().kind, Some(MediaKind::Movie));

    let show = Query::from_search(SubtitleSearch {
        kind: Some(MediaKind::Tv),
        title: Some("Some Show".into()),
        ..SubtitleSearch::default()
    });
    assert!(!show.movie && show.episode.is_none() && show.seed().is_none());
}

#[tokio::test]
async fn cached_files_are_shifted_without_a_download() {
    let (subtitles, dir) = temp("file");
    let id = "gestdown:bb4199f2-e518-4e66-b3c6-0c49cf45650f";
    subtitles.files.put(
        id,
        Arc::new("WEBVTT\n\n00:02:05.000 --> 00:02:07.500\nHello\n\n".into()),
    );
    let later = subtitles.file(id, 126.0).await.unwrap();
    assert_eq!(later, "WEBVTT\n\n00:00:00.000 --> 00:00:01.500\nHello\n\n");
    let past = subtitles.file(id, 600.0).await.unwrap();
    assert_eq!(past, "WEBVTT\n\n");
    let shifted = subtitles.file(id, 120.0).await.unwrap();
    assert!(
        shifted.contains("00:00:05.000 --> 00:00:07.500"),
        "{shifted}"
    );
    let whole = subtitles.file(id, 0.0).await.unwrap();
    assert!(whole.contains("00:02:05.000"));
    let err = subtitles.file("gestdown:nope", 0.0).await.unwrap_err();
    assert_eq!(err.0, StatusCode::NOT_FOUND);
    let err = subtitles.file("opensubtitles:123", 0.0).await.unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert_eq!(err.1 .0["detail"], NO_KEY);
    let err = subtitles.file("subsource:123", 0.0).await.unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert_eq!(err.1 .0["detail"], NO_SUBSOURCE_KEY);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn season_pack_files_are_kept_per_episode() {
    let (subtitles, dir) = temp("pack");
    let id = "subsource:77";
    assert_eq!(file_key(id, None), "subsource:77");
    assert_eq!(file_key(id, Some((1, 2))), "subsource:77@S01E02");
    let listed = |episode| Found {
        id: id.into(),
        source: Source::Subsource,
        language: "en".into(),
        release: "Show.S01.WEB".into(),
        hearing_impaired: false,
        downloads: None,
        rating: None,
        hash_match: false,
        episode: Some(episode),
        episode_unsure: true,
        machine_translated: false,
    };
    subtitles.files.put(
        &file_key(id, Some((1, 2))),
        Arc::new("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nTwo\n\n".into()),
    );
    subtitles.remember(&[listed((1, 2))]);
    assert!(subtitles.file(id, 0.0).await.unwrap().contains("Two"));
    // Episode 3 of the same pack does not get episode 2's text.
    subtitles.remember(&[listed((1, 3))]);
    assert_eq!(subtitles.files.get(&file_key(id, Some((1, 3)))), None);
    let err = subtitles.file(id, 0.0).await.unwrap_err();
    assert_eq!(err.1 .0["detail"], NO_SUBSOURCE_KEY);
    // An id that names its episode keeps it, whatever was searched last.
    assert_eq!(
        split_episode("subsource:77@S01E02"),
        ("subsource:77", Some((1, 2)))
    );
    assert_eq!(split_episode("subsource:77@x"), ("subsource:77@x", None));
    assert!(valid_file_id("subsource:77@S01E02"));
    assert!(subtitles
        .file("subsource:77@S01E02", 0.0)
        .await
        .unwrap()
        .contains("Two"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_failed_language_keeps_the_others() {
    let busy = || SourceError::Note("busy".into());
    assert_eq!(
        merge_languages(vec![Ok(vec![1, 2]), Err(SourceError::Failed), Ok(vec![3])]),
        Ok(vec![1, 2, 3])
    );
    assert_eq!(
        merge_languages(vec![Err(busy()), Ok(Vec::<u8>::new())]),
        Ok(vec![])
    );
    // Only when every language failed is the source reported, with its reason.
    assert_eq!(
        merge_languages::<u8>(vec![Err(SourceError::Failed), Err(busy())]),
        Err(busy())
    );
    assert_eq!(
        merge_languages::<u8>(vec![Err(SourceError::Failed)]),
        Err(SourceError::Failed)
    );
    assert_eq!(merge_languages::<u8>(Vec::new()), Ok(vec![]));
}
