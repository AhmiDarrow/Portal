use super::*;

fn info(kind: Option<MediaKind>) -> MediaInfo {
    MediaInfo {
        kind,
        ..MediaInfo::default()
    }
}

#[test]
fn keys_use_the_best_identity() {
    let mut movie = info(Some(MediaKind::Movie));
    assert_eq!(key(&movie), None);
    movie.title = Some("The Shawshank Redemption!".into());
    assert_eq!(
        key(&movie).as_deref(),
        Some("movie:title:the shawshank redemption:")
    );
    movie.year = Some(1994);
    assert_eq!(
        key(&movie).as_deref(),
        Some("movie:title:the shawshank redemption:1994")
    );
    movie.tvdb_id = Some("1".into());
    assert_eq!(key(&movie).as_deref(), Some("movie:tvdb:1"));
    movie.tmdb_id = Some("278".into());
    assert_eq!(key(&movie).as_deref(), Some("movie:tmdb:278"));
    movie.imdb_id = Some("tt0111161".into());
    assert_eq!(key(&movie).as_deref(), Some("movie:imdb:tt0111161"));

    let mut episode = info(Some(MediaKind::Episode));
    episode.title = Some("Pilot".into());
    episode.show_title = Some("Breaking Bad".into());
    episode.season = Some(1);
    episode.episode = Some(2);
    episode.tmdb_id = Some("62085".into());
    assert_eq!(
        key(&episode).as_deref(),
        Some("episode:title:breaking bad::s1e2")
    );
    episode.imdb_id = Some("tt0959621".into());
    assert_eq!(
        key(&episode).as_deref(),
        Some("episode:imdb:tt0959621:s1e2")
    );

    let mut any = info(None);
    any.title = Some("Heat".into());
    assert_eq!(key(&any).as_deref(), Some("any:title:heat:"));
}

#[test]
fn entries_expire_and_persist() {
    let path =
        std::env::temp_dir().join(format!("portal-metadata-cache-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let cache = Cache::open(Some(&path));
    let found = vec![MediaInfo {
        plot: Some("Plot".into()),
        ..MediaInfo::default()
    }];
    cache.put("movie:imdb:tt0111161", found.clone(), FOUND_TTL, 1_000);
    cache.put("movie:title:x:", Vec::new(), FAILED_TTL, 1_000);
    assert_eq!(
        cache.get("movie:imdb:tt0111161", 1_000 + 3600),
        Some(found.clone())
    );
    assert_eq!(cache.get("movie:title:x:", 1_000 + 599), Some(Vec::new()));
    assert_eq!(cache.get("movie:title:x:", 1_000 + 600), None);
    assert_eq!(
        cache.get("movie:imdb:tt0111161", 1_000 + FOUND_TTL.as_secs()),
        None
    );

    // Stored far in the past, so a reload at the real time drops it.
    let reopened = Cache::open(Some(&path));
    assert_eq!(reopened.len(), 0);
    let now = now();
    cache.put("tv:imdb:tt0903747", found.clone(), FOUND_TTL, now);
    let reopened = Cache::open(Some(&path));
    assert_eq!(reopened.get("tv:imdb:tt0903747", now), Some(found));
    reopened.clear();
    assert_eq!(Cache::open(Some(&path)).len(), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_older_snapshot_never_overwrites_a_newer_one() {
    let path = std::env::temp_dir().join(format!(
        "portal-metadata-cache-order-{}.json",
        std::process::id()
    ));
    let writing = Mutex::new(0);
    save(&path, "new", 2, &writing);
    save(&path, "old", 1, &writing);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    save(&path, "newer", 3, &writing);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "newer");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn the_cache_is_bounded() {
    let cache = Cache::open(None);
    for index in 0..(CAPACITY as u64 + 5) {
        cache.put(&format!("k{index}"), Vec::new(), FOUND_TTL, 10_000 + index);
    }
    assert_eq!(cache.len(), CAPACITY);
    assert_eq!(cache.get("k0", 10_500), None);
    assert!(cache.get(&format!("k{}", CAPACITY + 4), 10_500).is_some());
}
