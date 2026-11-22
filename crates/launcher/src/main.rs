//! Desktop entry point and tray icon. Without arguments it starts `portal.exe` in the
//! background when it is not already running, opens the dashboard, and stays in the
//! notification area: the icon reopens the dashboard, holds the server options, and quits
//! Portal. A second launch hands its arguments to the running tray (only opening the
//! dashboard, unless it asks for more) and exits with the tray's answer. `--stop` ends the
//! tray and the service (the installer uses it, and waits for the exit code).
//!
//! On Windows this is a GUI-subsystem program so the desktop shortcut never flashes a
//! console. Problems are shown in Portal's own dialog; the service log is
//! `%LOCALAPPDATA%\Portal\logs\portal.log`. Linux runs the same tray (StatusNotifierItem,
//! or the X11 tray), with `portal` beside the launcher.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod autostart;
mod model;
mod service;
mod tray;
mod ui;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const HELP: &str = "portal-launcher                 start Portal, open the dashboard, and stay in the tray\n\
                    portal-launcher --no-browser    start Portal in the tray without opening a browser\n\
                    portal-launcher --stop          quit the tray and stop Portal (--quiet: no messages)\n\
                    portal-launcher --restart-server            restart the Portal server\n\
                    portal-launcher --autostart on|off|status   start with Windows (status exits 3 when off)";

/// Exit code of `--autostart status` when Portal does not start with Windows.
const AUTOSTART_OFF: i32 = 3;

/// How long a second launch waits for the running tray to answer. `--stop` is answered once
/// the service stopped: a start in flight finishes first (up to 45 s), then the stop (20 s).
pub const REPLY_WAIT: Duration = Duration::from_secs(90);
/// How long `--stop` then waits for the tray's process to exit.
pub const TRAY_EXIT_WAIT: Duration = Duration::from_secs(10);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let quiet = flag("--quiet");
    if flag("--help") || flag("-h") || flag("/?") {
        ui::alert_alone(HELP);
        return;
    }
    let port = service::port();
    let request = if flag("--stop") {
        Request::Stop { quiet }
    } else if flag("--restart-server") {
        Request::RestartServer { quiet }
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
        Request::Tray {
            open_browser: !flag("--no-browser"),
        }
    };
    let code = run(port, request);
    std::process::exit(code);
}

/// What this launch is for when it is the only one running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// Become the tray.
    Tray { open_browser: bool },
    /// Stop the service: there is no tray to ask.
    Stop { quiet: bool },
    /// Restart the service: there is no tray to ask.
    RestartServer { quiet: bool },
}

/// One Portal tray per port: the default port's is "Portal", another port's (a second
/// install, a test) its own.
pub fn instance_name(port: u16) -> String {
    if port == service::DEFAULT_PORT {
        ui::NAME.to_string()
    } else {
        format!("{} {port}", ui::NAME)
    }
}

/// Runs this launch: as the tray, or, when one already runs, by handing it the arguments
/// and exiting with its answer (`remedy_ui::run` exits with it). Returns the exit code.
fn run(port: u16, request: Request) -> i32 {
    let mut config = ui::config().single_instance_reply(REPLY_WAIT);
    config.name = instance_name(port);
    if matches!(request, Request::Stop { .. }) {
        // The installer replaces the files once this returns.
        config = config.single_instance_await_exit(TRAY_EXIT_WAIT);
    }
    let code = Rc::new(Cell::new(0));
    let c2 = code.clone();
    let ran = remedy_ui::run(config, move |app| match request {
        Request::Tray { open_browser } => {
            tray::Tray::start(app, port, open_browser, c2);
        }
        Request::Stop { quiet } => alone(app, quiet, c2, move || stop(port)),
        Request::RestartServer { quiet } => alone(app, quiet, c2, move || {
            service::restart(port).map(|()| None)
        }),
    });
    match ran {
        Ok(()) => code.get(),
        // The running tray did not answer in time, or could not be reached.
        Err(_) => 1,
    }
}

/// A launch with no tray to ask: `work` runs in the background; its message (`Ok(Some)`)
/// or error is shown unless `quiet`, and the app ends with 0, or 1 after an error.
fn alone(
    app: &remedy_ui::App,
    quiet: bool,
    code: Rc<Cell<i32>>,
    work: impl FnOnce() -> Result<Option<String>, String> + Send + 'static,
) {
    let a = app.clone();
    app.background(work, move |result| {
        code.set(i32::from(result.is_err()));
        let message = match result {
            Ok(message) => message,
            Err(message) => Some(message),
        };
        match message.filter(|_| !quiet) {
            Some(message) => {
                let a2 = a.clone();
                a.confirm(ui::notice(&message), move |_| a2.quit());
            }
            None => a.quit(),
        }
    });
}

/// `--stop` with no tray running: stops the service and waits until its port is free.
fn stop(port: u16) -> Result<Option<String>, String> {
    let stopped = service::shutdown(port)?;
    Ok((!stopped).then(|| "Portal is not running.".to_string()))
}

/// The dashboard's "Start with Windows": turns it on or off, or answers which it is through
/// the exit code.
fn set_autostart(mode: &str) -> Result<i32, String> {
    match mode {
        "on" => autostart::set(true).map(|()| 0),
        "off" => autostart::set(false).map(|()| 0),
        "status" => Ok(if autostart::enabled() {
            0
        } else {
            AUTOSTART_OFF
        }),
        _ => Err("--autostart takes on, off or status.".into()),
    }
}
