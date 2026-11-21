use super::*;
use axum::http::HeaderValue;

fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(*name, HeaderValue::from_str(value).unwrap());
    }
    map
}

#[test]
fn only_local_hosts_are_answered() {
    for host in [
        "127.0.0.1:8490",
        "localhost:8490",
        "192.0.2.10:8490",
        "[::1]:8490",
        "[fe80::1]",
        "desktop-abc:8490",
        "desktop-abc.local:8490",
        "nas.home.arpa",
        "pc.tail1234.ts.net:8490",
        "pc.fritz.box",
        "pc.localdomain.",
        "LOCALHOST",
    ] {
        assert!(local_host(host, &[]), "{host}");
    }
    for host in [
        "evil.example:8490",
        "127.0.0.1.nip.io:8490",
        "attacker.com",
        "[::1",
        "[::1]x",
        "",
        "bad host",
    ] {
        assert!(!local_host(host, &[]), "{host}");
    }
    assert!(local_host(
        "portal.example.com:443",
        &["portal.example.com".into()]
    ));
}

#[test]
fn a_rebound_name_is_refused_even_for_reads() {
    let rebound = headers(&[("host", "evil.example:8490")]);
    assert!(check(&Method::GET, "/", &rebound, &[], false).is_err());
    let local = headers(&[("host", "127.0.0.1:8490")]);
    assert!(check(&Method::GET, "/", &local, &[], false).is_ok());
    // The tray launcher and old clients send no Origin or Host.
    assert!(check(&Method::POST, "/api/shutdown", &HeaderMap::new(), &[], false).is_ok());
}

#[test]
fn changes_from_other_sites_are_refused() {
    let own = headers(&[
        ("host", "192.0.2.10:8490"),
        ("origin", "http://192.0.2.10:8490"),
        ("sec-fetch-site", "same-origin"),
    ]);
    assert!(check(&Method::POST, "/api/prefs", &own, &[], false).is_ok());
    let cross = headers(&[
        ("host", "127.0.0.1:8490"),
        ("origin", "https://evil.example"),
        ("sec-fetch-site", "cross-site"),
    ]);
    assert!(check(&Method::POST, "/api/prefs", &cross, &[], false).is_err());
    let other_port = headers(&[
        ("host", "127.0.0.1:8490"),
        ("origin", "http://127.0.0.1:3000"),
    ]);
    assert!(check(&Method::DELETE, "/api/history", &other_port, &[], false).is_err());
    let opaque = headers(&[("host", "127.0.0.1:8490"), ("origin", "null")]);
    assert!(check(&Method::PUT, "/api/prefs", &opaque, &[], false).is_err());
    let same_site = headers(&[("host", "127.0.0.1:8490"), ("sec-fetch-site", "same-site")]);
    assert!(check(&Method::POST, "/api/prefs", &same_site, &[], false).is_err());
}

#[test]
fn other_sites_may_only_read_what_a_tv_loads() {
    let cross = headers(&[("host", "192.0.2.5:8490"), ("sec-fetch-site", "cross-site")]);
    // A browse runs the add-on and saves titles; a prompt check starts Kodi.
    for path in [
        "/api/browse",
        "/api/search",
        "/api/prompt",
        "/api/cast/devices",
    ] {
        assert!(check(&Method::GET, path, &cross, &[], false).is_err(), "{path}");
    }
    for path in [
        "/",
        "/index.html",
        "/media/abc/index.m3u8",
        "/api/subtitles/file/gestdown:1",
    ] {
        assert!(check(&Method::GET, path, &cross, &[], false).is_ok(), "{path}");
    }
    assert!(check(&Method::GET, "/api/image", &cross, &[], false).is_err());
    let same_site = headers(&[("host", "192.0.2.5:8490"), ("sec-fetch-site", "same-site")]);
    assert!(check(&Method::GET, "/api/browse", &same_site, &[], false).is_err());
    for site in ["same-origin", "none"] {
        let own = headers(&[("host", "192.0.2.5:8490"), ("sec-fetch-site", site)]);
        assert!(
            check(&Method::GET, "/api/browse", &own, &[], false).is_ok(),
            "{site}"
        );
    }
    // The launcher and older browsers send no Sec-Fetch-Site.
    let plain = headers(&[("host", "127.0.0.1:8490")]);
    assert!(check(&Method::GET, "/api/browse", &plain, &[], false).is_ok());
}

#[test]
fn this_pc_answers_only_its_own_names() {
    let own: Vec<String> = ["mypc", "mypc.local", "mypc.corp.example", "portal.example.com"]
        .map(String::from)
        .into();
    for host in [
        "127.0.0.1:8490",
        "127.9.9.9",
        "localhost:8490",
        "LOCALHOST.",
        "app.localhost:8490",
        "[::1]:8490",
        "mypc:8490",
        "MyPC.local:8490",
        "mypc.corp.example",
        "portal.example.com:443",
    ] {
        let at = headers(&[("host", host)]);
        assert!(check(&Method::POST, "/api/repos", &at, &own, true).is_ok(), "{host}");
    }
    // A LAN neighbour can answer these lookups and rebind them to this PC.
    for host in [
        "evil:8490",
        "evil.local:8490",
        "pc.fritz.box",
        "nas.home.arpa",
        "pc.tail1234.ts.net",
        "192.0.2.10:8490",
        "[2001:db8::1]:8490",
        "evil.example",
        "[::1",
        "",
    ] {
        let at = headers(&[("host", host)]);
        assert!(check(&Method::POST, "/api/repos", &at, &own, true).is_err(), "{host}");
        assert!(check(&Method::GET, "/", &at, &own, true).is_err(), "{host}");
    }
}

#[test]
fn a_rebound_lan_name_cannot_install_from_this_pc() {
    // The page is same-origin with its own (rebound) name, so the browser
    // sends what Portal's own page would; only the Host gives it away.
    let rebound = headers(&[
        ("host", "printer.local:8490"),
        ("origin", "http://printer.local:8490"),
        ("sec-fetch-site", "same-origin"),
    ]);
    assert!(check(&Method::POST, "/api/repos", &rebound, &[], true).is_err());
    assert!(check(&Method::GET, "/api/browse", &rebound, &[], true).is_err());
}

#[test]
fn paired_devices_keep_their_lan_names() {
    // Another device (never this PC's rights) opens Portal by IP or a LAN name.
    for host in [
        "192.0.2.10:8490",
        "desktop-abc:8490",
        "desktop-abc.local:8490",
        "pc.fritz.box:8490",
    ] {
        let at = headers(&[
            ("host", host),
            ("sec-fetch-site", "same-origin"),
            ("user-agent", "Mozilla/5.0 (Linux; Android 14)"),
        ]);
        assert!(check(&Method::GET, "/api/browse", &at, &[], false).is_ok(), "{host}");
        assert!(check(&Method::POST, "/api/play", &at, &[], false).is_ok(), "{host}");
    }
    let rebound = headers(&[("host", "evil.example:8490")]);
    assert!(check(&Method::GET, "/", &rebound, &[], false).is_err());
}

#[test]
fn reads_without_sec_fetch_site_need_portals_own_page() {
    let browser = "Mozilla/5.0 (SMART-TV; Linux; Tizen 4.0) AppleWebKit/537.36";
    let with = |extra: &[(&'static str, &str)]| {
        let mut pairs = vec![("host", "192.0.2.10:8490"), ("user-agent", browser)];
        pairs.extend_from_slice(extra);
        headers(&pairs)
    };
    // An old browser's own fetch sends the page as Referer.
    let own = with(&[("referer", "http://192.0.2.10:8490/#/browse")]);
    assert!(check(&Method::GET, "/api/browse", &own, &[], false).is_ok());
    let own = with(&[("origin", "http://192.0.2.10:8490")]);
    assert!(check(&Method::GET, "/api/prompt", &own, &[], false).is_ok());
    // Another site's page, or one that hides where it came from.
    for extra in [
        &[("referer", "https://evil.example/x")][..],
        &[("referer", "http://192.0.2.10:9999/")][..],
        &[("origin", "https://evil.example")][..],
        &[("origin", "null")][..],
        &[][..],
    ] {
        let foreign = with(extra);
        for path in ["/api/browse", "/api/prompt", "/api/search"] {
            assert!(check(&Method::GET, path, &foreign, &[], false).is_err(), "{path} {extra:?}");
        }
        // Pages, video and a TV's subtitles still load.
        for path in ["/", "/assets/index-abc.js", "/media/s/index.m3u8", "/api/subtitles/file/x"] {
            assert!(check(&Method::GET, path, &foreign, &[], false).is_ok(), "{path} {extra:?}");
        }
    }
    // The tray launcher and scripts are no browser and send none of these.
    let tool = headers(&[("host", "127.0.0.1:8490"), ("user-agent", "curl/8.4.0")]);
    assert!(check(&Method::GET, "/api/prompt", &tool, &[], true).is_ok());
    let launcher = headers(&[("host", "127.0.0.1:8490")]);
    assert!(check(&Method::GET, "/api/status", &launcher, &[], true).is_ok());
}
