//! The tray on RemedyUI's in-memory host, against a stand-in Portal on a free port: the
//! menu as the engine's tray shows it, the Quit question, and the watchdog.

use super::*;

use std::cell::RefCell;

use remedy_ui::a11y::Role;
use remedy_ui::host::HostEvent;
use remedy_ui::testing::TestApp;

#[allow(dead_code)]
#[path = "../fake_portal.rs"]
mod fake_portal;
use fake_portal::FakePortal;

struct Running {
    app: TestApp,
    tray: Rc<RefCell<Option<Tray>>>,
    code: Rc<Cell<i32>>,
}

impl Running {
    fn start(port: u16) -> Running {
        let tray: Rc<RefCell<Option<Tray>>> = Rc::default();
        let code = Rc::new(Cell::new(-1));
        let (t2, c2) = (tray.clone(), code.clone());
        let app = TestApp::new(ui::config(), move |app: &App| {
            *t2.borrow_mut() = Some(Tray::start(app, port, false, c2));
        });
        let mut running = Running { app, tray, code };
        running.until("the service answers", |r| {
            r.model(|m| m.service == Service::Running && m.job.is_none() && m.engine.is_some())
        });
        running
    }

    fn model<R>(&self, read: impl FnOnce(&Model) -> R) -> R {
        read(&self.tray.borrow().as_ref().unwrap().model())
    }

    /// Runs frames until `done`, letting background work finish in between.
    fn until(&mut self, what: &str, done: impl Fn(&mut Running) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            self.app.frames(1);
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn menu(&mut self, cmd: Cmd) {
        self.app
            .host()
            .send(HostEvent::Tray(TrayEvent::Menu(cmd.id())));
        self.app.frames(2);
    }

    /// The next health check (the timer runs on the host's clock).
    fn tick(&mut self) {
        self.app.host().advance(HEALTH_EVERY);
        self.app.frames(1);
    }
}

fn item(id: Cmd, label: &str, enabled: bool, default: bool) -> TrayItem {
    TrayItem::Item {
        id: id.id(),
        label: label.into(),
        enabled,
        default,
    }
}

fn status(label: &str) -> TrayItem {
    TrayItem::Item {
        id: 0,
        label: label.into(),
        enabled: false,
        default: false,
    }
}

fn check(id: Cmd, label: &str, enabled: bool, checked: bool) -> TrayItem {
    TrayItem::Check {
        id: id.id(),
        label: label.into(),
        enabled,
        checked,
    }
}

#[test]
fn the_tray_menu_is_the_models_word_for_word() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    r.app.frames(1);
    let tray = r.app.host().tray().cloned().expect("a tray icon");
    assert_eq!(tray.tooltip, "Portal — running");
    assert!(
        tray.artwork.is_some(),
        "Portal's icon at every size it is drawn at"
    );
    let port = fake.port;
    let expected = vec![
        item(Cmd::Open, "Open Portal", true, true),
        TrayItem::Submenu {
            label: "Server".into(),
            items: vec![
                status(&format!("Server: running on port {port}")),
                status("Kodi engine: running"),
                status("Portal version 0.3.10"),
                status("Video encoding: NVIDIA graphics card"),
                TrayItem::Separator,
                check(Cmd::ToggleLan, "Allow TVs on this network", true, false),
                check(
                    Cmd::ToggleEncoder,
                    "Encode video on the graphics card",
                    true,
                    true,
                ),
                item(Cmd::RestartEngine, "Restart Kodi engine", true, false),
                item(Cmd::RestartServer, "Restart Portal server", true, false),
                item(Cmd::CheckUpdates, "Check for updates", true, false),
                TrayItem::Separator,
                item(Cmd::OpenServerPage, "Open settings", true, false),
                item(Cmd::OpenLogs, "Open log folder", true, false),
            ],
        },
        check(
            Cmd::ToggleAutostart,
            "Start with Windows",
            true,
            autostart::enabled(),
        ),
        TrayItem::Separator,
        item(Cmd::Quit, "Quit Portal", true, false),
    ];
    assert_eq!(tray.menu, expected);
    // The engine's tray is the model's menu, whatever state the model is in.
    assert_eq!(tray.menu, r.model(|m| tray_items(&m.menu())));
    // The dashboard opens in the browser from the bold default.
    r.app.host().send(HostEvent::Tray(TrayEvent::Activate));
    r.app.frames(1);
    assert_eq!(
        r.app.host().opened_urls(),
        [format!("http://127.0.0.1:{port}/")]
    );
}

#[test]
fn an_ampersand_in_a_label_is_shown_not_taken_as_an_access_key() {
    let menu = vec![Entry::Submenu("Kodi & co".into(), Vec::new())];
    assert_eq!(
        tray_items(&menu),
        [TrayItem::Submenu {
            label: "Kodi && co".into(),
            items: Vec::new()
        }]
    );
}

#[test]
fn cancelling_quit_leaves_portal_running() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    r.menu(Cmd::Quit);
    let dialog = r
        .app
        .node(Role::Dialog, "Quit Portal?")
        .expect("Quit asks first");
    assert!(dialog
        .description
        .is_some_and(|d| d.contains("Anything playing or casting will stop")));
    assert!(
        r.app.node(Role::Button, "No").is_some_and(|n| n.focused),
        "No is the default, as it was"
    );
    // Tray clicks wait while a question is open.
    r.app.host().send(HostEvent::Tray(TrayEvent::Activate));
    r.app.frames(1);
    assert!(r.app.host().opened_urls().is_empty());
    assert!(r.app.press(Role::Button, "No"));
    r.app.frames(2);
    assert!(r.app.host().window_ids().is_empty(), "the question closed");
    assert!(!r.app.host().has_exited(), "Portal still runs");
    assert!(r.app.host().tray().is_some(), "and its icon stays");
    assert_eq!(r.model(|m| (m.job, m.quitting())), (None, false));
    assert!(
        !fake.requests().iter().any(|l| l.contains("/api/shutdown")),
        "the server was not asked to stop"
    );
    // Still watched: the next check reaches the server.
    let before = fake.requests().len();
    r.tick();
    r.until("the next check", |_| fake.requests().len() > before);
}

#[test]
fn quitting_stops_the_tray_after_yes() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    r.menu(Cmd::Quit);
    assert!(r.app.press(Role::Button, "Yes"));
    r.until("the app quits", |r| r.app.host().has_exited());
    assert!(r.app.host().tray().is_none(), "the icon is gone");
    assert_eq!(r.code.get(), 0);
}

#[test]
fn restart_asks_and_no_changes_nothing() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    r.menu(Cmd::RestartServer);
    let dialog = r
        .app
        .node(Role::Dialog, "Restart the Portal server?")
        .expect("asked first");
    assert_eq!(
        dialog.description.as_deref(),
        Some("Anything playing will stop.")
    );
    assert!(r.app.press(Role::Button, "No"));
    r.app.frames(1);
    assert_eq!(r.model(|m| m.job), None, "nothing restarts");
}

#[test]
fn a_second_launch_with_stop_quits_the_tray() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    r.app.host().send(HostEvent::SecondInstance(vec![
        "--stop".into(),
        "--quiet".into(),
    ]));
    r.until("the app quits", |r| r.app.host().has_exited());
    assert!(r.app.host().window_ids().is_empty(), "quiet: nothing shown");
    assert_eq!(r.code.get(), 0);
}

#[test]
fn the_watchdog_restarts_a_vanished_server_three_times_then_says_so() {
    let fake = FakePortal::start();
    let mut r = Running::start(fake.port);
    fake.go_away();
    // Each check finds nothing and starts it again; here no portal.exe is beside the test,
    // so every start fails, as a server that keeps dying would.
    let pending = |r: &mut Running| r.tray.borrow().as_ref().unwrap().0.health_pending.get();
    let mut restarts = 0;
    for _ in 0..12 {
        r.tick();
        r.until("the check is read", |r| !pending(r));
        if r.model(|m| m.job == Some(Job::Starting)) {
            restarts += 1;
            r.until("the restart finishes", |r| r.model(|m| m.job.is_none()));
        }
        if !r.app.host().notifications().is_empty() {
            break;
        }
    }
    assert_eq!(restarts, 3, "at most 3 restarts in 10 minutes");
    let notes = r.app.host().notifications();
    assert_eq!(notes.len(), 1, "told once: {notes:?}");
    assert_eq!(notes[0].title, "Portal's server keeps stopping");
    assert_eq!(notes[0].urgency, Urgency::Warning);
    // Given up: later checks do not start it again, and the menu offers to.
    r.tick();
    r.app.frames(2);
    assert_eq!(r.model(|m| m.job), None);
    let menu = r.app.host().tray().unwrap().menu.clone();
    let server = match &menu[1] {
        TrayItem::Submenu { items, .. } => items.clone(),
        other => panic!("{other:?}"),
    };
    assert!(server.contains(&status("Server: stopped")));
    assert!(server.contains(&item(Cmd::StartServer, "Start Portal server", true, false)));
    assert_eq!(r.code.get(), -1, "the tray keeps running");
}
