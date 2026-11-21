//! The tray menu's own choices, for the dashboard on this PC.
//!
//! The tray launcher owns the server process and the "Start with Windows"
//! setting, so these routes hand the work to `portal-launcher.exe` beside
//! this program: it restarts or stops the server through the running tray,
//! and reads or writes the sign-in setting. Only this PC may ask.
//!
//! Whether a graphics card encodes the video is this server's own setting,
//! kept in `encoder.json` beside the library.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::Json;
use portal_api::OkBody;
use serde::{Deserialize, Serialize};

use crate::{fail, from_this_pc, paths, ApiError, AppState};

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
    encoder: EncoderView,
}

/// What makes the picture when a video has to be encoded.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct EncoderView {
    /// Whether a graphics card may be used, as the user last chose.
    hardware: bool,
    /// The card this PC has for it, or "cpu" when it has none that works.
    card: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EncoderBody {
    on: bool,
}

const ENCODER_FILE: &str = "encoder.json";

/// The saved choice; a graphics card is used unless it was turned off.
pub(crate) fn hardware_saved(dir: &std::path::Path) -> bool {
    std::fs::read_to_string(dir.join(ENCODER_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| value.get("hardware").and_then(|item| item.as_bool()))
        .unwrap_or(true)
}

fn save_hardware(dir: &std::path::Path, on: bool) -> std::io::Result<()> {
    crate::paths::write_settings(
        &dir.join(ENCODER_FILE),
        format!("{}\n", serde_json::json!({ "hardware": on })).as_bytes(),
    )
}

async fn encoder_view(state: &AppState) -> EncoderView {
    // The first look runs in a task of its own, so a caller that gives up
    // early (the tray waits only a few seconds) does not cut it short and
    // start it over on the next ask.
    let player = state.0.player.clone();
    let card = tokio::spawn(async move { player.hardware().await.name() })
        .await
        .unwrap_or("cpu");
    EncoderView {
        hardware: state.0.player.hardware_wanted(),
        card,
    }
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

async fn view(state: &AppState) -> AppView {
    let encoder = encoder_view(state).await;
    match launcher() {
        Some(exe) => AppView {
            tray: true,
            autostart: autostart_now(exe).await,
            encoder,
        },
        None => AppView {
            tray: false,
            autostart: None,
            encoder,
        },
    }
}

/// `GET /api/app`: what the dashboard's Portal menu shows beside its actions.
pub(crate) async fn app_get(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<AppView>, ApiError> {
    this_pc(peer)?;
    Ok(Json(view(&state).await))
}

/// `GET /api/app/encoder`: just the encoder choice, for the tray to read
/// every few seconds without asking the launcher about "Start with Windows".
pub(crate) async fn app_encoder_get(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<EncoderView>, ApiError> {
    this_pc(peer)?;
    Ok(Json(encoder_view(&state).await))
}

/// `POST /api/app/encoder`: lets a graphics card encode the video, or keeps
/// it on the processor. It takes effect with the next video started.
pub(crate) async fn app_encoder(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<EncoderBody>,
) -> Result<Json<AppView>, ApiError> {
    this_pc(peer)?;
    if save_hardware(&paths::portal_dir(), body.on).is_err() {
        return Err(fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "That choice could not be saved.",
        ));
    }
    state.0.player.set_hardware(body.on);
    Ok(Json(view(&state).await))
}

/// `POST /api/app/autostart`: turns "Start with Windows" on or off.
pub(crate) async fn app_autostart(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<AutostartBody>,
) -> Result<Json<AppView>, ApiError> {
    this_pc(peer)?;
    let exe = require_launcher()?;
    let mode = if body.on { "on" } else { "off" };
    if ask_launcher(exe, &["--autostart", mode]).await != Some(0) {
        return Err(fail(StatusCode::BAD_GATEWAY, LAUNCHER_FAILED));
    }
    Ok(Json(view(&state).await))
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

#[cfg(test)]
#[path = "../../../tests-private/http/src/app.rs"]
mod tests;
