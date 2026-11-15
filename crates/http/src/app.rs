//! The tray menu's own choices, for the dashboard on this PC.
//!
//! The tray launcher owns the server process and the "Start with Windows"
//! setting, so these routes hand the work to `portal-launcher.exe` beside
//! this program: it restarts or stops the server through the running tray,
//! and reads or writes the sign-in setting. Only this PC may ask.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::Json;
use portal_api::OkBody;
use serde::{Deserialize, Serialize};

use crate::{fail, from_this_pc, paths, ApiError};

const PC_ONLY: &str = "That can only be done on the PC running Portal.";
const NO_LAUNCHER: &str = "Portal's tray program is not installed beside it.";
const LAUNCHER_FAILED: &str = "Portal's tray program did not do that.";
const LAUNCHER_LIMIT: Duration = Duration::from_secs(10);
/// The launcher's exit code for "Start with Windows is off".
const AUTOSTART_OFF: i32 = 3;

#[derive(Debug, Serialize)]
pub(crate) struct AppView {
    /// False when the tray program is missing, as in a source checkout.
    tray: bool,
    /// Whether Portal starts when the user signs in; absent when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    autostart: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AutostartBody {
    on: bool,
}

fn this_pc(peer: SocketAddr) -> Result<(), ApiError> {
    if from_this_pc(peer) {
        Ok(())
    } else {
        Err(fail(StatusCode::FORBIDDEN, PC_ONLY))
    }
}

fn launcher() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "portal-launcher.exe"
    } else {
        "portal-launcher"
    };
    let path = paths::install_dir()?.join(name);
    path.is_file().then_some(path)
}

fn require_launcher() -> Result<PathBuf, ApiError> {
    launcher().ok_or_else(|| fail(StatusCode::NOT_FOUND, NO_LAUNCHER))
}

/// Runs the launcher and waits for its exit code.
async fn ask_launcher(exe: PathBuf, args: &[&str]) -> Option<i32> {
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(LAUNCHER_LIMIT, command.status())
        .await
        .ok()?
        .ok()?;
    status.code()
}

/// Starts the launcher and does not wait: it is about to stop this server.
/// It leaves the server's job object, or it would be ended with the server.
fn send_launcher(exe: PathBuf, args: &[&str]) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        command.creation_flags(
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
        );
    }
    command.spawn().map(|_| ())
}

async fn autostart_now(exe: PathBuf) -> Option<bool> {
    match ask_launcher(exe, &["--autostart", "status"]).await? {
        0 => Some(true),
        AUTOSTART_OFF => Some(false),
        _ => None,
    }
}

async fn view() -> AppView {
    match launcher() {
        Some(exe) => AppView {
            tray: true,
            autostart: autostart_now(exe).await,
        },
        None => AppView {
            tray: false,
            autostart: None,
        },
    }
}

/// `GET /api/app`: what the dashboard's Portal menu shows beside its actions.
pub(crate) async fn app_get(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<AppView>, ApiError> {
    this_pc(peer)?;
    Ok(Json(view().await))
}

/// `POST /api/app/autostart`: turns "Start with Windows" on or off.
pub(crate) async fn app_autostart(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<AutostartBody>,
) -> Result<Json<AppView>, ApiError> {
    this_pc(peer)?;
    let exe = require_launcher()?;
    let mode = if body.on { "on" } else { "off" };
    if ask_launcher(exe, &["--autostart", mode]).await != Some(0) {
        return Err(fail(StatusCode::BAD_GATEWAY, LAUNCHER_FAILED));
    }
    Ok(Json(view().await))
}

fn sent(result: std::io::Result<()>) -> Result<Json<OkBody>, ApiError> {
    match result {
        Ok(()) => Ok(Json(OkBody { ok: true })),
        Err(err) => {
            tracing::warn!("the tray program did not start: {err}");
            Err(fail(StatusCode::BAD_GATEWAY, LAUNCHER_FAILED))
        }
    }
}

/// `POST /api/app/restart`: restarts this server, as the tray's menu does.
pub(crate) async fn app_restart(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<OkBody>, ApiError> {
    this_pc(peer)?;
    sent(send_launcher(require_launcher()?, &["--restart-server"]))
}

/// `POST /api/app/quit`: closes the tray and stops this server.
pub(crate) async fn app_quit(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<OkBody>, ApiError> {
    this_pc(peer)?;
    sent(send_launcher(require_launcher()?, &["--stop", "--quiet"]))
}

/// `POST /api/app/logs`: opens the log folder in the file manager.
pub(crate) async fn app_logs(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<OkBody>, ApiError> {
    this_pc(peer)?;
    let dir = paths::portal_dir().join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let opener = if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    let opened = std::process::Command::new(opener)
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ());
    match opened {
        Ok(()) => Ok(Json(OkBody { ok: true })),
        Err(_) => Err(fail(
            StatusCode::BAD_GATEWAY,
            "The log folder could not be opened.",
        )),
    }
}
