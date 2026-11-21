use super::*;

fn temp_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "portal-library-safety-{name}-{}.json",
        std::process::id()
    ));
    let _ = fs::remove_file(&path);
    path
}

fn movie(id: &str, position: f64) -> ProgressReport {
    ProgressReport {
        id: id.into(),
        kind: TitleKind::Movie,
        name: id.into(),
        art: None,
        season: None,
        episode: None,
        episode_name: None,
        file: Some(id.into()),
        position,
        duration: 7_200.0,
    }
}

fn position(store: &Store, id: &str) -> f64 {
    store.doc.progress[&progress_key(TitleKind::Movie, id, 0, 0)].position
}

/// Ten minutes of two devices reporting every five seconds, one after the
/// other. Returns how many reports rewrote the file.
fn alternate(store: &mut Store, first: &str, second: &str) -> usize {
    let mut writes = 0;
    let mut last = fs::read(&store.path).unwrap_or_default();
    for tick in 0..120 {
        let at = 1_000_000 + tick * 5_000;
        let seconds = tick as f64 * 5.0;
        store
            .apply_progress_from(&movie(first, 100.0 + seconds), at, false, "tv")
            .unwrap();
        store
            .apply_progress_from(&movie(second, 3_000.0 + seconds), at + 1, false, "phone")
            .unwrap();
        let now = fs::read(&store.path).unwrap();
        if now != last {
            writes += 1;
            last = now;
        }
    }
    writes
}

#[test]
fn two_devices_on_two_titles_write_rarely_and_keep_both_places() {
    let path = temp_path("two-titles");
    let mut store = Store::open(&path).unwrap();
    let writes = alternate(&mut store, "movie-a", "movie-b");
    // 240 reports; each device writes on its own 30 second cadence.
    assert!(writes <= 45, "{writes} rewrites of the library file");
    assert_eq!(position(&store, "movie-a"), 100.0 + 119.0 * 5.0);
    assert_eq!(position(&store, "movie-b"), 3_000.0 + 119.0 * 5.0);
    // Each title keeps one history row; a renamed row keeps its name.
    assert_eq!(store.history().len(), 2);
    // Stopping on each device writes its place for the next start.
    store
        .apply_progress_from(&movie("movie-a", 900.0), 2_000_000, true, "tv")
        .unwrap();
    store
        .apply_progress_from(&movie("movie-b", 4_000.0), 2_000_001, true, "phone")
        .unwrap();
    let reopened = Store::open(&path).unwrap();
    assert_eq!(position(&reopened, "movie-a"), 900.0);
    assert_eq!(position(&reopened, "movie-b"), 4_000.0);
    assert_eq!(reopened.history().len(), 2);
    assert_eq!(reopened.home_continue().len(), 2);
    let _ = fs::remove_file(&path);
}

#[test]
fn two_devices_on_one_title_write_rarely_and_the_newest_report_wins() {
    let path = temp_path("one-title");
    let mut store = Store::open(&path).unwrap();
    let writes = alternate(&mut store, "movie-a", "movie-a");
    assert!(writes <= 45, "{writes} rewrites of the library file");
    assert_eq!(position(&store, "movie-a"), 3_000.0 + 119.0 * 5.0);
    assert_eq!(store.history().len(), 1);
    // The device that stops last leaves its place for Continue.
    store
        .apply_progress_from(&movie("movie-a", 3_800.0), 2_000_000, true, "phone")
        .unwrap();
    store
        .apply_progress_from(&movie("movie-a", 750.0), 2_000_001, true, "tv")
        .unwrap();
    let reopened = Store::open(&path).unwrap();
    assert_eq!(position(&reopened, "movie-a"), 750.0);
    assert_eq!(reopened.history()[0].position, 750.0);
    let _ = fs::remove_file(&path);
}

#[test]
fn a_row_played_again_after_another_keeps_its_place_at_the_top() {
    let path = temp_path("row-order");
    let mut store = Store::open(&path).unwrap();
    store.apply_progress(&movie("movie-a", 10.0), 1, true).unwrap();
    store.apply_progress(&movie("movie-b", 10.0), 2, true).unwrap();
    assert!(store.rename_history(0, "My cut").unwrap());
    store.apply_progress(&movie("movie-a", 20.0), 3, true).unwrap();
    let rows = store.history();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].id, "movie-a");
    assert_eq!(rows[0].name, "My cut");
    assert_eq!(rows[0].position, 20.0);
    let _ = fs::remove_file(&path);
}

#[cfg(windows)]
#[test]
fn a_library_file_that_cannot_be_read_is_never_saved_over() {
    use std::os::windows::fs::OpenOptionsExt;
    let path = temp_path("held");
    let mut first = Store::open(&path).unwrap();
    first.apply_progress(&movie("movie-a", 10.0), 1, true).unwrap();
    let before = fs::read(&path).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&path)
        .unwrap();
    let mut store = Store::open(&path).unwrap();
    drop(lock);
    assert!(store.history().is_empty());
    assert!(store.apply_progress(&movie("movie-b", 10.0), 2, true).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
    let _ = fs::remove_file(&path);
}
