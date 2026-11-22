use super::*;

#[test]
fn restart_follows_the_ownership_rules() {
    assert_eq!(
        restart_plan(EngineState::Stopped, false),
        RestartChoice::Ensure
    );
    assert_eq!(
        restart_plan(EngineState::Running, false),
        RestartChoice::LeaveForeign
    );
    assert_eq!(
        restart_plan(EngineState::Running, true),
        RestartChoice::QuitThenEnsure
    );
    assert_eq!(
        restart_plan(EngineState::Starting, true),
        RestartChoice::Ensure
    );
}

#[test]
fn a_foreign_kodi_is_not_replaced() {
    assert!(!spawn_allowed(true));
    assert!(spawn_allowed(false));
}

#[test]
fn one_process_can_be_ours_and_two_cannot() {
    assert_eq!(attach_decision(&[], None), (None, false));
    assert_eq!(attach_decision(&[7], None), (Some(7), false));
    assert_eq!(attach_decision(&[7], Some(7)), (Some(7), true));
    assert_eq!(attach_decision(&[7, 8], Some(7)), (None, false));
}

#[test]
fn a_start_needs_proof_that_no_kodi_is_running() {
    assert_eq!(start_gate(Some(&[])), StartGate::Start);
    assert_eq!(start_gate(Some(&[4])), StartGate::AlreadyRunning);
    assert_eq!(start_gate(Some(&[4, 9])), StartGate::AlreadyRunning);
    assert_eq!(start_gate(None), StartGate::Unknown);
    assert!(refuse_start(Some(&[])).is_ok());
    assert!(refuse_start(Some(&[4])).is_err());
    assert!(refuse_start(None).is_err());
    assert!(!extra_started(4, Some(&[4])));
    assert!(extra_started(4, Some(&[4, 9])));
    assert!(extra_started(4, Some(&[9])));
    assert!(!extra_started(4, Some(&[])));
    assert!(!extra_started(4, None));
}

#[test]
fn only_our_lone_kodi_is_replaced_when_stuck() {
    assert_eq!(stuck_owned(&[7], Some(7)), Some(7));
    assert_eq!(stuck_owned(&[7], None), None);
    assert_eq!(stuck_owned(&[7], Some(8)), None);
    assert_eq!(stuck_owned(&[7, 8], Some(7)), None);
    assert_eq!(stuck_owned(&[], Some(7)), None);
}

#[tokio::test]
async fn a_port_that_does_not_answer_ping_is_not_kodi() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let kodi = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let kodi_port = kodi.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = kodi.accept().await.unwrap();
        let mut buf = vec![0u8; 4096];
        let n = socket.read(&mut buf).await.unwrap();
        let request: Value = serde_json::from_slice(&buf[..n]).unwrap();
        let body = json!({"jsonrpc":"2.0","id":request["id"],"result":"pong"});
        socket
            .write_all(serde_json::to_vec(&body).unwrap().as_slice())
            .await
            .unwrap();
    });
    assert!(answers_ping("127.0.0.1", kodi_port).await);

    let other = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other_port = other.local_addr().unwrap().port();
    tokio::spawn(async move {
        let (mut socket, _) = other.accept().await.unwrap();
        socket
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await
            .unwrap();
    });
    assert!(!answers_ping("127.0.0.1", other_port).await);
}

#[test]
fn image_paths_are_web_art_or_kodi_files_only() {
    let roots = vec![
        PathBuf::from(r"C:\Users\someone\AppData\Roaming\Kodi"),
        PathBuf::from(r"C:\Program Files\Kodi"),
    ];
    let ok = |path: &str| image_path_allowed(path, &roots);
    // Web art, as Kodi wraps it.
    assert!(ok(
        "image://https%3a%2f%2fimage.example.org%2ft%2fp%2fw500%2fposter.jpg/"
    ));
    assert!(ok(
        "IMAGE://http%3a%2f%2fart.example.com%2fa.png/@size=small"
    ));
    // Files inside Kodi's own folders.
    assert!(ok(
        r"image://C%3a%5cUsers%5csomeone%5cAppData%5cRoaming%5cKodi%5caddons%5cplugin.video.example%5cicon.png/"
    ));
    assert!(ok(
        "image://C%3a%5cProgram%20Files%5cKodi%5caddons%5cskin.estuary%5cmedia%5cx.png/"
    ));
    assert!(ok("special://home/addons/plugin.video.example/icon.png"));
    assert!(ok("SPECIAL://profile/Thumbnails/a/b.jpg"));
    // Anything else.
    for bad in [
        "image://abc",
        "SPECIAL://skin",
        "special://home/addons/../userdata/guisettings.xml",
        "http://127.0.0.1/a.jpg",
        "image://http%3a%2f%2f127.0.0.1%3a8490%2fapi%2fbrowse/",
        "image://http%3a%2f%2flocalhost%2fx.png/",
        "image://http%3a%2f%2f192.168.1.5%2fx.png/",
        "image://http%3a%2f%2f10.0.0.2%2fx.png/",
        "image://http%3a%2f%2fmypc%2fx.png/",
        "image://http%3a%2f%2fuser%3apw%40example.com%2fx.png/",
        "image://http%3a%2f%2f%5b%3a%3a1%5d%2fx.png/",
        r"image://C%3a%5cUsers%5csomeone%5cPictures%5cme.jpg/",
        r"image://C%3a%5cUsers%5csomeone%5cAppData%5cRoaming%5cKodi%5c..%5c..%5csecret.jpg/",
        r"image://%5c%5cserver%5cshare%5cx.jpg/",
        "image://bad\n",
        "special://bad\r",
    ] {
        assert!(!ok(bad), "{bad}");
    }
}

#[test]
fn a_random_password_is_long_and_plain() {
    let one = random_password().unwrap();
    let two = random_password().unwrap();
    assert_eq!(one.len(), 32);
    assert!(one.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    assert_ne!(one, two);
}

#[test]
fn owned_pid_roundtrips_without_a_password_field() {
    let path = std::env::temp_dir().join(format!(
        "portal-owned-{}-{}.json",
        std::process::id(),
        line!()
    ));
    // A running process is recognised again by its id and start time.
    let me = std::process::id();
    save_owned(&path, Some(me)).unwrap();
    assert_eq!(load_owned(&path), Some(me));
    // An id with no matching process (or one reused later) is not ours.
    fs::write(&path, r#"{"owned_pid":42,"started":1}"#).unwrap();
    assert_eq!(load_owned(&path), None);
    fs::write(&path, r#"{"owned_pid":42}"#).unwrap();
    assert_eq!(load_owned(&path), None);
    save_owned(&path, Some(me)).unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains("owned_pid"));
    assert!(!text.to_ascii_lowercase().contains("password"));
    save_owned(&path, None).unwrap();
    assert_eq!(load_owned(&path), None);
    let _ = fs::remove_file(&path);
}

#[test]
fn guisettings_reads_port_and_user_and_never_the_default_password() {
    let xml = r#"<settings version="2">
            <setting id="services.webserverusername">user</setting>
            <setting id="services.webserverport">8088</setting>
            <setting id="services.webserverpassword" default="SECRETTOKEN"></setting>
        </settings>"#;
    let creds = parse_web_creds(xml).unwrap();
    assert_eq!(creds.port, 8088);
    assert_eq!(creds.username, "user");
    assert!(creds.password.is_empty());

    let parent = r#"<settings><services>
            <webserverusername>user</webserverusername>
            <webserverport>9091</webserverport>
            <webserverpassword></webserverpassword>
        </services></settings>"#;
    let creds = parse_web_creds(parent).unwrap();
    assert_eq!(creds.port, 9091);
    assert_eq!(creds.username, "user");
    assert!(creds.password.is_empty());

    let missing = r#"<settings><!-- SECRETTOKEN -->
            <setting id="services.webserverusername">user</setting>
        </settings>"#;
    let err = match parse_web_creds(missing) {
        Ok(_) => panic!("a missing password element must fail"),
        Err(err) => err,
    };
    let shown = err.to_string();
    assert!(!shown.contains("SECRETTOKEN"));
    assert_eq!(crate::public_error(&shown), "Kodi setup failed.");
    let auth = image_failure(401);
    assert!(!auth.to_string().contains("SECRETTOKEN"));
    assert_eq!(auth.to_string(), "Kodi setup failed.");
}

#[tokio::test]
async fn a_changed_web_server_setting_is_read_again_after_a_failed_image() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn answer_once(status: &'static str) -> u16 {
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = server.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let _ = socket.read(&mut buf).await;
            let body = [0x89, 0x50, 0x4E, 0x47];
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket.write_all(head.as_bytes()).await.unwrap();
            socket.write_all(&body).await.unwrap();
        });
        port
    }
    let profile =
        std::env::temp_dir().join(format!("portal-creds-{}-{}", std::process::id(), line!()));
    fs::create_dir_all(profile.join("userdata")).unwrap();
    let write_settings = |port: u16, password: &str| {
        let xml = format!(
            r#"<settings version="2">
                <setting id="services.webserverport">{port}</setting>
                <setting id="services.webserverpassword">{password}</setting>
            </settings>"#
        );
        fs::write(profile.join("userdata").join("guisettings.xml"), xml).unwrap();
    };
    let supervisor = Supervisor::start(ProcessConfig {
        host: "127.0.0.1".into(),
        port: 9,
        kodi_exe: PathBuf::from("kodi.exe"),
        profile: profile.clone(),
        state_path: profile.join("engine.json"),
    });
    write_settings(answer_once("401 Unauthorized").await, "old");
    assert!(supervisor
        .image("image://https%3a%2f%2fart.example.com%2fposter.jpg/")
        .await
        .is_err());
    // The user changed the password in Kodi; the next image uses it.
    write_settings(answer_once("200 OK").await, "new");
    let (bytes, kind) = supervisor
        .image("image://https%3a%2f%2fart.example.com%2fposter.jpg/")
        .await
        .unwrap();
    assert_eq!(bytes, [0x89, 0x50, 0x4E, 0x47]);
    assert_eq!(kind, "image/png");
    let _ = fs::remove_dir_all(&profile);
}

#[test]
fn a_real_password_element_is_not_copied_into_errors() {
    let xml = r#"<settings version="2">
            <setting id="services.webserverpassword">SECRETTOKEN</setting>
            <setting id="services.webserverusername">user</setting>
        </settings>"#;
    let creds = parse_web_creds(xml).unwrap();
    assert_eq!(creds.username, "user");
    assert_eq!(creds.port, 8080);
    assert!(!creds.password.is_empty());
    let broken = match parse_web_creds("<settings") {
        Ok(_) => panic!("broken settings xml must fail"),
        Err(err) => err,
    };
    assert!(!broken.to_string().contains("SECRETTOKEN"));
}

#[test]
fn overlapping_reveals_restore_where_kodi_was_before_the_first() {
    let place = |left: i32| PlacedWindow {
        hwnd: 7,
        left,
        top: left,
        width: 960,
        height: 540,
    };
    let hidden = place(-32000);
    let shown = place(80);
    // Outer reveal of a hidden Kodi, then an inner one that sees it shown.
    assert!(begin_reveal(std::slice::from_ref(&hidden), false));
    assert!(!begin_reveal(std::slice::from_ref(&shown), true));
    // The outer ends first; the inner, ending last, restores the outer's places.
    assert_eq!(end_reveal(), None);
    assert_eq!(end_reveal(), Some(vec![hidden]));
    // A Kodi that was already on screen is left where it is.
    assert!(begin_reveal(std::slice::from_ref(&shown), true));
    assert_eq!(end_reveal(), None);
}

#[test]
fn a_new_profile_gets_a_settings_file_with_the_web_server_on() {
    let (text, creds) = web_server_settings(None, "PW123").unwrap();
    let back = parse_web_creds(&text).unwrap();
    assert_eq!(
        (back.username.as_str(), back.password.as_str(), back.port),
        ("kodi", "PW123", 8080)
    );
    assert_eq!(
        (creds.username.as_str(), creds.password.as_str()),
        ("kodi", "PW123")
    );
    assert!(text.contains(r#"<setting id="services.webserver">true</setting>"#));
}

#[test]
fn the_web_server_is_turned_on_keeping_the_rest_of_the_file() {
    let xml = "<settings version=\"2\">\r\n    <setting id=\"lookandfeel.skin\" default=\"true\">skin.estuary</setting>\r\n    <setting id=\"services.webserver\" default=\"true\">false</setting>\r\n    <setting id=\"services.webserverport\">8099</setting>\r\n    <setting id=\"services.webserverusername\" default=\"true\">kodi</setting>\r\n    <setting id=\"services.webserverpassword\" default=\"true\" />\r\n</settings>\r\n";
    let (text, creds) = web_server_settings(Some(xml), "NEWPASS").unwrap();
    assert!(
        text.contains(r#"<setting id="lookandfeel.skin" default="true">skin.estuary</setting>"#)
    );
    assert!(text.contains(r#"<setting id="services.webserver">true</setting>"#));
    assert!(text.contains(r#"<setting id="services.webserverpassword">NEWPASS</setting>"#));
    assert!(!text.contains("default=\"true\">false"));
    assert_eq!(
        (creds.port, creds.username.as_str(), creds.password.as_str()),
        (8099, "kodi", "NEWPASS")
    );
    assert_eq!(parse_web_creds(&text).unwrap().password, "NEWPASS");
}

#[test]
fn a_kept_password_and_a_running_server_are_left_alone() {
    let on = r#"<settings version="2"><setting id="services.webserver">true</setting></settings>"#;
    assert!(web_server_settings(Some(on), "X").is_none());
    let off = r#"<settings version="2">
            <setting id="services.webserver">false</setting>
            <setting id="services.webserverusername">me</setting>
            <setting id="services.webserverpassword">mine</setting>
        </settings>"#;
    let (text, creds) = web_server_settings(Some(off), "X").unwrap();
    assert_eq!(
        (creds.username.as_str(), creds.password.as_str()),
        ("me", "mine")
    );
    assert!(text.contains(r#"<setting id="services.webserverpassword">mine</setting>"#));
    // Missing settings are added before the end of the file.
    let bare = "<settings version=\"2\">\n</settings>\n";
    let (text, _) = web_server_settings(Some(bare), "PW").unwrap();
    assert_eq!(parse_web_creds(&text).unwrap().password, "PW");
    assert!(text.trim_end().ends_with("</settings>"));
}

#[test]
fn a_file_that_is_not_kodi_settings_is_not_touched() {
    assert!(web_server_settings(Some("<settings"), "X").is_none());
    assert!(web_server_settings(Some("<advancedsettings></advancedsettings>"), "X").is_none());
    assert!(web_server_settings(Some(""), "X").is_none());
}

#[test]
fn with_setting_replaces_whole_elements_only() {
    let xml = r#"<settings><setting id="services.webserverport">1</setting><setting id="services.webserver" default="true">false</setting></settings>"#;
    let out = with_setting(xml, "services.webserver", "true").unwrap();
    assert_eq!(
        out,
        r#"<settings><setting id="services.webserverport">1</setting><setting id="services.webserver">true</setting></settings>"#
    );
    let escaped = with_setting("<settings></settings>", "a", "x<&\"y").unwrap();
    assert!(escaped.contains("x&lt;&amp;&quot;y"));
}

#[test]
fn prepare_web_server_writes_the_file_once() {
    let dir = std::env::temp_dir().join(format!("portal-web-{}-{}", std::process::id(), line!()));
    let path = dir.join("userdata").join("guisettings.xml");
    let marker = dir.join("kodi-web-server-on");
    let creds = prepare_web_server(&path, &marker).unwrap();
    assert_eq!(
        parse_web_creds(&fs::read_to_string(&path).unwrap())
            .unwrap()
            .password,
        creds.password
    );
    assert!(marker.is_file());
    assert!(prepare_web_server(&path, &marker).is_none());
    assert!(!path.with_extension("xml.portal").exists());
    // Turned off after Portal turned it on: the viewer's choice stands.
    fs::write(
        &path,
        r#"<settings version="2"><setting id="services.webserver">false</setting></settings>"#,
    )
    .unwrap();
    assert!(prepare_web_server(&path, &marker).is_none());
    // A cut-off file is never edited.
    let _ = fs::remove_file(&marker);
    fs::write(
        &path,
        r#"<settings version="2"><setting id="services.webserver">false</setting>"#,
    )
    .unwrap();
    assert!(prepare_web_server(&path, &marker).is_none());
    // An empty file is a fresh start.
    fs::write(&path, "").unwrap();
    assert!(prepare_web_server(&path, &marker).is_some());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn loose_numeric_hosts_and_inner_ranges_are_not_web_art() {
    let roots: Vec<PathBuf> = Vec::new();
    let art =
        |host: &str| image_path_allowed(&format!("image://http%3a%2f%2f{host}%2fa.jpg/"), &roots);
    assert!(!art("127.1"));
    assert!(!art("0x7f.1"));
    assert!(!art("0177.0.0.1"));
    assert!(!art("0.0.0.0"));
    assert!(!art("224.0.0.1"));
    assert!(!art("%5b%3a%3affff%3a127.0.0.1%5d"));
    assert!(art("203.0.113.9"));
    assert!(art("art.example.com"));
    assert_eq!(
        web_art_host("image://https%3a%2f%2fart.example.com%3a8443%2fp.jpg/"),
        Some(("art.example.com".to_string(), 8443))
    );
    assert_eq!(
        web_art_host("image://http%3a%2f%2fart.example.com%2fp.jpg/"),
        Some(("art.example.com".to_string(), 80))
    );
    assert_eq!(web_art_host("special://home/addons/x/icon.png"), None);
}

/// Starts the installed Kodi as Portal does and checks no window of it
/// stays on screen while it starts. Run by hand, with no Kodi running:
/// `cargo test -p portal-engine kodi_starts_hidden -- --ignored`.
/// On Linux it starts `KODI_EXE`, else `kodi` from PATH, on the X display.
#[test]
#[ignore]
fn kodi_starts_hidden() {
    #[cfg(windows)]
    let pid = {
        let exe = Path::new(r"C:\Program Files\Kodi\kodi.exe");
        if !exe.is_file() {
            return;
        }
        spawn_hidden(exe, DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
            .expect("Kodi starts")
    };
    #[cfg(target_os = "linux")]
    let pid = {
        let exe = std::env::var_os("KODI_EXE").map_or_else(|| PathBuf::from("kodi"), PathBuf::from);
        spawn_kodi(&exe).expect("Kodi starts")
    };
    guard_start(pid);
    let mut saw_window = false;
    let mut shown_since: Option<std::time::Instant> = None;
    let mut longest = Duration::ZERO;
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        saw_window |= !windows_of(pid).is_empty();
        if window_visible(pid) {
            let since = *shown_since.get_or_insert_with(std::time::Instant::now);
            longest = longest.max(since.elapsed());
        } else {
            shown_since = None;
        }
        // A sleep here lasts up to a timer tick (about 15 ms), longer than
        // what is being measured, so look again at once.
        std::thread::yield_now();
    }
    end_start_guard();
    stop_pid(pid);
    println!("longest on screen: {longest:?}");
    assert!(saw_window, "Kodi made no window");
    assert!(longest < Duration::from_millis(50), "Kodi showed for {longest:?}");
}
