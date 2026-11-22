use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::response::IntoResponse;
use axum::routing::get;

/// A local "site": loopback counts as public for these proxies only.
fn proxy(limits: Limits) -> ImageProxy {
    ImageProxy::new(limits).with_public(|ip| ip.is_loopback() || portal_engine::public_ip(ip))
}

async fn site(hits: Arc<AtomicUsize>) -> String {
    let counted = hits.clone();
    let app = axum::Router::new()
        .route(
            "/a.png",
            get(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                async { ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=600")], vec![1u8; 64]) }
            }),
        )
        .route("/plain.png", get(|| async { ([(header::CONTENT_TYPE, "image/png")], vec![2u8; 8]) }))
        .route("/page", get(|| async { ([(header::CONTENT_TYPE, "text/html")], "<script>1</script>") }))
        .route("/to-home", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "http://192.168.1.1/x.png")]).into_response() }))
        .route("/to-a", get(|| async { (StatusCode::MOVED_PERMANENTLY, [(header::LOCATION, "/a.png")]).into_response() }))
        .route("/loop", get(|| async { (StatusCode::FOUND, [(header::LOCATION, "/loop")]).into_response() }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    format!("http://{address}")
}

fn status(result: Result<Picture, ApiError>) -> StatusCode {
    result.err().map(|e| e.0).unwrap_or(StatusCode::OK)
}

#[tokio::test]
async fn private_and_odd_addresses_are_refused_before_connecting() {
    let real = ImageProxy::new(Limits::default());
    for url in [
        "http://10.0.0.1/a.png",
        "http://192.168.0.9/a.png",
        "http://127.0.0.1:8490/api/status",
        "http://[::1]/a.png",
        "http://localhost/a.png",
        "http://mypc/a.png",
        "http://169.254.169.254/latest/meta-data",
        "http://user:pw@example.com/a.png",
        "ftp://example.com/a.png",
        "file:///C:/Windows/win.ini",
        "not a url",
    ] {
        assert_eq!(status(real.get(url).await), StatusCode::BAD_REQUEST, "{url}");
    }
}

#[tokio::test]
async fn a_redirect_into_the_home_network_is_refused() {
    let base = site(Arc::default()).await;
    let p = proxy(Limits::default());
    assert_eq!(status(p.get(&format!("{base}/to-home")).await), StatusCode::BAD_REQUEST);
    // A redirect to a public picture is followed.
    assert_eq!(p.get(&format!("{base}/to-a")).await.unwrap().bytes.len(), 64);
    assert_eq!(status(p.get(&format!("{base}/loop")).await), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn only_pictures_within_the_size_limit_come_back() {
    let base = site(Arc::default()).await;
    let p = proxy(Limits::default().max_bytes(32));
    assert_eq!(status(p.get(&format!("{base}/page")).await), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(status(p.get(&format!("{base}/a.png")).await), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(p.get(&format!("{base}/plain.png")).await.unwrap().bytes, vec![2u8; 8]);
}

#[tokio::test]
async fn answers_are_cached_and_keep_the_sites_cache_control() {
    let hits = Arc::new(AtomicUsize::new(0));
    let base = site(hits.clone()).await;
    let p = proxy(Limits::default().cache(1, 1 << 20));
    let url = format!("{base}/a.png");
    let first = p.get(&url).await.unwrap();
    assert_eq!(first.cache_control.as_deref(), Some("public, max-age=600"));
    assert_eq!(p.get(&url).await.unwrap(), first);
    assert_eq!(hits.load(Ordering::SeqCst), 1, "the second ask came from the cache");
    // One entry: another picture pushes the first out.
    p.get(&format!("{base}/plain.png")).await.unwrap();
    p.get(&url).await.unwrap();
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    let response = picture_response(first);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "public, max-age=600");
    assert_eq!(response.headers()[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    let plain = picture_response(Picture { bytes: vec![], content_type: "image/png".into(), cache_control: None });
    assert_eq!(plain.headers()[header::CACHE_CONTROL], DEFAULT_CACHE);
}

#[tokio::test]
async fn a_slow_site_runs_out_of_time() {
    let app = axum::Router::new().route(
        "/slow.png",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ([(header::CONTENT_TYPE, "image/png")], vec![0u8; 4])
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });
    let p = proxy(Limits::default().timeout(Duration::from_millis(200)));
    let got = p.get(&format!("http://{address}/slow.png")).await;
    assert!(matches!(status(got), StatusCode::GATEWAY_TIMEOUT | StatusCode::BAD_GATEWAY));
}

#[test]
fn limits_have_defaults_and_setters() {
    assert_eq!(Limits::default().max_bytes, 16 << 20);
    assert_eq!(Limits::default().redirects(2).redirects, 2);
}
