use super::*;

fn dir(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "portal-packs-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn pack_row() -> &'static str {
    "Show.A.S03.1080P.WEB.H265-GRP — 1080P | CACHED PACK | HEVC"
}

fn single_row() -> &'static str {
    "Show.A.S03E06.1080p.WEB.x264-GRP — 1080p | x264"
}

#[test]
fn a_played_pack_is_reused_for_that_season_only() {
    let dir = dir("reuse");
    let mut packs = SeasonPacks::open(&dir);
    packs.begin("job-1", "tv:1", 3);
    packs.note("job-1", pack_row());
    packs.note("job-1", "English");
    packs.settle("job-1", true);
    assert_eq!(
        packs.preferred("tv:1", 3).as_deref(),
        pack_identity(pack_row()).as_deref()
    );
    assert_eq!(packs.preferred("tv:1", 4), None);
    let again = SeasonPacks::open(&dir);
    assert_eq!(again.preferred("tv:1", 3), packs.preferred("tv:1", 3));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failed_play_keeps_the_pack_and_a_single_file_clears_it() {
    let dir = dir("clear");
    let mut packs = SeasonPacks::open(&dir);
    packs.begin("job-1", "tv:1", 3);
    packs.note("job-1", pack_row());
    packs.settle("job-1", true);

    packs.begin("job-2", "tv:1", 3);
    packs.note("job-2", pack_row());
    packs.settle("job-2", false);
    assert!(packs.preferred("tv:1", 3).is_some());

    packs.begin("job-3", "tv:1", 3);
    packs.settle("job-3", true);
    assert!(packs.preferred("tv:1", 3).is_some());

    packs.begin("job-4", "tv:1", 3);
    packs.note("job-4", single_row());
    packs.settle("job-4", true);
    assert_eq!(packs.preferred("tv:1", 3), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_long_link_without_an_id_is_hashed_to_a_saved_key() {
    let show = format!("plugin://plugin.video.example/show/{}", "x".repeat(2000));
    let key = show_key(&show);
    assert!(key.starts_with("id:") && key.len() < 64);
    assert_eq!(key, show_key(&show));
    assert_eq!(show_key("tv:1"), "tv:1");
    let dir = dir("hashed-show");
    let mut packs = SeasonPacks::open(&dir);
    packs.begin("job-1", &show, 1);
    packs.note("job-1", pack_row());
    packs.settle("job-1", true);
    assert!(SeasonPacks::open(&dir).preferred(&show, 1).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_corrupt_file_starts_empty() {
    let dir = dir("corrupt");
    std::fs::write(dir.join("packs.json"), "{").unwrap();
    let packs = SeasonPacks::open(&dir);
    assert_eq!(packs.preferred("tv:1", 3), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_newest_play_takes_a_manual_row() {
    let dir = dir("manual");
    let mut packs = SeasonPacks::open(&dir);
    packs.begin("older", "tv:1", 1);
    packs.begin("newer", "tv:1", 3);
    packs.note_latest("English", |_| true);
    packs.note_latest(pack_row(), |id| id == "newer");
    packs.settle("newer", true);
    packs.settle("older", true);
    assert!(packs.preferred("tv:1", 3).is_some());
    assert_eq!(packs.preferred("tv:1", 1), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_row_that_names_the_pack_differently_keeps_it() {
    let dir = dir("loose");
    let mut packs = SeasonPacks::open(&dir);
    packs.begin("job-1", "tv:1", 3);
    packs.note("job-1", pack_row());
    packs.settle("job-1", true);
    let saved = packs.preferred("tv:1", 3).expect("pack");
    // Laid out by a newer add-on so the row reads as one episode of the pack.
    let row = "Show.A.S03E06 from Show.A.S03.1080P.WEB.H265-GRP — 1080p | HEVC";
    assert_eq!(pack_identity(row), None);
    packs.begin("job-2", "tv:1", 3);
    packs.note("job-2", row);
    packs.settle("job-2", true);
    assert_eq!(packs.preferred("tv:1", 3), Some(saved));
    let _ = std::fs::remove_dir_all(&dir);
}
