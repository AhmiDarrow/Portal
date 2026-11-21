use super::*;

const PAIRED: &str = r#"[{"id":"tv","name":"Living room TV","key_hash":"abc","paired_at":1}]"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "portal-pair-safety-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Holds the file the way antivirus or a backup can: no one else may open it.
#[cfg(windows)]
fn hold(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[test]
fn a_devices_file_that_cannot_be_read_is_never_saved_over() {
    let dir = scratch("held");
    let path = dir.join("devices.json");
    std::fs::write(&path, PAIRED).unwrap();
    let lock = hold(&path);
    let pairing = Pairing::open(&dir);
    drop(lock);
    assert!(pairing.lock().devices.is_empty());
    // A save later in the run (a device seen, one paired) must not wipe it.
    pairing.save(&[]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), PAIRED);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_devices_file_that_reads_pairs_its_devices() {
    let dir = scratch("reads");
    std::fs::write(dir.join("devices.json"), PAIRED).unwrap();
    assert_eq!(Pairing::open(&dir).lock().devices.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_devices_file_is_set_aside_and_a_save_starts_a_new_one() {
    let dir = scratch("damaged");
    let path = dir.join("devices.json");
    std::fs::write(&path, "[{").unwrap();
    let pairing = Pairing::open(&dir);
    assert!(pairing.lock().devices.is_empty());
    let aside: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("devices.corrupt-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(std::fs::read_to_string(aside[0].path()).unwrap(), "[{");
    pairing.save(&[]);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]");
    let _ = std::fs::remove_dir_all(&dir);
}
