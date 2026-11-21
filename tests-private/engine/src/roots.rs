use super::*;

#[test]
fn https_parts_splits_host_port_and_path() {
    assert_eq!(
        https_parts("https://archive.org/download/a%20b.mp4?x=1"),
        Some(("archive.org".into(), 443, "/download/a%20b.mp4?x=1".into()))
    );
    assert_eq!(
        https_parts("HTTPS://cdn.example.com:8443"),
        Some(("cdn.example.com".into(), 8443, "/".into()))
    );
    assert_eq!(
        https_parts("https://example.com?token=1#frag"),
        Some(("example.com".into(), 443, "/?token=1".into()))
    );
    assert_eq!(
        https_parts("https://[2001:db8::1]:444/v.mkv"),
        Some(("2001:db8::1".into(), 444, "/v.mkv".into()))
    );
}

#[test]
fn https_parts_refuses_other_addresses() {
    assert_eq!(https_parts("http://example.com/v.mp4"), None);
    assert_eq!(https_parts("C:\\Videos\\v.mp4"), None);
    assert_eq!(https_parts("https://user:pass@example.com/v.mp4"), None);
    assert_eq!(https_parts("https:///v.mp4"), None);
    assert_eq!(https_parts("https://example.com:0/v.mp4"), None);
    assert_eq!(https_parts("https://example.com:http/v.mp4"), None);
    assert_eq!(https_parts("https://example.com/a b.mp4"), None);
}

#[test]
fn tests_never_fetch() {
    assert!(!fetch_root("https://example.com/"));
}
