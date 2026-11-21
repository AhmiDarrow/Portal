use super::*;

#[test]
fn searches_expire_by_outcome() {
    let searches = Searches::default();
    searches.put("a", Ok(Vec::new()));
    searches.put("b", Err(Some("busy".into())));
    assert_eq!(searches.get("a"), Some(Ok(Vec::new())));
    assert_eq!(searches.get("b"), Some(Err(Some("busy".into()))));
    assert_eq!(searches.get("c"), None);
    searches.clear();
    assert_eq!(searches.get("a"), None);
    // "Try again in a minute" must be true a minute later.
    assert_eq!(ttl_for(&Err(Some("busy".into()))), NOTE_TTL);
    assert!(NOTE_TTL <= Duration::from_secs(60));
    assert_eq!(ttl_for(&Err(None)), FAILED_TTL);
    assert_eq!(ttl_for(&Ok(Vec::new())), EMPTY_TTL);
}

#[test]
fn files_persist_and_stay_bounded() {
    let dir = std::env::temp_dir().join(format!("portal-subtitle-files-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let files = Files::open(Some(&dir));
    files.put("gestdown:a", Arc::new("WEBVTT\n\nA\n".into()));
    assert_eq!(
        files.get("gestdown:a").as_deref().map(String::as_str),
        Some("WEBVTT\n\nA\n")
    );
    let reopened = Files::open(Some(&dir));
    assert_eq!(
        reopened.get("gestdown:a").as_deref().map(String::as_str),
        Some("WEBVTT\n\nA\n")
    );
    assert_eq!(reopened.get("gestdown:b"), None);

    for index in 0..5 {
        std::fs::write(dir.join(format!("{index}.vtt")), vec![b'x'; 100]).unwrap();
    }
    prune(&dir, 3, 250);
    let left = std::fs::read_dir(&dir).unwrap().count();
    assert!(left <= 3, "{left}");

    let memory = Files::open(None);
    for index in 0..(MEMORY_FILES + 3) {
        memory.put(&format!("k{index}"), Arc::new(format!("WEBVTT {index}")));
    }
    assert_eq!(memory.get("k0"), None);
    assert!(memory.get(&format!("k{}", MEMORY_FILES + 2)).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}
