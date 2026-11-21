use super::*;

#[test]
fn quality_names_round_trip() {
    for name in ["auto", "source", "2160", "1080", "720", "480"] {
        assert_eq!(Quality::parse(name).as_str(), name);
        let quoted = format!("\"{name}\"");
        let parsed: Quality = serde_json::from_str(&quoted).unwrap();
        assert_eq!(parsed, Quality::parse(name));
        assert_eq!(serde_json::to_string(&parsed).unwrap(), quoted);
    }
    assert_eq!(Quality::parse("nope"), Quality::Auto);
    assert_eq!(Quality::Auto.cap_height(), Some(1080));
    assert_eq!(Quality::Source.cap_height(), None);
    assert_eq!(Quality::P2160.cap_height(), Some(2160));
}

#[test]
fn status_is_snake_case() {
    let status = Status::stopped(None);
    let json = serde_json::to_value(&status).unwrap();
    assert_eq!(json["state"], "stopped");
    assert_eq!(json["started_by_us"], false);
    assert!(json.get("window_visible").is_some());
}

#[test]
fn imdb_ids_come_out_of_text() {
    assert_eq!(imdb_id("tt0111161").as_deref(), Some("tt0111161"));
    assert_eq!(
        imdb_id("https://www.imdb.com/title/tt0903747/").as_deref(),
        Some("tt0903747")
    );
    assert_eq!(imdb_id("imdb:tt12345678").as_deref(), Some("tt12345678"));
    assert_eq!(imdb_id("0111161"), None);
    assert_eq!(imdb_id("tt12"), None);
    assert_eq!(imdb_id("att0111161"), None);
    assert_eq!(imdb_id("tt0111161x"), None);
    assert_eq!(imdb_id(""), None);
}

#[test]
fn media_info_leaves_out_empty_fields() {
    let info = MediaInfo {
        kind: Some(MediaKind::Episode),
        title: Some("Pilot".into()),
        cast: vec![CastMember {
            name: "A".into(),
            role: None,
            thumb: None,
        }],
        ..MediaInfo::default()
    };
    let json = serde_json::to_value(&info).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "episode", "title": "Pilot", "cast": [{"name": "A"}]})
    );
    assert_eq!(
        serde_json::to_value(MediaInfo::default()).unwrap(),
        serde_json::json!({})
    );
}

#[test]
fn media_info_reads_null_lists_as_empty() {
    let info: MediaInfo = serde_json::from_str(
            r#"{"kind":"tv","title":"X","genres":null,"studios":null,"directors":null,"writers":null,"countries":null,"cast":null,"links":null,"sources":null,"year":null,"poster":"image://poster.jpg/"}"#,
        )
        .unwrap();
    assert_eq!(info.kind, Some(MediaKind::Tv));
    assert!(info.genres.is_empty() && info.cast.is_empty() && info.links.is_empty());
    assert!(info.sources.is_empty() && info.studios.is_empty());
    assert_eq!(info.poster.as_deref(), Some("image://poster.jpg/"));
    assert_eq!(info.year, None);
}

#[test]
fn metadata_put_tells_null_from_missing() {
    let put: MetadataPut = serde_json::from_str(r#"{"online": false}"#).unwrap();
    assert_eq!(put.online, Some(false));
    assert_eq!(put.tmdb_key, None);
    let put: MetadataPut = serde_json::from_str(r#"{"tmdb_key": null}"#).unwrap();
    assert_eq!(put.tmdb_key, Some(None));
    let put: MetadataPut = serde_json::from_str(r#"{"tmdb_key": "abc"}"#).unwrap();
    assert_eq!(put.tmdb_key, Some(Some("abc".into())));
}

#[test]
fn subtitle_types_are_snake_case_and_leave_out_empty_fields() {
    let candidate = SubtitleCandidate {
        id: "gestdown:abc".into(),
        source: "Gestdown".into(),
        language: "en".into(),
        language_name: "English".into(),
        release: String::new(),
        hearing_impaired: false,
        downloads: None,
        rating: None,
        score: 7,
        matched: SubtitleMatch::Title,
    };
    assert_eq!(
        serde_json::to_value(&candidate).unwrap(),
        serde_json::json!({"id": "gestdown:abc", "source": "Gestdown", "language": "en",
                "language_name": "English", "score": 7, "match": "title"})
    );
    let results = SubtitleResults {
        items: vec![candidate],
        best: Some("gestdown:abc".into()),
        notes: Vec::new(),
    };
    let json = serde_json::to_value(&results).unwrap();
    assert_eq!(json["best"], "gestdown:abc");
    assert!(json.get("notes").is_none());
    let search: SubtitleSearch = serde_json::from_str(
        r#"{"kind":"episode","show_title":"Show","season":1,"episode":2,"languages":null}"#,
    )
    .unwrap();
    assert_eq!(search.kind, Some(MediaKind::Episode));
    assert!(search.languages.is_empty());
}

#[test]
fn subtitle_settings_put_hides_secrets() {
    let put: SubtitleSettingsPut = serde_json::from_str(
            r#"{"opensubtitles_key":"KEYVALUE","opensubtitles_password":null,"hearing_impaired":true,"subdl_key":"SUBDLVALUE","subsource_key":null}"#,
        )
        .unwrap();
    assert_eq!(put.subdl_key, Some(Some("SUBDLVALUE".into())));
    assert_eq!(put.subsource_key, Some(None));
    assert_eq!(put.opensubtitles_key, Some(Some("KEYVALUE".into())));
    assert_eq!(put.opensubtitles_password, Some(None));
    assert_eq!(put.opensubtitles_username, None);
    let text = format!("{put:?}");
    assert!(!text.contains("KEYVALUE") && !text.contains("SUBDLVALUE"));
    assert!(text.contains("set") && text.contains("cleared") && text.contains("unchanged"));
}

#[test]
fn list_put_takes_year_and_imdb_id() {
    let put: ListPut =
        serde_json::from_str(r#"{"kind":"movie","name":"X","year":1994,"imdb_id":"tt0111161"}"#)
            .unwrap();
    assert_eq!(put.year, Some(1994));
    assert_eq!(put.imdb_id.as_deref(), Some("tt0111161"));
    let old: ListPut = serde_json::from_str(r#"{"kind":"tv"}"#).unwrap();
    assert_eq!(old.year, None);
}
