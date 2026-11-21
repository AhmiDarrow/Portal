use super::*;

fn reply(raw: &str) -> Response {
    parse_response(raw).unwrap()
}

#[test]
fn ping_pid_is_read_when_present() {
    let with =
        reply("HTTP/1.1 200 OK\r\n\r\n{\"app\":\"portal\",\"version\":\"0.2.0\",\"pid\":4242}");
    assert_eq!(ping_pid(&with), Some(4242));
    let older = reply("HTTP/1.1 200 OK\r\n\r\n{\"app\":\"portal\",\"version\":\"0.1.0\"}");
    assert_eq!(ping_pid(&older), None);
    let zero = reply("HTTP/1.1 200 OK\r\n\r\n{\"app\":\"portal\",\"pid\":0}");
    assert_eq!(ping_pid(&zero), None);
}

#[test]
fn ping_must_name_portal() {
    let ok = reply("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"app\":\"portal\",\"version\":\"0.1.0\"}");
    assert!(is_portal_ping(&ok));
    assert!(!is_portal_ping(&reply("HTTP/1.1 404 Not Found\r\n\r\n")));
    assert!(!is_portal_ping(&reply(
        "HTTP/1.1 200 OK\r\n\r\n<html>other app</html>"
    )));
    assert!(!is_portal_ping(&reply(
        "HTTP/1.1 200 OK\r\n\r\n{\"app\":\"other\"}"
    )));
    assert!(parse_response("garbage").is_none());
}

#[test]
fn urls_are_loopback() {
    assert_eq!(dashboard_url(8490), "http://127.0.0.1:8490/");
    assert_eq!(server_page_url(8490), "http://127.0.0.1:8490/#/server");
}

#[test]
fn requests_carry_json_bodies() {
    let text = request_text(8490, "PUT", "/api/cast", &[], Some("{\"lan\":false}"));
    assert!(text.starts_with("PUT /api/cast HTTP/1.1\r\n"));
    assert!(text.contains("Content-Length: 13\r\n"));
    assert!(text.contains("Content-Type: application/json\r\n"));
    assert!(text.ends_with("\r\n\r\n{\"lan\":false}"));

    let text = request_text(8490, "POST", "/api/shutdown", &[LAUNCHER_HEADER], None);
    assert!(text.contains("Content-Length: 0\r\n"));
    assert!(text.contains("X-Portal-Launcher: 1\r\n"));
    assert!(!text.contains("Content-Type"));
    assert!(text.ends_with("\r\n\r\n"));
}

#[test]
fn errors_use_the_service_detail() {
    let failed = reply("HTTP/1.1 409 Conflict\r\n\r\n{\"detail\":\"Kodi is busy.\"}");
    assert!(!failed.ok());
    assert_eq!(failed.problem(), "Kodi is busy.");
    let bare = reply("HTTP/1.1 500 Internal Server Error\r\n\r\n");
    assert_eq!(bare.problem(), "Portal answered with HTTP 500.");
}

#[test]
fn reads_status_cast_and_update_bodies() {
    let status: Value = serde_json::from_str(r#"{"state":"running","pid":4}"#).unwrap();
    assert_eq!(engine_state(&status).as_deref(), Some("running"));

    let cast: Value =
        serde_json::from_str(r#"{"lan":false,"bind":"127.0.0.1:8490","restart_needed":true}"#)
            .unwrap();
    assert_eq!(
        parse_lan_reply(&cast),
        Some(LanReply {
            lan: false,
            restart_needed: true
        })
    );
    let plain: Value = serde_json::from_str(r#"{"lan":true}"#).unwrap();
    assert_eq!(
        parse_lan_reply(&plain).map(|r| r.restart_needed),
        Some(false)
    );
    assert_eq!(parse_lan_reply(&serde_json::json!({})), None);

    let update: Value = serde_json::from_str(
            r#"{"current":"0.1.0","latest":"0.2.0","available":true,"state":"downloading","progress":0.5,"error":null,"release_url":null}"#,
        )
        .unwrap();
    let update = parse_update(&update).unwrap();
    assert_eq!(update.latest.as_deref(), Some("0.2.0"));
    assert!(update.available);
    assert_eq!(update.state, "downloading");
    assert_eq!(update.progress, Some(0.5));
    assert_eq!(update.error, None);
    assert_eq!(
        parse_update(&serde_json::json!({"detail": "Not Found"})),
        None
    );
}

#[test]
fn a_listener_that_never_replies_is_silent_not_gone() {
    // Accepted by the system backlog, never read: a busy server.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(!port_free(port));
    assert_eq!(probe(port), Probe::Silent);
    assert!(shutdown(port).is_err(), "not a clean stop");
    drop(listener);
    assert!(port_free(port));
    assert_eq!(probe(port), Probe::Nothing);
    assert_eq!(shutdown(port), Ok(false));
}

#[test]
fn a_slow_but_answering_service_is_portal() {
    // Answers the ping after 1.5 s, inside the probe's 3 s.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::sleep(Duration::from_millis(1500));
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let body = r#"{"app":"portal","pid":4242}"#;
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    assert_eq!(probe_pid(port), (Probe::Portal, Some(4242)));
    // A server that answers in the end is never ended.
    let me = std::env::current_exe().unwrap();
    assert_eq!(kill_hung(port, Some(std::process::id()), &me), Ok(()));
}

/// The hung server `a_hung_portal_is_ended_and_frees_its_port` starts: it
/// accepts on the port it is given and never answers.
#[test]
#[ignore = "started as a child process"]
fn hung_child() {
    let Some(port) = std::env::var("PORTAL_TEST_HUNG_PORT")
        .ok()
        .and_then(|port| port.parse::<u16>().ok())
    else {
        return;
    };
    let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    let _held: Vec<_> = listener.incoming().collect();
}

/// A free port, and a child process that listens on it and never answers.
fn hung_child_on_a_port() -> (u16, std::process::Child) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "service::tests::hung_child"])
        .env("PORTAL_TEST_HUNG_PORT", port.to_string())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while port_free(port) {
        assert!(Instant::now() < deadline, "the child never listened");
        std::thread::sleep(POLL);
    }
    (port, child)
}

#[test]
fn the_port_owner_is_found() {
    let (port, mut child) = hung_child_on_a_port();
    assert_eq!(port_owner(port), Some(child.id()));
    let me = std::env::current_exe().unwrap();
    assert!(same_path(&image_path(child.id()).unwrap(), &me));
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(port_owner(port), None);
}

#[test]
fn a_hung_portal_is_ended_and_frees_its_port() {
    let (port, mut child) = hung_child_on_a_port();
    assert_eq!(probe(port), Probe::Silent);
    let me = std::env::current_exe().unwrap();
    assert_eq!(kill_hung(port, Some(child.id()), &me), Ok(()));
    assert!(child.wait().is_ok());
    assert!(port_free(port));
    assert_eq!(probe(port), Probe::Nothing);
}

#[test]
fn a_hung_portal_that_never_answered_is_found_by_its_port() {
    // No ping was ever answered, so there is no pid to go on.
    let (port, mut child) = hung_child_on_a_port();
    let me = std::env::current_exe().unwrap();
    assert_eq!(kill_hung(port, None, &me), Ok(()));
    assert!(child.wait().is_ok());
    assert!(port_free(port));
}

#[test]
fn another_program_on_the_port_is_left_alone() {
    let (port, mut child) = hung_child_on_a_port();
    let portal = std::path::Path::new(r"C:\Program Files\Portal\portal.exe");
    // Neither the port's owner nor a stale pid that names it is ended.
    for pid in [None, Some(child.id())] {
        let refused = kill_hung(port, pid, portal).unwrap_err();
        let me = std::env::current_exe().unwrap();
        assert!(refused.contains(&me.display().to_string()), "{refused}");
        assert!(
            refused.contains(&format!("(process {})", child.id())),
            "{refused}"
        );
        assert!(refused.ends_with("leaving it alone."), "{refused}");
    }
    assert!(child.try_wait().unwrap().is_none(), "still running");
    assert!(!port_free(port));
    child.kill().unwrap();
    child.wait().unwrap();
}

#[test]
fn an_install_still_downloading_is_not_a_failure() {
    // Accepted, never answered: a download that outlasts the wait.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert_eq!(
        install_update_within(port, Duration::from_millis(300)),
        Ok(())
    );
    drop(listener);
    assert!(install_update_within(port, Duration::from_millis(300)).is_err());
}

/// Talks to a Portal already running on `PORTAL_PORT` (default 8490):
/// `cargo test -p portal-launcher -- --ignored`. Flips "Allow TVs on this
/// network" and puts it back; never touches the Kodi engine.
#[test]
#[ignore = "needs a running Portal"]
fn live_service() {
    let port = port();
    assert_eq!(probe(port), Probe::Portal);
    let health = health(port);
    assert_eq!(health.probe, Probe::Portal);
    assert!(health.engine.is_some(), "{health:?}");
    let lan = health.lan.expect("cast settings");
    let flipped = set_lan(port, !lan).unwrap();
    assert_eq!(flipped.lan, !lan);
    assert!(flipped.restart_needed, "the bind address changes");
    let back = set_lan(port, lan).unwrap();
    assert_eq!(back.lan, lan);
    assert!(!back.restart_needed, "back to the address it listens on");
    if let Some(update) = health.update {
        assert_eq!(update.current, env!("CARGO_PKG_VERSION"));
        let checked = check_update(port).unwrap();
        assert_ne!(checked.state, "checking");
    }
    // Shutdown without the launcher header is refused.
    let refused = request(port, "POST", "/api/shutdown", &[], None, QUICK).unwrap();
    assert_eq!(refused.status, 403);
}
