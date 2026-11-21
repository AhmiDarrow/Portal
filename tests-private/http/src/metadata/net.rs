use super::*;

#[test]
fn urls_encode_queries_and_segments() {
    let url = url(
        "https://example.org/w/api.php",
        &[("srsearch", "Heat 1995 film"), ("format", "json")],
    )
    .unwrap();
    assert_eq!(
        url.as_str(),
        "https://example.org/w/api.php?srsearch=Heat+1995+film&format=json"
    );
    let url = url_with_segment("https://example.org/page/summary/", "AC/DC (band)").unwrap();
    assert_eq!(
        url.as_str(),
        "https://example.org/page/summary/AC%2FDC%20(band)"
    );
}

#[test]
fn keyed_requests_follow_redirects_only_within_their_host() {
    let parse = |text: &str| Url::parse(text).unwrap();
    let asked = [parse("https://api.example.org/v1/subtitles")];
    let same = parse("https://api.example.org/v1/subtitles?page=1");
    let other = parse("https://cdn.example.net/file.zip");
    let other_port = parse("https://api.example.org:8443/v1/subtitles");
    let plain = parse("http://api.example.org/v1/subtitles");

    assert!(follows(&asked, &same, true));
    assert!(!follows(&asked, &other, true));
    assert!(!follows(&asked, &other_port, true));
    assert!(!follows(&asked, &plain, true));
    // Back to the first host after a hop elsewhere is still refused.
    let hopped = [asked[0].clone(), other.clone()];
    assert!(!follows(&hopped, &other, true));

    assert!(follows(&asked, &other, false));
    assert!(!follows(&asked, &plain, false));
    let many = vec![asked[0].clone(); 5];
    assert!(!follows(&many, &same, true));
    assert!(!follows(&many, &other, false));
}

#[test]
fn a_redirect_the_keyed_client_stopped_is_handed_on_only_over_https() {
    let answer = |status: u16, location: &str| {
        let response = axum::http::Response::builder()
            .status(status)
            .header(LOCATION, location)
            .body("")
            .unwrap();
        reqwest::Response::from(response)
    };
    let cdn = "https://cdn.example.net/file.zip";
    assert_eq!(
        elsewhere(&answer(302, cdn)).map(String::from).as_deref(),
        Some(cdn)
    );
    let plain = "http://cdn.example.net/file.zip";
    assert_eq!(elsewhere(&answer(302, plain)), None);
    assert_eq!(elsewhere(&answer(200, cdn)), None);
}
