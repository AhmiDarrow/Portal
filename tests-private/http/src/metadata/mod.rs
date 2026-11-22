use super::*;

fn temp_metadata(name: &str) -> (Metadata, PathBuf) {
    let dir = std::env::temp_dir().join(format!("portal-metadata-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    (Metadata::open(&dir), dir)
}

fn peer(ip: &str) -> SocketAddr {
    SocketAddr::new(ip.parse().unwrap(), 50000)
}

fn put(json: &str) -> MetadataPut {
    serde_json::from_str(json).unwrap()
}

#[tokio::test]
async fn metadata_settings_change_only_from_loopback() {
    let (metadata, dir) = temp_metadata("guard");
    assert_eq!(
        metadata.public(),
        MetadataSettings {
            online: true,
            tmdb: false
        }
    );

    for ip in ["192.168.1.20", "10.0.0.5", "::ffff:192.168.1.20", "0.0.0.0"] {
        let err = metadata
            .apply(peer(ip), put(r#"{"online": false}"#))
            .await
            .unwrap_err();
        assert_eq!(err.0, StatusCode::FORBIDDEN);
    }
    assert!(metadata.public().online);
    assert!(!dir.join("metadata.json").exists());

    let saved = metadata
        .apply(peer("127.0.0.1"), put(r#"{"online": false}"#))
        .await
        .unwrap();
    assert_eq!(
        saved,
        MetadataSettings {
            online: false,
            tmdb: false
        }
    );
    let saved = metadata
        .apply(peer("::1"), put(r#"{"online": true}"#))
        .await
        .unwrap();
    assert!(saved.online);
    let saved = metadata
        .apply(peer("::ffff:127.0.0.1"), put(r#"{}"#))
        .await
        .unwrap();
    assert!(saved.online);
    assert!(settings::load(&dir.join("metadata.json")).online);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_malformed_key_is_refused_before_any_request_and_null_clears() {
    let (metadata, dir) = temp_metadata("key");
    let err = metadata
        .apply(peer("127.0.0.1"), put(r#"{"tmdb_key": "not a key"}"#))
        .await
        .unwrap_err();
    assert_eq!(err.0, StatusCode::BAD_REQUEST);
    assert_eq!(err.1 .0.detail, KEY_REFUSED);

    // A stored key reads back as `tmdb: true` and never as the key.
    settings::save(
        &dir.join("metadata.json"),
        &settings::Stored {
            online: true,
            tmdb_key: Some("0123456789abcdef0123456789abcdef".into()),
        },
    )
    .unwrap();
    let metadata = Metadata::open(&dir);
    let public = metadata.public();
    assert!(public.tmdb);
    let json = serde_json::to_string(&public).unwrap();
    assert!(!json.contains("0123456789abcdef"));

    let cleared = metadata
        .apply(peer("127.0.0.1"), put(r#"{"tmdb_key": null}"#))
        .await
        .unwrap();
    assert!(!cleared.tmdb);
    assert_eq!(settings::load(&dir.join("metadata.json")).tmdb_key, None);
    let blank = metadata
        .apply(peer("127.0.0.1"), put(r#"{"tmdb_key": "  "}"#))
        .await
        .unwrap();
    assert!(!blank.tmdb);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn offline_lookups_return_the_cleaned_seed() {
    let (metadata, dir) = temp_metadata("offline");
    metadata
        .apply(peer("127.0.0.1"), put(r#"{"online": false}"#))
        .await
        .unwrap();
    let seed: MediaInfo = serde_json::from_value(serde_json::json!({
        "kind": "movie",
        "title": "The Shawshank Redemption",
        "year": 1994,
        "imdb_id": "tt0111161",
        "genres": null
    }))
    .unwrap();
    let info = metadata.lookup(seed).await;
    assert_eq!(info.title.as_deref(), Some("The Shawshank Redemption"));
    assert_eq!(
        info.links,
        vec![portal_engine::kodi_info::imdb_link("tt0111161")]
    );
    assert!(info.sources.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn cached_lookups_merge_without_asking_again() {
    let (metadata, dir) = temp_metadata("cached");
    let seed = MediaInfo {
        kind: Some(MediaKind::Tv),
        title: Some("Example Show".into()),
        imdb_id: Some("tt0903747".into()),
        ..MediaInfo::default()
    };
    let found = MediaInfo {
        plot: Some("From the cache.".into()),
        sources: vec![SOURCE_TVMAZE.into()],
        ..MediaInfo::default()
    };
    metadata.cache.put(
        &cache::key(&seed).unwrap(),
        vec![found],
        cache::FOUND_TTL,
        cache::now(),
    );
    let info = metadata.lookup(seed).await;
    assert_eq!(info.plot.as_deref(), Some("From the cache."));
    assert_eq!(info.sources, vec![SOURCE_TVMAZE]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn queries_pick_their_sources() {
    let movie = Query::from_seed(&MediaInfo {
        kind: Some(MediaKind::Movie),
        title: Some("Heat".into()),
        year: Some(1995),
        ..MediaInfo::default()
    });
    assert!(movie.wants_tmdb() && movie.wants_wikipedia() && !movie.wants_tvmaze());

    let episode = Query::from_seed(&MediaInfo {
        kind: Some(MediaKind::Episode),
        title: Some("Pilot".into()),
        show_title: Some("Breaking Bad".into()),
        year: Some(2008),
        season: Some(1),
        episode: Some(1),
        ..MediaInfo::default()
    });
    assert_eq!(episode.title.as_deref(), Some("Breaking Bad"));
    assert_eq!(episode.year, None);
    assert_eq!(episode.episode_numbers(), Some((1, 1)));
    assert!(episode.wants_tvmaze() && !episode.wants_wikipedia());

    let unknown = Query::from_seed(&MediaInfo {
        title: Some("Something".into()),
        ..MediaInfo::default()
    });
    assert!(!unknown.wants_tmdb() && !unknown.wants_tvmaze() && unknown.wants_wikipedia());
}
