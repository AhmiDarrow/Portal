use super::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "portal-metadata-{name}-{}.json",
        std::process::id()
    ))
}

#[test]
fn settings_round_trip_with_the_key_encrypted() {
    let path = temp("round-trip");
    let _ = std::fs::remove_file(&path);
    assert_eq!(load(&path), Stored::default());
    assert!(load(&path).online);

    let stored = Stored {
        online: false,
        tmdb_key: Some("0123456789abcdef0123456789abcdef".into()),
    };
    save(&path, &stored).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("0123456789abcdef"));
    assert!(text.contains("dpapi:"));
    assert_eq!(load(&path), stored);

    save(&path, &Stored::default()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("tmdb_key"));
    assert_eq!(load(&path), Stored::default());

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(load(&path), Stored::default());
    std::fs::write(&path, r#"{"tmdb_key": "  "}"#).unwrap();
    assert_eq!(load(&path), Stored::default());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_plain_key_is_read_as_typed_and_a_broken_sealed_one_is_dropped() {
    let path = temp("plain");
    std::fs::write(&path, r#"{"online": false, "tmdb_key": " typed-key "}"#).unwrap();
    let (stored, plain) = read(&path);
    assert!(!stored.online);
    assert_eq!(stored.tmdb_key.as_deref(), Some("typed-key"));
    assert!(plain);

    std::fs::write(&path, r#"{"tmdb_key": "dpapi:zz"}"#).unwrap();
    let (stored, plain) = read(&path);
    assert_eq!(stored, Stored::default());
    assert!(!plain);
    std::fs::write(&path, r#"{"tmdb_key": " "}"#).unwrap();
    assert!(!read(&path).1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_plain_key_from_an_earlier_version_is_encrypted_on_load() {
    let path = temp("migrate");
    std::fs::write(
        &path,
        r#"{"online": true, "tmdb_key": "0123456789abcdef0123456789abcdef"}"#,
    )
    .unwrap();
    let stored = load(&path);
    assert_eq!(
        stored.tmdb_key.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("0123456789abcdef"));
    assert!(text.contains("dpapi:"));
    assert_eq!(load(&path), stored);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn debug_output_hides_the_key() {
    let stored = Stored {
        online: true,
        tmdb_key: Some("secret-value".into()),
    };
    let text = format!("{stored:?}");
    assert!(!text.contains("secret-value"));
    assert!(text.contains("tmdb: true"));
}
