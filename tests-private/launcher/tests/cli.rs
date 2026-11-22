//! The command line the installer uses (installer/portal.iss runs `--stop --quiet` and
//! waits for it, ending Portal's processes itself when it exits non-zero), run as the real
//! `portal-launcher` against a stand-in Portal on a free port, with its data in a scratch
//! folder. Nothing here reaches the installed Portal: another port is another tray.

use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[path = "../fake_portal.rs"]
mod fake_portal;
use fake_portal::FakePortal;

/// A scratch data folder, as `%LOCALAPPDATA%` (`$XDG_DATA_HOME` on Linux), with the first-run notice already seen.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(port: u16) -> Scratch {
        let dir = std::env::temp_dir().join(format!("portal-launcher-cli-{port}"));
        let data = dir.join(if cfg!(windows) { "Portal" } else { "portal" });
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("tray-intro-shown"), b"").unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn launcher(port: u16, scratch: &Scratch, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_portal-launcher"));
    command
        .args(args)
        .env("PORTAL_PORT", port.to_string())
        .env("LOCALAPPDATA", &scratch.0)        .env("XDG_DATA_HOME", &scratch.0);
    command
}

/// Waits up to `limit` for `child` to exit.
fn exit_within(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Ends a child left running by a failed assertion.
struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn stop_quiet_with_nothing_running_succeeds_silently() {
    let fake = FakePortal::start();
    fake.go_away();
    let scratch = Scratch::new(fake.port);
    let mut child = launcher(fake.port, &scratch, &["--stop", "--quiet"])
        .spawn()
        .unwrap();
    let status = exit_within(&mut child, Duration::from_secs(30)).expect("it returns");
    assert_eq!(status.code(), Some(0));
}

#[test]
fn stop_quiet_fails_when_portal_does_not_answer() {
    // A hung server: the installer must end the processes itself.
    let fake = FakePortal::start();
    fake.go_silent();
    let scratch = Scratch::new(fake.port);
    let mut child = launcher(fake.port, &scratch, &["--stop", "--quiet"])
        .spawn()
        .unwrap();
    let status = exit_within(&mut child, Duration::from_secs(30)).expect("it returns");
    assert_eq!(status.code(), Some(1));
}

#[test]
fn stop_quiet_ends_a_running_tray_and_waits_for_it() {
    let fake = FakePortal::start();
    let scratch = Scratch::new(fake.port);
    let tray = launcher(fake.port, &scratch, &["--no-browser"])
        .spawn()
        .unwrap();
    let mut tray = Reap(tray);
    // The tray is up once it has read the server's state.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !fake.requests().iter().any(|l| l.contains("/api/update")) {
        assert!(
            Instant::now() < deadline,
            "the tray never checked the server"
        );
        assert!(tray.0.try_wait().unwrap().is_none(), "the tray ended early");
        std::thread::sleep(Duration::from_millis(50));
    }
    // A plain second launch hands over and returns at once, leaving the tray.
    let mut again = launcher(fake.port, &scratch, &["--no-browser"])
        .spawn()
        .unwrap();
    let status = exit_within(&mut again, Duration::from_secs(15)).expect("it returns");
    assert_eq!(status.code(), Some(0));
    assert!(tray.0.try_wait().unwrap().is_none(), "still one tray");

    let started = Instant::now();
    let mut stop = launcher(fake.port, &scratch, &["--stop", "--quiet"])
        .spawn()
        .unwrap();
    let status = exit_within(&mut stop, Duration::from_secs(60)).expect("--stop returns");
    assert_eq!(status.code(), Some(0), "the installer goes on");
    // --stop returned only after the tray's process was gone.
    let tray_status = tray.0.try_wait().unwrap();
    assert!(
        tray_status.is_some(),
        "the tray ended before --stop returned ({:?})",
        started.elapsed()
    );
    assert_eq!(tray_status.and_then(|s| s.code()), Some(0));
}

#[test]
fn autostart_status_answers_through_the_exit_code() {
    let fake = FakePortal::start();
    let scratch = Scratch::new(fake.port);
    let status = launcher(fake.port, &scratch, &["--autostart", "status"])
        .status()
        .unwrap();
    assert!(matches!(status.code(), Some(0 | 3)), "{status:?}");
    let status = launcher(fake.port, &scratch, &["--autostart", "sideways"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(1));
}
