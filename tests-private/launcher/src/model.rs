use super::*;

fn running() -> Health {
    Health {
        probe: Probe::Portal,
        pid: Some(4242),
        engine: Some("running".into()),
        lan: Some(true),
        encoder: Some(EncoderChoice {
            hardware: true,
            card: "nvidia".into(),
        }),
        update: Some(update(false, None, "idle")),
    }
}

fn gone() -> Health {
    Health {
        probe: Probe::Nothing,
        pid: None,
        engine: None,
        lan: None,
        encoder: None,
        update: None,
    }
}

fn update(available: bool, latest: Option<&str>, state: &str) -> Update {
    Update {
        current: "0.1.0".into(),
        latest: latest.map(str::to_string),
        available,
        state: state.into(),
        progress: None,
        error: None,
    }
}

fn items(entries: &[Entry]) -> Vec<Item> {
    let mut out = Vec::new();
    for entry in entries {
        match entry {
            Entry::Item(item) => out.push(item.clone()),
            Entry::Submenu(_, inner) => out.extend(items(inner)),
            Entry::Separator => {}
        }
    }
    out
}

fn find(model: &Model, cmd: Cmd) -> Option<Item> {
    items(&model.menu())
        .into_iter()
        .find(|item| item.cmd == Some(cmd))
}

#[test]
fn command_ids_round_trip() {
    for cmd in Cmd::ALL {
        assert_ne!(cmd.id(), 0, "0 means no choice to TrackPopupMenu");
        assert_eq!(Cmd::from_id(cmd.id()), Some(cmd));
    }
    assert_eq!(Cmd::from_id(0), None);
    assert_eq!(Cmd::from_id(999), None);
}

#[test]
fn budget_allows_three_restarts_in_ten_minutes() {
    let start = Instant::now();
    let mut budget = RestartBudget::new(3, Duration::from_secs(600));
    assert!(budget.take(start));
    assert!(budget.take(start + Duration::from_secs(10)));
    assert!(budget.take(start + Duration::from_secs(20)));
    assert!(!budget.take(start + Duration::from_secs(30)));
    // The first restart ages out of the window.
    assert!(budget.take(start + Duration::from_secs(600)));
    assert!(!budget.take(start + Duration::from_secs(601)));
    budget.reset();
    assert!(budget.take(start + Duration::from_secs(602)));
}

#[test]
fn a_vanished_service_is_restarted_then_given_up_on() {
    let now = Instant::now();
    let mut model = Model::new(8490);
    assert!(model.on_health(running(), now).is_empty());
    for _ in 0..3 {
        assert_eq!(model.on_health(gone(), now), vec![Reaction::AutoRestart]);
        model.job = Some(Job::Starting);
        assert!(
            model.on_health(gone(), now).is_empty(),
            "a start is running"
        );
        model.started(false, false);
    }
    assert_eq!(model.on_health(gone(), now), vec![Reaction::GaveUp]);
    assert!(
        model.on_health(gone(), now).is_empty(),
        "the user is told once"
    );
    assert_eq!(model.tooltip(), "Portal — stopped");
    let start = find(&model, Cmd::StartServer).unwrap();
    assert!(start.enabled);
    assert!(find(&model, Cmd::RestartServer).is_none());

    // Starting it by hand refills the budget.
    model.started(true, true);
    assert_eq!(model.on_health(gone(), now), vec![Reaction::AutoRestart]);
}

#[test]
fn a_slow_service_is_not_restarted() {
    let now = Instant::now();
    let mut model = Model::new(8490);
    assert!(model.on_health(running(), now).is_empty());
    let silent = Health {
        probe: Probe::Silent,
        ..gone()
    };
    for _ in 0..5 {
        assert!(model.on_health(silent.clone(), now).is_empty());
    }
    assert_eq!(model.service, Service::Running);
    assert_eq!(model.engine.as_deref(), Some("running"));
}

fn silent() -> Health {
    Health {
        probe: Probe::Silent,
        ..gone()
    }
}

/// A model whose server has run past its startup grace by `at`.
fn settled() -> (Model, Instant) {
    let mut model = Model::new(8490);
    model.started(true, false);
    let at = Instant::now() + Duration::from_secs(10 * 60);
    assert!(model.on_health(running(), at).is_empty());
    (model, at)
}

/// Silent checks 5 s apart from `from`; the reactions of the last one.
fn silent_for(model: &mut Model, from: Instant, checks: u32) -> Vec<Reaction> {
    let mut last = Vec::new();
    for check in 1..=checks {
        let at = from + Duration::from_secs(5 * u64::from(check));
        last = model.on_health(silent(), at);
        if check < checks {
            assert!(last.is_empty(), "check {check} is too early");
        }
    }
    last
}

#[test]
fn a_hung_service_is_restarted_after_the_misses() {
    let (mut model, at) = settled();
    let reactions = silent_for(&mut model, at, 10);
    assert_eq!(
        reactions,
        vec![Reaction::RestartHung {
            pid: Some(4242),
            misses: 10,
            silent: Duration::from_secs(45),
        }]
    );
}

#[test]
fn a_service_that_never_answered_is_restarted_by_its_port() {
    let (mut model, at) = settled();
    let no_pid = Health {
        pid: None,
        ..running()
    };
    model.on_health(no_pid, at);
    assert!(matches!(
        silent_for(&mut model, at, 10).as_slice(),
        [Reaction::RestartHung { pid: None, .. }]
    ));
}

/// The tray's model, driven by real health checks every 5 s, against a
/// debug `portal.exe` on a throwaway port that freezes after a few seconds.
/// Data, logs and Kodi are moved aside, so nothing of the installed Portal
/// is touched. Build it first, then run alone:
/// `cargo build -p portal-http && cargo test -p portal-launcher frozen -- --ignored`
/// Ends a test server left running by a failed assertion.
struct EndOnPanic(u32);

impl Drop for EndOnPanic {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/PID", &self.0.to_string()])
                .status();
        }
    }
}

#[test]
#[ignore = "needs a debug portal.exe; takes about a minute"]
fn a_frozen_portal_is_restarted_end_to_end() {
    use crate::service;
    let deps = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let built = deps
        .ancestors()
        .map(|dir| dir.join("portal.exe"))
        .find(|exe| exe.is_file())
        .expect("run cargo build -p portal-http first");
    // The launcher starts the portal.exe beside itself.
    let _ = std::fs::remove_file(deps.join("portal.exe"));
    std::fs::copy(&built, deps.join("portal.exe")).unwrap();
    let free = || {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    };
    let port = free();
    assert_ne!(port, 8490);
    let scratch = std::env::temp_dir().join(format!("portal-frozen-{port}"));
    std::fs::create_dir_all(&scratch).unwrap();
    // Only this test runs in the process (it is run alone), so these do not
    // reach another test.
    std::env::set_var("PORTAL_PORT", port.to_string());
    std::env::set_var("LOCALAPPDATA", &scratch);
    std::env::set_var("APPDATA", &scratch);
    std::env::set_var("KODI_EXE", scratch.join("no-kodi.exe"));
    std::env::set_var("PORTAL_TEST_KODI_PORT", free().to_string());
    std::env::set_var("PORTAL_NO_UPDATE", "1");
    std::env::set_var("PORTAL_TEST_FREEZE_AFTER", "5");

    let mut model = Model::new(port);
    model.job = Some(Job::Starting);
    service::ensure_running(port).unwrap();
    model.started(true, false);
    // Past the startup grace, so the 45 s limit applies.
    model.started_at = Instant::now().checked_sub(STARTUP_GRACE);
    let first = service::health(port);
    assert_eq!(first.probe, Probe::Portal);
    let frozen_pid = first.pid.unwrap();
    let _cleanup = EndOnPanic(frozen_pid);
    model.on_health(first, Instant::now());
    // With Kodi's port moved, a Kodi already running on this PC reads as
    // "starting", which would stretch the limit to 3 minutes.
    model.engine = None;

    let began = Instant::now();
    let reaction = loop {
        assert!(
            began.elapsed() < Duration::from_secs(120),
            "never restarted"
        );
        std::thread::sleep(Duration::from_secs(5));
        let health = service::health(port);
        let mut reactions = model.on_health(health, Instant::now());
        if let Some(reaction) = reactions.pop() {
            break reaction;
        }
    };
    let Reaction::RestartHung {
        pid,
        misses,
        silent,
    } = reaction
    else {
        panic!("{reaction:?}");
    };
    assert_eq!(pid, Some(frozen_pid));
    assert!(misses >= 6 && silent >= Duration::from_secs(45));
    eprintln!("hung after {misses} checks over {silent:?}");

    // The new one answers for good, so it can be stopped cleanly.
    std::env::remove_var("PORTAL_TEST_FREEZE_AFTER");
    model.job = Some(Job::Restarting);
    let restarted = service::restart_hung(port, pid);
    model.started(restarted.is_ok(), false);
    restarted.unwrap();
    let after = service::health(port);
    assert_eq!(after.probe, Probe::Portal);
    assert_ne!(after.pid, Some(frozen_pid));
    let _cleanup_new = EndOnPanic(after.pid.unwrap());
    assert!(model.on_health(after, Instant::now()).is_empty());
    assert_eq!(service::shutdown(port), Ok(true));
    let _ = std::fs::remove_file(deps.join("portal.exe"));
    let _ = std::fs::remove_dir_all(&scratch);
}

#[test]
fn a_slow_but_answering_service_is_never_restarted() {
    let (mut model, mut at) = settled();
    for _ in 0..50 {
        // Five slow checks, then it answers: the count starts over.
        assert!(silent_for(&mut model, at, 5).is_empty());
        at += Duration::from_secs(30);
        assert!(model.on_health(running(), at).is_empty());
    }
    assert_eq!(model.service, Service::Running);
}

#[test]
fn a_long_silence_with_few_checks_is_not_hung() {
    // The PC slept: two checks an hour apart.
    let (mut model, at) = settled();
    assert!(model.on_health(silent(), at).is_empty());
    assert!(model
        .on_health(silent(), at + Duration::from_secs(3600))
        .is_empty());
}

#[test]
fn starting_and_long_work_get_a_longer_grace() {
    // Just started: silent from the start, it gets about 3 minutes.
    let mut model = Model::new(8490);
    model.started(true, false);
    let now = Instant::now();
    assert!(model.on_health(running(), now).is_empty());
    let restarted_at = (1..=60u64)
        .find(|check| {
            !model
                .on_health(silent(), now + Duration::from_secs(5 * check))
                .is_empty()
        })
        .unwrap();
    assert!((35..=38).contains(&restarted_at), "{restarted_at}");

    // Settled, but a download is running.
    let (mut model, at) = settled();
    let mut busy = running();
    busy.update = Some(update(true, Some("9.9.9"), "downloading"));
    model.on_health(busy, at);
    assert!(silent_for(&mut model, at, 20).is_empty());

    // An install the user started: no restart at all.
    let (mut model, at) = settled();
    model.updating_since = Some(at);
    assert!(silent_for(&mut model, at, 50).is_empty());
}

#[test]
fn hung_restarts_share_the_restart_budget() {
    let (mut model, mut at) = settled();
    for _ in 0..3 {
        assert!(matches!(
            silent_for(&mut model, at, 10).as_slice(),
            [Reaction::RestartHung {
                pid: Some(4242),
                ..
            }]
        ));
        model.job = Some(Job::Restarting);
        model.started(false, false);
        // It came back, then hung again.
        model.service = Service::Running;
        at += Duration::from_secs(60);
    }
    assert_eq!(silent_for(&mut model, at, 10), vec![Reaction::GaveUp]);
    assert!(silent_for(&mut model, at + Duration::from_secs(60), 20).is_empty());

    // A vanished service spends from the same budget.
    let (mut model, at) = settled();
    for _ in 0..3 {
        assert_eq!(model.on_health(gone(), at), vec![Reaction::AutoRestart]);
        model.started(true, false);
        model.on_health(running(), at);
    }
    assert_eq!(silent_for(&mut model, at, 10), vec![Reaction::GaveUp]);
}

#[test]
fn quit_during_a_start_waits_for_it_then_stops() {
    let mut model = Model::new(8490);
    model.job = Some(Job::Starting);
    assert!(!model.quit(), "the start is still running");
    assert!(model.quitting());
    assert!(!model.quit(), "asking twice changes nothing");
    assert!(model.started(true, false), "the start hands over");
    assert_eq!(model.job, Some(Job::Quitting));
    assert!(model.quitting());

    let mut failed = Model::new(8490);
    failed.job = Some(Job::Restarting);
    assert!(!failed.quit());
    assert!(failed.started(false, true));
    assert_eq!(failed.job, Some(Job::Quitting));

    let mut idle = Model::new(8490);
    assert!(!idle.quitting());
    assert!(idle.quit(), "nothing in flight: stop now");
    assert_eq!(idle.job, Some(Job::Quitting));
    assert!(!idle.quit());

    let mut plain = Model::new(8490);
    plain.job = Some(Job::Starting);
    assert!(!plain.started(true, false));
    assert_eq!(plain.job, None);
}

#[test]
fn another_program_on_the_port_is_not_fought() {
    let mut model = Model::new(8490);
    let other = Health {
        probe: Probe::Other,
        ..gone()
    };
    assert!(model.on_health(other, Instant::now()).is_empty());
    assert_eq!(model.service, Service::PortTaken);
    assert!(!find(&model, Cmd::Open).unwrap().enabled);
    assert_eq!(model.tooltip(), "Portal — port 8490 is in use");
}

#[test]
fn encoder_choice_matches_the_dashboard() {
    let mut model = Model::new(8490);
    assert!(
        !find(&model, Cmd::ToggleEncoder).unwrap().enabled,
        "state unknown yet"
    );
    let status = |model: &Model| {
        model
            .menu()
            .into_iter()
            .find_map(|entry| match entry {
                Entry::Submenu(_, items) => Some(items),
                _ => None,
            })
            .unwrap()
            .into_iter()
            .find_map(|entry| match entry {
                Entry::Item(item) if item.label.starts_with("Video encoding") => Some(item.label),
                _ => None,
            })
    };

    model.on_health(running(), Instant::now());
    let item = find(&model, Cmd::ToggleEncoder).unwrap();
    assert!(item.enabled && item.checked);
    assert_eq!(
        status(&model).unwrap(),
        "Video encoding: NVIDIA graphics card"
    );

    let mut off = running();
    off.encoder = Some(EncoderChoice {
        hardware: false,
        card: "intel".into(),
    });
    model.on_health(off, Instant::now());
    let item = find(&model, Cmd::ToggleEncoder).unwrap();
    assert!(item.enabled && !item.checked);
    assert_eq!(status(&model).unwrap(), "Video encoding: processor");

    let mut none = running();
    none.encoder = Some(EncoderChoice {
        hardware: true,
        card: "cpu".into(),
    });
    model.on_health(none, Instant::now());
    let item = find(&model, Cmd::ToggleEncoder).unwrap();
    assert!(!item.enabled && !item.checked, "no card to choose");
    assert_eq!(
        status(&model).unwrap(),
        "Video encoding: processor (no graphics card found for it)"
    );
}

#[test]
fn menu_follows_the_service() {
    let mut model = Model::new(8490);
    let open = find(&model, Cmd::Open).unwrap();
    assert!(open.default && open.enabled);
    assert!(
        !find(&model, Cmd::ToggleLan).unwrap().enabled,
        "state unknown yet"
    );
    assert!(find(&model, Cmd::Quit).unwrap().enabled);

    model.on_health(running(), Instant::now());
    model.autostart = true;
    let lan = find(&model, Cmd::ToggleLan).unwrap();
    assert!(lan.enabled && lan.checked);
    assert!(find(&model, Cmd::RestartEngine).unwrap().enabled);
    assert!(find(&model, Cmd::RestartServer).unwrap().enabled);
    assert!(find(&model, Cmd::CheckUpdates).unwrap().enabled);
    assert!(find(&model, Cmd::ToggleAutostart).unwrap().checked);
    assert!(find(&model, Cmd::InstallUpdate).is_none());
    let labels: Vec<String> = items(&model.menu()).into_iter().map(|i| i.label).collect();
    assert!(labels.contains(&"Server: running on port 8490".to_string()));
    assert!(labels.contains(&"Kodi engine: running".to_string()));
    assert_eq!(model.tooltip(), "Portal — running");

    model.job = Some(Job::Restarting);
    assert!(!find(&model, Cmd::ToggleLan).unwrap().enabled);
    assert!(!find(&model, Cmd::RestartServer).unwrap().enabled);
    assert_eq!(model.tooltip(), "Portal — restarting…");

    model.job = Some(Job::Quitting);
    assert!(!find(&model, Cmd::Quit).unwrap().enabled);
}

#[test]
fn updates_are_announced_once_and_offered() {
    let now = Instant::now();
    let mut model = Model::new(8490);
    let mut health = running();
    health.update = Some(update(true, Some("0.2.0"), "ready"));
    assert_eq!(
        model.on_health(health.clone(), now),
        vec![Reaction::UpdateAvailable("0.2.0".into())]
    );
    assert!(model.on_health(health.clone(), now).is_empty());
    let install = find(&model, Cmd::InstallUpdate).unwrap();
    assert_eq!(install.label, "Install update 0.2.0…");
    assert!(install.enabled);
    assert_eq!(model.tooltip(), "Portal — running (update available)");

    // Installing: no offer, a progress line instead, and no restarts.
    model.updating_since = Some(now);
    let mut downloading = update(true, Some("0.2.0"), "downloading");
    downloading.progress = Some(0.42);
    health.update = Some(downloading);
    model.on_health(health.clone(), now);
    assert!(find(&model, Cmd::InstallUpdate).is_none());
    let labels: Vec<String> = items(&model.menu()).into_iter().map(|i| i.label).collect();
    assert!(labels.contains(&"Updating Portal… 42%".to_string()));
    assert_eq!(model.tooltip(), "Portal — updating…");
    assert!(
        model.on_health(gone(), now).is_empty(),
        "the installer stops it"
    );
    assert!(model
        .on_health(gone(), now + UPDATE_GRACE)
        .contains(&Reaction::AutoRestart));
}

#[test]
fn an_install_that_went_nowhere_stops_saying_updating() {
    let now = Instant::now();
    let mut model = Model::new(8490);
    model.updating_since = Some(now);
    let mut health = running();
    health.update = Some(update(true, Some("0.2.0"), "idle"));
    model.on_health(health.clone(), now + Duration::from_secs(5));
    assert!(
        model.updating_since.is_some(),
        "a moment of idle mid-install"
    );
    model.on_health(health, now + UPDATE_SETTLE);
    assert_eq!(model.updating_since, None);
    assert_eq!(model.tooltip(), "Portal — running (update available)");
    assert!(find(&model, Cmd::RestartServer).unwrap().enabled);
}

#[test]
fn a_failed_install_is_reported_once() {
    let now = Instant::now();
    let mut model = Model::new(8490);
    let mut health = running();
    let mut failed = update(true, Some("0.2.0"), "failed");
    failed.error = Some("The download did not match its checksum.".into());
    health.update = Some(failed);
    model.announced("0.2.0");
    assert!(
        model.on_health(health.clone(), now).is_empty(),
        "background failures stay quiet"
    );
    model.updating_since = Some(now);
    assert_eq!(
        model.on_health(health.clone(), now),
        vec![Reaction::UpdateFailed(
            "The download did not match its checksum.".into()
        )]
    );
    assert_eq!(model.updating_since, None);
    model.updating_since = Some(now);
    assert!(
        model.on_health(health, now).is_empty(),
        "same error, told once"
    );
}

#[test]
fn wide_text_is_cut_to_fit() {
    let mut buffer = [7u16; 6];
    fill_wide(&mut buffer, "Portal");
    assert_eq!(String::from_utf16_lossy(&buffer[..5]), "Porta");
    assert_eq!(buffer[5], 0);
    fill_wide(&mut buffer, "ab");
    assert_eq!(&buffer[..3], &[b'a' as u16, b'b' as u16, 0]);
    assert!(buffer[3..].iter().all(|unit| *unit == 0));
    let mut pair = [0u16; 3];
    fill_wide(&mut pair, "a😀");
    assert_eq!(pair, [b'a' as u16, 0, 0]);
    fill_wide(&mut [], "nothing");
}
