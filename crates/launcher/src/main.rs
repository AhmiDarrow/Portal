//! Desktop entry point and tray icon. Without arguments it starts `portal.exe`
//! in the background when it is not already running, opens the dashboard,
//! and stays in the notification area: the icon reopens the dashboard, holds
//! the server options, and quits Portal. A second launch only opens the
//! dashboard. `--stop` ends the tray and the service (the installer uses it).
//!
//! This is a GUI-subsystem program so the desktop shortcut never flashes a
//! console. Problems are shown in a message box; the service log is
//! `%LOCALAPPDATA%\Portal\logs\portal.log`.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod autostart;
mod model;
mod service;
#[cfg(windows)]
mod tray;
mod ui;

use std::time::{Duration, Instant};

use service::Probe;

/// How long `--stop` waits for the tray process to exit.
#[cfg(windows)]
const TRAY_EXIT_WAIT: Duration = Duration::from_secs(10);

const HELP: &str = "portal-launcher                 start Portal, open the dashboard, and stay in the tray\n\
                    portal-launcher --no-browser    start Portal in the tray without opening a browser\n\
                    portal-launcher --stop          quit the tray and stop Portal (--quiet: no messages)\n\
                    portal-launcher --restart-server            restart the Portal server\n\
                    portal-launcher --autostart on|off|status   start with Windows (status exits 3 when off)";

/// Exit code of `--autostart status` when Portal does not start with Windows.
const AUTOSTART_OFF: i32 = 3;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let quiet = flag("--quiet");
    let result = if flag("--help") || flag("-h") || flag("/?") {
        ui::alert(HELP);
        Ok(())
    } else if flag("--stop") {
        stop(quiet)
    } else if flag("--restart-server") {
        restart_server()
    } else if let Some(mode) = args
        .iter()
        .position(|arg| arg == "--autostart")
        .map(|at| args.get(at + 1).map(String::as_str).unwrap_or(""))
    {
        // The dashboard asks without a window to show a message in.
        match set_autostart(mode) {
            Ok(code) => std::process::exit(code),
            Err(_) => std::process::exit(1),
        }
    } else {
        launch(!flag("--no-browser"))
    };
    if let Err(message) = result {
        if !quiet {
            ui::alert(&message);
        }
        std::process::exit(1);
    }
}

/// Becomes the tray, or hands off to the one already running.
#[cfg(windows)]
fn launch(open_browser: bool) -> Result<(), String> {
    match tray::claim_instance() {
        Some(_instance) => tray::run(open_browser),
        None => open_existing(open_browser),
    }
}

#[cfg(not(windows))]
fn launch(open_browser: bool) -> Result<(), String> {
    open_existing(open_browser)
}

/// A tray already runs: wait for the service it starts and open the
/// dashboard. Starting a second service here would race the tray's own.
#[cfg(windows)]
fn open_existing(open_browser: bool) -> Result<(), String> {
    let port = service::port();
    service::await_running(port)?;
    if open_browser {
        ui::open(&service::dashboard_url(port));
    }
    Ok(())
}

/// No tray here: make sure the service answers and open the dashboard.
#[cfg(not(windows))]
fn open_existing(open_browser: bool) -> Result<(), String> {
    let port = service::port();
    service::ensure_running(port)?;
    if open_browser {
        ui::open(&service::dashboard_url(port));
    }
    Ok(())
}

/// The dashboard's "Start with Windows": turns it on or off, or answers
/// which it is through the exit code.
fn set_autostart(mode: &str) -> Result<i32, String> {
    match mode {
        "on" => autostart::set(true).map(|()| 0),
        "off" => autostart::set(false).map(|()| 0),
        "status" => Ok(if autostart::enabled() { 0 } else { AUTOSTART_OFF }),
        _ => Err("--autostart takes on, off or status.".into()),
    }
}

/// Restarts the server: through the running tray, which owns it, or directly
/// when no tray runs.
fn restart_server() -> Result<(), String> {
    #[cfg(windows)]
    if tray::request_restart() {
        return Ok(());
    }
    service::restart(service::port())
}

/// Ends the tray first, so it does not restart the service, then stops the
/// service and waits until its port is free.
fn stop(quiet: bool) -> Result<(), String> {
    let port = service::port();
    #[cfg(windows)]
    let had_tray = match tray::close_running(TRAY_EXIT_WAIT) {
        Ok(had_tray) => had_tray,
        // The service is still asked to stop, so its files come free.
        Err(err) => {
            let _ = service::shutdown(port);
            return Err(err);
        }
    };
    #[cfg(not(windows))]
    let had_tray = false;
    if had_tray {
        // A start the tray had in flight may still come up.
        let deadline = Instant::now() + Duration::from_secs(2);
        while service::probe(port) == Probe::Nothing && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let stopped = service::shutdown(port)?;
    if !had_tray && !stopped && !quiet {
        ui::alert("Portal is not running.");
    }
    Ok(())
}
