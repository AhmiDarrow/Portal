use super::*;

fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "portal-packs-safety-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn a_damaged_packs_file_is_set_aside_not_replaced() {
    for (name, text) in [("bad-json", "{"), ("not-a-map", "[1, 2]")] {
        let dir = dir(name);
        std::fs::write(dir.join("packs.json"), text).unwrap();
        let packs = SeasonPacks::open(&dir);
        assert_eq!(packs.preferred("tv:24", 3), None);
        assert!(!dir.join("packs.json").exists());
        let aside: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("packs.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(std::fs::read_to_string(aside[0].path()).unwrap(), text);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(windows)]
#[test]
fn a_packs_file_that_cannot_be_read_is_not_saved_over() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = dir("held");
    let path = dir.join("packs.json");
    std::fs::write(&path, "{\"tv:24\\t3\":\"abcdefgh\"}").unwrap();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let packs = SeasonPacks::open(&dir);
    drop(lock);
    assert!(save(&packs.path, &packs.order, &packs.saved).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"tv:24\\t3\":\"abcdefgh\"}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
