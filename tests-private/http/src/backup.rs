use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "portal-backup-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copies(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(backups_dir(dir))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

#[test]
fn a_new_version_copies_the_library_once() {
    let dir = scratch("once");
    fs::write(dir.join("library.json"), r#"{"watchlist":{"a":1}}"#).unwrap();
    fs::write(dir.join("prefs.json"), "{}").unwrap();
    fs::write(dir.join(MARKER), "0.2.1").unwrap();

    on_version_change(&dir, "0.2.2");
    let made = copies(&dir);
    assert_eq!(made.len(), 1);
    assert!(made[0].to_string_lossy().ends_with("-0.2.1"));
    assert_eq!(
        fs::read_to_string(made[0].join("library.json")).unwrap(),
        r#"{"watchlist":{"a":1}}"#
    );
    assert!(made[0].join("prefs.json").is_file());
    assert!(!made[0].join("cast.json").exists());
    // The originals stay where they are.
    assert!(dir.join("library.json").is_file());
    assert_eq!(fs::read_to_string(dir.join(MARKER)).unwrap(), "0.2.2");

    on_version_change(&dir, "0.2.2");
    assert_eq!(copies(&dir).len(), 1);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_first_start_without_data_copies_nothing() {
    let dir = scratch("fresh");
    on_version_change(&dir, "0.2.2");
    assert!(copies(&dir).is_empty());
    assert_eq!(fs::read_to_string(dir.join(MARKER)).unwrap(), "0.2.2");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn only_the_newest_copies_are_kept() {
    let dir = scratch("keep");
    fs::write(dir.join("library.json"), "{}").unwrap();
    for version in ["1", "2", "3", "4", "5"] {
        on_version_change(&dir, version);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let made = copies(&dir);
    assert_eq!(made.len(), KEEP);
    assert!(made[KEEP - 1].to_string_lossy().ends_with("-4"));
    let _ = fs::remove_dir_all(dir);
}
