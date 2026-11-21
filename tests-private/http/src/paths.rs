use super::*;

#[test]
fn source_tree_dashboard_is_under_web() {
    let dist = source_root().join("web").join("dist");
    assert!(dist.ends_with(Path::new("web/dist")));
}

#[test]
fn missing_candidates_fall_back() {
    let picked = first(
        vec![PathBuf::from("does/not/exist.exe")],
        |path| path.is_file(),
        PathBuf::from("ffmpeg"),
    );
    assert_eq!(picked, PathBuf::from("ffmpeg"));
}
