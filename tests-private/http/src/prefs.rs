use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("portal-prefs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn put(json: &str) -> PrefsPut {
    serde_json::from_str(json).unwrap()
}

#[test]
fn defaults_until_something_is_saved() {
    let dir = scratch("defaults");
    let store = PrefsStore::open(&dir);
    let body = serde_json::to_value(store.body()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "quality": "auto",
            "subtitles": "off",
            "subtitle_language": null,
            "audio_language": null,
            "autoplay": true,
            "stored": false
        })
    );
    assert!(!dir.join("prefs.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_partial_put_changes_only_its_fields_and_persists() {
    let dir = scratch("partial");
    let mut store = PrefsStore::open(&dir);
    let body = store
        .apply(put(
            r#"{"subtitles":"auto","subtitle_language":"pt_BR","extra":1}"#,
        ))
        .unwrap();
    assert!(body.stored);
    assert_eq!(body.prefs.subtitles, SubtitleMode::Auto);
    assert_eq!(body.prefs.subtitle_language.as_deref(), Some("pt-BR"));
    assert_eq!(body.prefs.quality, Quality::Auto);
    assert!(body.prefs.autoplay);

    let body = store
        .apply(put(r#"{"quality":"720","autoplay":false}"#))
        .unwrap();
    assert_eq!(body.prefs.quality, Quality::P720);
    assert!(!body.prefs.autoplay);
    assert_eq!(body.prefs.subtitle_language.as_deref(), Some("pt-BR"));
    let body = store.apply(put(r#"{"quality":"2160"}"#)).unwrap();
    assert_eq!(body.prefs.quality, Quality::P2160);
    let body = store.apply(put(r#"{"quality":"720"}"#)).unwrap();

    // Reopened, as after a restart.
    let again = PrefsStore::open(&dir);
    assert!(again.body().stored);
    assert_eq!(again.prefs(), &body.prefs);

    // null forgets the language; a missing field keeps it.
    let body = store.apply(put(r#"{"autoplay":true}"#)).unwrap();
    assert_eq!(body.prefs.subtitle_language.as_deref(), Some("pt-BR"));
    let body = store.apply(put(r#"{"subtitle_language":null}"#)).unwrap();
    assert_eq!(body.prefs.subtitle_language, None);
    let body = store.apply(put(r#"{"audio_language":"en-US"}"#)).unwrap();
    assert_eq!(body.prefs.audio_language.as_deref(), Some("en-US"));
    let body = store.apply(put(r#"{"quality":"720"}"#)).unwrap();
    assert_eq!(body.prefs.audio_language.as_deref(), Some("en-US"));
    let body = store.apply(put(r#"{"audio_language":null}"#)).unwrap();
    assert_eq!(body.prefs.audio_language, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_bad_value_changes_nothing() {
    let dir = scratch("bad");
    let mut store = PrefsStore::open(&dir);
    store.apply(put(r#"{"quality":"1080"}"#)).unwrap();
    for bad in [
        r#"{"quality":"4k","autoplay":false}"#,
        r#"{"quality":"","autoplay":false}"#,
        r#"{"subtitles":"on","autoplay":false}"#,
        r#"{"subtitle_language":"english please","autoplay":false}"#,
        r#"{"subtitle_language":"e","autoplay":false}"#,
        r#"{"subtitle_language":"en-<script>","autoplay":false}"#,
        r#"{"audio_language":"english please","autoplay":false}"#,
    ] {
        assert!(
            matches!(store.apply(put(bad)), Err(PutError::Invalid(_))),
            "{bad}"
        );
        assert_eq!(store.prefs().quality, Quality::P1080, "{bad}");
        assert!(store.prefs().autoplay, "{bad}");
    }
    assert!(serde_json::from_str::<PrefsPut>(r#"{"autoplay":"yes"}"#).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_file_reads_as_defaults_field_by_field() {
    assert_eq!(parse("not json"), Prefs::default());
    assert_eq!(parse("null"), Prefs::default());
    let prefs =
        parse(r#"{"quality":"8k","subtitles":"auto","subtitle_language":"??","autoplay":false}"#);
    assert_eq!(prefs.quality, Quality::Auto);
    assert_eq!(prefs.subtitles, SubtitleMode::Auto);
    assert_eq!(prefs.subtitle_language, None);
    assert_eq!(prefs.audio_language, None);
    assert!(!prefs.autoplay);
    assert_eq!(
        parse(r#"{"audio_language":"ja"}"#)
            .audio_language
            .as_deref(),
        Some("ja")
    );
    assert_eq!(parse(r#"{"quality":"source"}"#).quality, Quality::Source);
    assert_eq!(parse(r#"{"quality":"2160"}"#).quality, Quality::P2160);
    let dir = scratch("damaged");
    let path = dir.join("prefs.json");
    std::fs::write(&path, "{").unwrap();
    let mut store = PrefsStore::open(&dir);
    assert!(!store.body().stored);
    assert_eq!(store.prefs(), &Prefs::default());
    assert!(matches!(
        store.apply(put(r#"{"quality":"720"}"#)),
        Err(PutError::Save)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn languages_are_cleaned() {
    assert_eq!(clean_language("EN").as_deref(), Some("en"));
    assert_eq!(clean_language(" eng ").as_deref(), Some("eng"));
    assert_eq!(clean_language("zh-TW").as_deref(), Some("zh-TW"));
    assert_eq!(clean_language("pt_BR").as_deref(), Some("pt-BR"));
    assert_eq!(clean_language("english"), None);
    assert_eq!(clean_language("en-"), None);
    assert_eq!(clean_language("en-US-x-very-long-tag"), None);
}
