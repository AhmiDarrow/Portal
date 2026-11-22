use super::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "portal-subtitles-{name}-{}.json",
        std::process::id()
    ))
}

#[test]
fn settings_round_trip_with_secrets_encrypted() {
    let path = temp("round-trip");
    let _ = std::fs::remove_file(&path);
    assert_eq!(load(&path), Stored::default());

    let stored = Stored {
        languages: vec!["en".into(), "pt-BR".into()],
        hearing_impaired: true,
        opensubtitles_key: Some("KEY-abcdef0123456789".into()),
        opensubtitles_username: Some("someone".into()),
        opensubtitles_password: Some("pass word!".into()),
        subdl_key: Some("subdl-key-0123456789".into()),
        subsource_key: Some("subsource-key-0123456789".into()),
    };
    save(&path, &stored).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("KEY-abcdef0123456789"));
    assert!(!text.contains("pass word!"));
    assert!(!text.contains("subdl-key-0123456789"));
    assert!(!text.contains("subsource-key-0123456789"));
    assert!(text.contains(crate::subtitles::PROTECTED));
    assert_eq!(load(&path), stored);

    save(&path, &Stored::default()).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("opensubtitles"));
    assert!(!text.contains("subdl") && !text.contains("subsource"));

    std::fs::write(&path, "not json").unwrap();
    assert_eq!(load(&path), Stored::default());
    // Hand-typed values are read as they are; broken sealed ones are dropped.
    std::fs::write(
            &path,
            r#"{"languages":["EN-us","xx"],"opensubtitles_key":" typed ","opensubtitles_password":"dpapi:00ff","subdl_key":"dpapi:zz"}"#
                .replace("dpapi:", PROTECTED),
        )
        .unwrap();
    let loaded = load(&path);
    assert_eq!(loaded.languages, vec!["en"]);
    assert_eq!(loaded.opensubtitles_key.as_deref(), Some("typed"));
    assert_eq!(loaded.opensubtitles_password, None);
    assert_eq!(loaded.subdl_key, None);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn debug_output_hides_secrets() {
    let stored = Stored {
        languages: vec!["en".into()],
        hearing_impaired: false,
        opensubtitles_key: Some("secret-key".into()),
        opensubtitles_username: Some("user-name".into()),
        opensubtitles_password: Some("secret-password".into()),
        subdl_key: Some("secret-subdl".into()),
        subsource_key: None,
    };
    let text = format!("{stored:?}");
    assert!(!text.contains("secret-key"));
    assert!(!text.contains("user-name"));
    assert!(!text.contains("secret-password"));
    assert!(text.contains("opensubtitles: true"));
    assert!(text.contains("opensubtitles_user: true"));
    assert!(!text.contains("secret-subdl"));
    assert!(text.contains("subdl: true") && text.contains("subsource: false"));
}
