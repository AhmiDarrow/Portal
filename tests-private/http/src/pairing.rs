use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "portal-pair-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lan(last: u8) -> SocketAddr {
    SocketAddr::from(([192, 0, 2, last], 50000))
}

fn with_cookie(key: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::COOKIE,
        HeaderValue::from_str(&format!("theme=dark; {COOKIE}={key}")).unwrap(),
    );
    headers
}

#[test]
fn a_number_typed_on_this_pc_pairs_the_device_once() {
    let dir = scratch("flow");
    let pairing = Pairing::open(&dir);
    let tv = lan(20);
    assert!(!pairing.allowed(tv, &HeaderMap::new()));
    let started = pairing.start(tv, "TV".into()).unwrap();
    assert_eq!(started.code.len(), 6);
    assert!(started.code.bytes().all(|byte| byte.is_ascii_digit()));
    assert_eq!(pairing.status(&started.ticket), Ok(None));
    assert!(pairing.approve("000000x").is_err() || started.code == "000000");
    let spaced = format!("{} {}", &started.code[..3], &started.code[3..]);
    assert_eq!(pairing.approve(&spaced).unwrap(), "TV");
    // The same number does not pair a second device.
    assert!(pairing.approve(&started.code).is_err());
    let key = pairing.status(&started.ticket).unwrap().unwrap();
    // The key is handed out once.
    assert!(pairing.status(&started.ticket).is_err());
    assert!(pairing.allowed(tv, &with_cookie(&key)));
    assert!(!pairing.allowed(tv, &with_cookie(&"0".repeat(64))));
    let saved = std::fs::read_to_string(dir.join("devices.json")).unwrap();
    assert!(!saved.contains(&key));
    assert!(saved.contains(&digest(&key)));
    // A restart keeps the device; removing it ends its access.
    let again = Pairing::open(&dir);
    assert!(again.allowed(tv, &with_cookie(&key)));
    let id = again.list()[0].id.clone();
    assert!(again.remove(&id));
    assert!(!again.allowed(tv, &with_cookie(&key)));
    assert!(!again.remove(&id));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn this_pc_never_pairs_and_one_address_cannot_flood() {
    let dir = scratch("limits");
    let pairing = Pairing::open(&dir);
    assert!(pairing.allowed(SocketAddr::from(([127, 0, 0, 1], 1)), &HeaderMap::new()));
    let phone = lan(30);
    let first = pairing.start(phone, "Phone".into()).unwrap();
    for _ in 1..MAX_PENDING_PER_ADDRESS {
        pairing.start(phone, "Phone".into()).unwrap();
    }
    // One more replaces that address's oldest number, never anyone else's.
    assert!(pairing.start(phone, "Phone".into()).is_ok());
    assert!(pairing.status(&first.ticket).is_err());
    assert!(pairing.start(lan(31), "Phone".into()).is_ok());
    // A proxy on this PC passing a request along is not this PC.
    let mut proxied = HeaderMap::new();
    proxied.insert("x-forwarded-for", "203.0.113.5".parse().unwrap());
    assert!(!pairing.allowed(SocketAddr::from(([127, 0, 0, 1], 1)), &proxied));
    assert!(pairing.status("not-a-ticket").is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_pairing_and_what_a_tv_loads_skip_the_check() {
    assert!(open_path("/api/pair/start"));
    assert!(open_path("/api/pair/status"));
    assert!(!open_path("/api/image"));
    assert!(open_path("/api/subtitles/file/abc"));
    assert!(!open_path("/api/devices"));
    assert!(!open_path("/api/pair/approve"));
    assert!(!open_path("/api/watchlist"));
    assert!(!open_path("/api/play"));
}

#[test]
fn devices_get_plain_names() {
    assert_eq!(
        device_name("Mozilla/5.0 (SMART-TV; Linux; Tizen 6.0)"),
        "TV"
    );
    assert_eq!(
        device_name("Mozilla/5.0 (Linux; Android 14; Pixel 8) Mobile"),
        "Phone"
    );
    assert_eq!(
        device_name("Mozilla/5.0 (iPad; CPU OS 17_0 like Mac OS X)"),
        "Tablet"
    );
    assert_eq!(
        device_name("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
        "Computer"
    );
    assert_eq!(device_name(""), "Device");
}

#[test]
fn other_devices_may_watch_and_cast_but_change_nothing_else() {
    let get = Method::GET;
    let post = Method::POST;
    let put = Method::PUT;
    let delete = Method::DELETE;
    // Watching.
    assert!(remote_may(&get, "/api/home"));
    assert!(remote_may(&get, "/api/browse"));
    assert!(remote_may(&get, "/api/subtitles/settings"));
    assert!(remote_may(&post, "/api/play"));
    assert!(remote_may(&delete, "/api/play/abc"));
    assert!(remote_may(&post, "/api/play/abc/seek"));
    assert!(remote_may(&post, "/api/prompt"));
    assert!(remote_may(&put, "/api/prefs"));
    assert!(remote_may(&put, "/api/watchlist/x"));
    assert!(remote_may(&delete, "/api/history/3"));
    // Casting.
    assert!(remote_may(&post, "/api/cast/presence"));
    assert!(remote_may(&put, "/api/cast/session"));
    assert!(remote_may(&post, "/api/cast/devices/tv/volume"));
    assert!(remote_may(&post, "/api/pair/start"));
    // Settings and management stay on the PC.
    for (method, path) in [
        (&get, "/api/addons/plugin.video.example/settings"),
        (&put, "/api/addons/plugin.video.example/settings/quality"),
        (&post, "/api/addons/plugin.video.example/actions/auth"),
        (&delete, "/api/addons/plugin.video.example"),
        (&post, "/api/run"),
        (&get, "/api/package-install"),
        (&post, "/api/incoming"),
        (&put, "/api/source"),
        (&put, "/api/cast"),
        (&put, "/api/metadata"),
        (&get, "/api/metadata"),
        (&put, "/api/subtitles/settings"),
        (&delete, "/api/history"),
        (&get, "/api/repos"),
        (&post, "/api/repos"),
        (&post, "/api/engine/restart"),
        (&post, "/api/engine/window"),
        (&post, "/api/update/check"),
        (&post, "/api/update/install"),
        (&post, "/api/shutdown"),
        (&post, "/api/pair/approve"),
        (&get, "/api/devices"),
        (&delete, "/api/devices/abc"),
        (&post, "/api/some-new-route"),
    ] {
        assert!(!remote_may(method, path), "{method} {path}");
    }
}
