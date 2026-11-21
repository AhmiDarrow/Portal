use super::*;

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "portal-subtitles-safety-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("subtitles.json")
}

#[cfg(windows)]
#[test]
fn saved_keys_survive_a_file_that_could_not_be_read() {
    use std::os::windows::fs::OpenOptionsExt;
    let path = temp("held");
    let stored = Stored {
        subdl_key: Some("subdl-key-0123456789".into()),
        ..Stored::default()
    };
    save(&path, &stored).unwrap();
    let before = std::fs::read_to_string(&path).unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    assert_eq!(load(&path), Stored::default());
    drop(lock);
    // The settings page saves what it has, defaults and all: refused.
    assert!(save(&path, &Stored::default()).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_damaged_file_is_set_aside() {
    let path = temp("damaged");
    std::fs::write(&path, "{\"languages\":").unwrap();
    assert_eq!(load(&path), Stored::default());
    assert!(!path.exists());
    let dir = path.parent().unwrap();
    let aside = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("subtitles.corrupt-"))
        .count();
    assert_eq!(aside, 1);
    save(&path, &Stored::default()).unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
