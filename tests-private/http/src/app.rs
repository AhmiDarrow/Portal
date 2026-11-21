use super::*;

#[test]
fn the_graphics_card_choice_is_kept_and_is_on_until_turned_off() {
    let dir = std::env::temp_dir().join(format!("portal-encoder-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // Nothing saved yet.
    assert!(hardware_saved(&dir));
    save_hardware(&dir, false).unwrap();
    assert!(!hardware_saved(&dir));
    save_hardware(&dir, true).unwrap();
    assert!(hardware_saved(&dir));
    // A file that cannot be read means the default.
    std::fs::write(dir.join(ENCODER_FILE), "not json").unwrap();
    assert!(hardware_saved(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}
