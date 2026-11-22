//! Owns at most the Kodi this process started. An already-open port is attached
//! and left alone: no second process, no hide, no quit, no settings rewrite.
//! A new process is created only when a snapshot shows that no `kodi.exe` is
//! running, and only one Portal can pass that check at a time.

use crate::error::EngineError;
use crate::rpc::KodiClient;
use portal_api::{EngineState, Status};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::*;

const PORT_WAIT: Duration = Duration::from_secs(30);
/// Kodi saves its settings and add-on data while it quits; add-on services
/// can make that slow. Only after this long is it ended by force.
const QUIT_WAIT: Duration = Duration::from_secs(30);
const RPC_LIMIT: Duration = Duration::from_secs(5);
/// Status must answer while a plugin GetDirectory may be blocking Kodi.
const STATUS_RPC: Duration = Duration::from_millis(900);
const IMAGE_CACHE_CAP: usize = 64;
const IMAGE_CACHE_BYTES: usize = 32 * 1024 * 1024;

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x00000008;
#[cfg(windows)]
const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x01000000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartChoice {
    Ensure,
    LeaveForeign,
    QuitThenEnsure,
}

/// Stopped starts Kodi. A foreign process is refused. A running process we
/// started is quit, then started again. One of ours still coming up is waited
/// on by ensure, which replaces it if its port never opens.
pub fn restart_plan(state: EngineState, started_by_us: bool) -> RestartChoice {
    if state == EngineState::Stopped {
        return RestartChoice::Ensure;
    }
    if !started_by_us {
        return RestartChoice::LeaveForeign;
    }
    if state == EngineState::Running {
        return RestartChoice::QuitThenEnsure;
    }
    RestartChoice::Ensure
}

/// A Kodi this process did not start is not replaced after it exits.
pub fn spawn_allowed(attached_foreign: bool) -> bool {
    !attached_foreign
}

/// One Kodi can be ours. Two or more is ambiguous, so none of them is.
pub fn attach_decision(pids: &[u32], owned: Option<u32>) -> (Option<u32>, bool) {
    if pids.len() != 1 {
        return (None, false);
    }
    let pid = pids[0];
    let ours = owned == Some(pid);
    (Some(pid), ours)
}

/// Art Portal may ask Kodi for: a picture on the web, or a file inside
/// Kodi's own folders (`roots`). Kodi would fetch anything an `image://`
/// names, so local files elsewhere and addresses on this PC or the home
/// network are refused.
pub fn image_path_allowed(path: &str, roots: &[PathBuf]) -> bool {
    if path.contains(['\n', '\r']) || path.len() > 4096 {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("special://") {
        return !rest.contains("..")
            && ["home/addons/", "xbmc/addons/", "profile/thumbnails/", "masterprofile/thumbnails/"]
                .iter()
                .any(|prefix| rest.starts_with(prefix));
    }
    let Some(inner) = path
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("image://"))
        .map(|_| &path[8..])
    else {
        return false;
    };
    // Kodi writes `image://<percent-encoded address>/`, sometimes with options after `@`.
    let inner = inner.split('@').next().unwrap_or(inner).trim_end_matches('/');
    let inner = percent_decode(inner);
    let lower_inner = inner.to_ascii_lowercase();
    if lower_inner.starts_with("http://") || lower_inner.starts_with("https://") {
        return public_web_host(&inner);
    }
    if lower_inner.starts_with("special://") {
        return image_path_allowed(&inner, roots);
    }
    local_under(&inner, roots)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |byte: u8| (byte as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// An http(s) address whose host is a public name or address: not this PC,
/// not the home network, and with no login in it.
pub fn public_web_host(url: &str) -> bool {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(inside) = authority.strip_prefix('[') {
        inside.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let address = host.parse::<std::net::IpAddr>();
    // A bare name such as `mypc` is resolved on the home network.
    if host.is_empty()
        || host == "localhost"
        || host.ends_with(".localhost")
        || (!host.contains('.') && address.is_err())
    {
        return false;
    }
    match address {
        Ok(ip) => public_ip(ip),
        // A resolver reads "127.1", "0x7f.1" or "0177.0.0.1" as addresses.
        Err(_) => !host.split('.').all(|label| {
            let hex = label
                .strip_prefix("0x")
                .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_hexdigit()));
            hex || (!label.is_empty() && label.bytes().all(|byte| byte.is_ascii_digit()))
        }),
    }
}

/// An address on the internet: not this PC, the home network, or a range
/// that is not one machine.
pub fn public_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let shared = ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]);
            !(ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.octets()[0] == 0
                || shared)
        }
        std::net::IpAddr::V6(ip) => {
            let first = ip.segments()[0];
            match ip.to_ipv4_mapped() {
                Some(v4) => public_ip(std::net::IpAddr::V4(v4)),
                None => !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || (first & 0xfe00) == 0xfc00
                    || (first & 0xffc0) == 0xfe80),
            }
        }
    }
}

/// The host and port of web art: `image://<encoded http(s) address>/`.
fn web_art_host(path: &str) -> Option<(String, u16)> {
    let inner = path
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("image://"))
        .map(|_| &path[8..])?;
    let inner = inner.split('@').next().unwrap_or(inner).trim_end_matches('/');
    let url = percent_decode(inner);
    let lower = url.to_ascii_lowercase();
    let (rest, default_port) = if lower.starts_with("https://") {
        (&url[8..], 443)
    } else if lower.starts_with("http://") {
        (&url[7..], 80)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let (host, after) = inside.split_once(']')?;
        (host, after.strip_prefix(':'))
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = match port {
        Some(text) => text.parse().ok()?,
        None => default_port,
    };
    Some((host.trim_end_matches('.').to_string(), port))
}

/// Every address a web art host resolves to is public. Kodi fetches what
/// the name points at, and a public-looking name can point inside the home.
#[cfg(not(test))]
async fn resolves_public(host: &str, port: u16) -> bool {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return true;
    }
    let Ok(Ok(addresses)) = timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host.to_string(), port)),
    )
    .await
    else {
        return false;
    };
    let mut any = false;
    for address in addresses {
        if !public_ip(address.ip()) {
            return false;
        }
        any = true;
    }
    any
}

/// Tests never reach the network.
#[cfg(test)]
async fn resolves_public(_host: &str, _port: u16) -> bool {
    true
}

/// A local file inside one of Kodi's folders, with no way out of it.
fn local_under(path: &str, roots: &[PathBuf]) -> bool {
    let normalized = path.replace('/', "\\");
    if normalized.starts_with("\\\\") || normalized.split('\\').any(|part| part == "..") {
        return false;
    }
    let lower = normalized.to_ascii_lowercase();
    roots.iter().any(|root| {
        let root = root.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
        let root = root.trim_end_matches('\\');
        !root.is_empty() && lower.starts_with(&format!("{root}\\"))
    })
}

#[derive(Debug, Clone)]
pub struct ProcessConfig {
    pub host: String,
    pub port: u16,
    pub kodi_exe: PathBuf,
    pub profile: PathBuf,
    pub state_path: PathBuf,
}

pub struct Supervisor {
    config: ProcessConfig,
    browse_client: KodiClient,
    dialogs_client: KodiClient,
    player_client: KodiClient,
    /// Status probes only, so a play job's long call never holds up status.
    status_client: KodiClient,
    op: tokio::sync::Mutex<()>,
    saw_running: AtomicBool,
    /// Pid of a Kodi that was already running. Zero when none has been attached.
    foreign_pid: AtomicU32,
    image_state: Mutex<ImageState>,
}

impl Supervisor {
    pub fn start(config: ProcessConfig) -> Self {
        Self {
            browse_client: KodiClient::new(config.host.clone(), config.port),
            dialogs_client: KodiClient::new(config.host.clone(), config.port),
            player_client: KodiClient::new(config.host.clone(), config.port),
            status_client: KodiClient::new(config.host.clone(), config.port),
            config,
            op: tokio::sync::Mutex::new(()),
            saw_running: AtomicBool::new(false),
            foreign_pid: AtomicU32::new(0),
            image_state: Mutex::new(ImageState::new()),
        }
    }

    pub fn browse(&self) -> KodiClient {
        self.browse_client.clone()
    }

    pub fn dialogs(&self) -> KodiClient {
        self.dialogs_client.clone()
    }

    pub fn player_rpc(&self) -> KodiClient {
        self.player_client.clone()
    }

    pub async fn status(&self) -> Status {
        // Do not take `op`: a long Files.GetDirectory on browse_client can hold
        // that client's write lock for the plugin timeout, and ensure/restart
        // already serialize under `op`. Status must stay responsive while an
        // add-on root is opening.
        self.status_snapshot().await
    }

    pub async fn ensure(&self) -> Result<Status, EngineError> {
        let _op = self.op.lock().await;
        self.ensure_locked().await
    }

    /// Run one settings `RunPlugin(...)` on this machine's Kodi.
    /// `ensure` has already released `op`. This does not take that lock, so a
    /// prompt poll can read the dialog while it is open.
    pub async fn run_settings_builtin(&self, builtin: &str) -> Result<(), EngineError> {
        crate::event::run_settings_builtin(&self.guisettings_path(), builtin).await
    }

    pub async fn restart(&self) -> Result<Status, EngineError> {
        let _op = self.op.lock().await;
        let status = self.status_locked().await;
        match restart_plan(status.state, status.started_by_us) {
            RestartChoice::Ensure => {
                // Restart is someone asking for Kodi. A Kodi this process did
                // not start is not replaced on its own when it exits, but once
                // none runs, the ask starts one, rather than "Kodi exited."
                // until Portal itself restarts. A failed snapshot is not "none
                // running": clearing the latch then would let the next start
                // pass while that Kodi was still there.
                if scan_kodi().is_some_and(|pids| pids.is_empty()) {
                    self.foreign_pid.store(0, Ordering::SeqCst);
                }
                self.ensure_locked().await
            }
            RestartChoice::LeaveForeign => Err(EngineError::ForeignRestart),
            RestartChoice::QuitThenEnsure => {
                self.quit_locked().await?;
                self.ensure_locked().await
            }
        }
    }

    pub async fn quit(&self) -> Result<Status, EngineError> {
        let _op = self.op.lock().await;
        self.quit_locked().await?;
        Ok(self.status_locked().await)
    }

    pub async fn set_window(&self, visible: bool) -> Result<Status, EngineError> {
        let _op = self.op.lock().await;
        let status = self.status_locked().await;
        if !status.started_by_us {
            return Err(EngineError::ForeignWindow);
        }
        let Some(pid) = status.pid else {
            return Err(EngineError::ForeignWindow);
        };
        end_start_guard();
        show_pid(pid, visible);
        Ok(self.status_locked().await)
    }

    /// Puts a hidden Kodi on screen so a dialog can take Back. Drop hides it
    /// again. A window that was already visible is left alone. One shot, not a hide loop.
    pub fn reveal_for_input(&self) -> Option<InputWindow> {
        let pids = kodi_pids();
        let owned = load_owned(&self.config.state_path);
        let (pid, _) = attach_decision(&pids, owned);
        pid.map(InputWindow::reveal)
    }

    /// Art from Kodi's own web server. This does not start Kodi.
    /// Credentials are parsed once. One HTTP client and a small byte cache are reused.
    pub async fn image(&self, path: &str) -> Result<(Vec<u8>, String), EngineError> {
        let mut roots = vec![self.config.profile.clone()];
        if let Some(dir) = self.config.kodi_exe.parent() {
            roots.push(dir.to_path_buf());
        }
        if !image_path_allowed(path, &roots) {
            return Err(EngineError::BadRequest("Unsupported image path".into()));
        }
        if let Some(hit) = self.cached_image(path) {
            return Ok(hit);
        }
        if let Some((host, port)) = web_art_host(path) {
            if !resolves_public(&host, port).await {
                return Err(EngineError::BadRequest("Unsupported image path".into()));
            }
        }
        if !self.web_ready().await {
            return Err(EngineError::Message(
                "Kodi's web server is off, so artwork cannot be shown. Portal turns it on whenever it starts Kodi.".into(),
            ));
        }
        let (client, creds) = self.image_transport()?;
        let fetched = match fetch_image(&client, &creds, path).await {
            Ok(fetched) => fetched,
            Err(err) => {
                // Kodi saves a changed web server password or port to its
                // settings file. Read it again next time instead of failing
                // every image until Portal restarts.
                self.forget_creds();
                return Err(err);
            }
        };
        self.store_image(path, &fetched.0, &fetched.1);
        Ok(fetched)
    }

    fn forget_creds(&self) {
        self.image_state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .creds = None;
    }

    fn cached_image(&self, path: &str) -> Option<(Vec<u8>, String)> {
        let state = self
            .image_state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        state
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| (entry.bytes.clone(), entry.content_type.clone()))
    }

    /// Kodi hands out artwork only through its web server, which a new Kodi
    /// has off. Portal turns it on in the settings file before it starts Kodi
    /// (`prepare_web_server`), never while Kodi runs: then Kodi asks yes or no
    /// in its own window, which a hidden Kodi never shows. False only when
    /// Kodi says the server is off. When Kodi does not answer, the settings
    /// file is used as before.
    async fn web_ready(&self) -> bool {
        if self
            .image_state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .creds
            .is_some()
        {
            return true;
        }
        match self
            .status_client
            .call(
                "Settings.GetSettingValue",
                json!({ "setting": "services.webserver" }),
                STATUS_RPC,
            )
            .await
        {
            Ok(on) => matches!(on.get("value"), Some(Value::Bool(true))),
            Err(_) => true,
        }
    }

    fn image_transport(&self) -> Result<(reqwest::Client, WebCreds), EngineError> {
        let mut state = self
            .image_state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if state.client.is_none() {
            state.client = Some(
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .timeout(Duration::from_secs(20))
                    .build()
                    .map_err(|_| EngineError::Message("Kodi setup failed.".into()))?,
            );
        }
        if state.creds.is_none() {
            let xml = fs::read_to_string(self.guisettings_path()).map_err(|_| {
                EngineError::Message("Could not read Kodi's web server settings.".into())
            })?;
            state.creds = Some(parse_web_creds(&xml)?);
        }
        match (state.client.clone(), state.creds.clone()) {
            (Some(client), Some(creds)) => Ok((client, creds)),
            _ => Err(EngineError::Message("Kodi setup failed.".into())),
        }
    }

    fn store_image(&self, path: &str, bytes: &[u8], content_type: &str) {
        if bytes.len() > IMAGE_CACHE_BYTES {
            return;
        }
        let mut state = self
            .image_state
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        if let Some(index) = state.entries.iter().position(|entry| entry.path == path) {
            if let Some(old) = state.entries.remove(index) {
                state.total = state.total.saturating_sub(old.bytes.len());
            }
        }
        while !state.entries.is_empty()
            && (state.entries.len() >= IMAGE_CACHE_CAP
                || state.total + bytes.len() > IMAGE_CACHE_BYTES)
        {
            if let Some(old) = state.entries.pop_front() {
                state.total = state.total.saturating_sub(old.bytes.len());
            }
        }
        if state.entries.is_empty() {
            state.total = 0;
        }
        state.total += bytes.len();
        state.entries.push_back(CachedImage {
            path: path.to_string(),
            bytes: bytes.to_vec(),
            content_type: content_type.to_string(),
        });
    }

    async fn status_locked(&self) -> Status {
        self.status_snapshot().await
    }

    async fn status_snapshot(&self) -> Status {
        let pids = kodi_pids();
        let owned = load_owned(&self.config.state_path);
        let (pid, started_by_us) = attach_decision(&pids, owned);
        let open = self.port_open().await;
        if !open {
            if self.saw_running.load(Ordering::SeqCst) && pid.is_none() {
                self.saw_running.store(false, Ordering::SeqCst);
                return Status::stopped(Some("Kodi exited.".into()));
            }
            if pid.is_some() {
                return Status {
                    state: EngineState::Starting,
                    pid,
                    version: None,
                    started_by_us,
                    window_visible: pid.is_some_and(window_visible),
                    webserver: false,
                    error: None,
                };
            }
            return Status::stopped(None);
        }
        self.saw_running.store(true, Ordering::SeqCst);
        // Probe on its own connection with a short budget so a stuck add-on
        // browse or a long Player.Open cannot freeze /api/status or the page chip.
        let (version, webserver) = self.probe_status_details().await;
        Status {
            state: EngineState::Running,
            pid,
            version,
            started_by_us,
            window_visible: pid.is_some_and(window_visible),
            webserver,
            error: None,
        }
    }

    async fn probe_status_details(&self) -> (Option<String>, bool) {
        if !self.status_client.connected().await
            && self
                .status_client
                .connect_timeout(Duration::from_millis(400))
                .await
                .is_err()
        {
            return (None, false);
        }
        let version = match self
            .status_client
            .call(
                "Application.GetProperties",
                json!({ "properties": ["version"] }),
                STATUS_RPC,
            )
            .await
        {
            Ok(value) => value.get("version").and_then(format_version),
            Err(_) => return (None, false),
        };
        let Some(version) = version else {
            return (None, false);
        };
        let webserver = match self
            .status_client
            .call(
                "Settings.GetSettingValue",
                json!({ "setting": "services.webserver" }),
                STATUS_RPC,
            )
            .await
        {
            Ok(value) => match value.get("value") {
                Some(Value::Bool(on)) => *on,
                Some(Value::String(text)) => text.eq_ignore_ascii_case("true"),
                _ => false,
            },
            Err(_) => false,
        };
        (Some(version), webserver)
    }

    async fn ensure_locked(&self) -> Result<Status, EngineError> {
        if self.port_open().await {
            return self.attach_locked().await;
        }
        // A snapshot that failed is not an empty machine. Starting then could
        // open a second Kodi beside one this check simply did not see.
        let Some(pids) = scan_kodi() else {
            return Err(EngineError::Unavailable(LIST_UNREADABLE));
        };
        if !pids.is_empty() {
            if self.wait_port(PORT_WAIT).await {
                return self.attach_locked().await;
            }
            // A Kodi we started whose port never opened is stuck, as at a
            // crash dialog. It is replaced once; anything else is left alone.
            let owned = load_owned(&self.config.state_path);
            let Some(pids) = scan_kodi() else {
                return Err(EngineError::Unavailable(LIST_UNREADABLE));
            };
            let Some(pid) = stuck_owned(&pids, owned) else {
                return Err(EngineError::Unavailable("Kodi is starting."));
            };
            // Blocks for up to 3 seconds while Kodi exits.
            let _ = tokio::task::spawn_blocking(move || terminate_owned(pid)).await;
            sleep(Duration::from_millis(300)).await;
            match scan_kodi() {
                Some(pids) if pids.is_empty() => {}
                Some(_) => return Err(EngineError::Unavailable("Kodi is not responding.")),
                None => return Err(EngineError::Unavailable(LIST_UNREADABLE)),
            }
            let _ = save_owned(&self.config.state_path, None);
            self.saw_running.store(false, Ordering::SeqCst);
        }
        if !spawn_allowed(self.foreign_pid.load(Ordering::SeqCst) != 0) {
            return Err(EngineError::Unavailable("Kodi exited."));
        }
        let exe = self.config.kodi_exe.clone();
        let guisettings = self.guisettings_path();
        let marker = self.web_marker_path();
        let launched =
            tokio::task::spawn_blocking(move || launch_if_alone(&exe, &guisettings, &marker))
            .await
            .map_err(|_| EngineError::Message("Could not start Kodi.".into()))?;
        let pid = match launched {
            Ok((pid, web)) => {
                if let Some(creds) = web {
                    tracing::info!("Turned on Kodi's web server for artwork, with a new random password");
                    self.image_state
                        .lock()
                        .unwrap_or_else(|err| err.into_inner())
                        .creds = Some(creds);
                }
                pid
            }
            Err(err) => {
                if self.port_open().await {
                    return self.attach_locked().await;
                }
                // It appeared while we were deciding. Use it; do not start another.
                if scan_kodi().is_some_and(|pids| !pids.is_empty())
                    && self.wait_port(PORT_WAIT).await
                {
                    return self.attach_locked().await;
                }
                return Err(err);
            }
        };
        save_owned(&self.config.state_path, Some(pid))?;
        if !self.wait_port(PORT_WAIT).await {
            return Err(EngineError::Message("Kodi did not open its port.".into()));
        }
        self.connect_all().await?;
        hide_when_ready(pid).await;
        Ok(self.status_locked().await)
    }

    /// Attaches to whatever holds Kodi's port. The first attach checks that it
    /// answers JSON-RPC, so another program on the port is not taken for Kodi.
    async fn attach_locked(&self) -> Result<Status, EngineError> {
        if !self.browse_client.connected().await
            && !answers_ping(&self.config.host, self.config.port).await
        {
            if kodi_pids().is_empty() {
                return Err(EngineError::Message(format!(
                    "Another program is using Kodi's port {}.",
                    self.config.port
                )));
            }
            return Err(EngineError::Unavailable("Kodi is not responding."));
        }
        self.connect_all().await?;
        let status = self.status_locked().await;
        self.note_attach(&status);
        Ok(status)
    }

    async fn quit_locked(&self) -> Result<(), EngineError> {
        let status = self.status_locked().await;
        if !status.started_by_us {
            return Err(EngineError::ForeignQuit);
        }
        let Some(pid) = status.pid else {
            return Err(EngineError::ForeignQuit);
        };
        if self.port_open().await {
            let _ = self.browse_client.connect().await;
            let _ = self
                .browse_client
                .call("Application.Quit", json!({}), RPC_LIMIT)
                .await;
        }
        let deadline = Instant::now() + QUIT_WAIT;
        while Instant::now() < deadline {
            if !kodi_pids().contains(&pid) {
                let _ = save_owned(&self.config.state_path, None);
                self.saw_running.store(false, Ordering::SeqCst);
                return Ok(());
            }
            sleep(Duration::from_millis(200)).await;
        }
        let pids = kodi_pids();
        if pids.len() == 1 && pids[0] == pid {
            // Blocks for up to 3 seconds while Kodi exits.
            let _ = tokio::task::spawn_blocking(move || terminate_owned(pid)).await;
            sleep(Duration::from_millis(300)).await;
        }
        let _ = save_owned(&self.config.state_path, None);
        self.saw_running.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn connect_all(&self) -> Result<(), EngineError> {
        self.browse_client.connect().await?;
        self.dialogs_client.connect().await?;
        self.player_client.connect().await?;
        Ok(())
    }

    async fn port_open(&self) -> bool {
        let address = format!("{}:{}", self.config.host, self.config.port);
        timeout(Duration::from_millis(400), TcpStream::connect(address))
            .await
            .ok()
            .and_then(|result| result.ok())
            .is_some()
    }

    async fn wait_port(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if self.port_open().await {
                return true;
            }
            sleep(Duration::from_millis(200)).await;
        }
        self.port_open().await
    }

    fn note_attach(&self, status: &Status) {
        if status.started_by_us {
            self.foreign_pid.store(0, Ordering::SeqCst);
        } else if let Some(pid) = status.pid {
            self.foreign_pid.store(pid, Ordering::SeqCst);
        }
    }

    /// Present once Portal has turned Kodi's web server on.
    fn web_marker_path(&self) -> PathBuf {
        self.config
            .state_path
            .with_file_name("kodi-web-server-on")
    }

    pub fn guisettings_path(&self) -> PathBuf {
        self.config.profile.join("userdata").join("guisettings.xml")
    }
}

/// The only running Kodi, when it is the one this process started.
fn stuck_owned(pids: &[u32], owned: Option<u32>) -> Option<u32> {
    match attach_decision(pids, owned) {
        (Some(pid), true) => Some(pid),
        _ => None,
    }
}

/// True when the program on the port answers `JSONRPC.Ping` as Kodi does.
/// A short connection of its own, closed again either way.
async fn answers_ping(host: &str, port: u16) -> bool {
    let client = KodiClient::new(host, port);
    if client.connect().await.is_err() {
        return false;
    }
    let answer = client.call("JSONRPC.Ping", json!({}), RPC_LIMIT).await;
    client.close().await;
    matches!(answer, Ok(Value::String(text)) if text == "pong")
}

fn format_version(value: &Value) -> Option<String> {
    let major = value.get("major")?.as_i64()?;
    let minor = value.get("minor")?.as_i64()?;
    let mut text = format!("{major}.{minor}");
    if let Some(tag) = value.get("tag").and_then(Value::as_str) {
        if !tag.is_empty() {
            text.push(' ');
            text.push_str(tag);
        }
    }
    Some(text)
}

const ALREADY_RUNNING: &str = "Kodi is already running.";
const LIST_UNREADABLE: &str = "Could not check whether Kodi is already running.";

/// What a process snapshot allows. `None` means the snapshot failed, which is
/// not the same as an empty list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StartGate {
    Start,
    AlreadyRunning,
    Unknown,
}

fn start_gate(running: Option<&[u32]>) -> StartGate {
    match running {
        Some([]) => StartGate::Start,
        Some(_) => StartGate::AlreadyRunning,
        None => StartGate::Unknown,
    }
}

fn refuse_start(running: Option<&[u32]>) -> Result<(), EngineError> {
    match start_gate(running) {
        StartGate::Start => Ok(()),
        StartGate::AlreadyRunning => Err(EngineError::Unavailable(ALREADY_RUNNING)),
        StartGate::Unknown => Err(EngineError::Unavailable(LIST_UNREADABLE)),
    }
}

/// True when `running` names some Kodi other than the process just created.
/// An unreadable list does not claim an extra: the new process is already up.
fn extra_started(pid: u32, running: Option<&[u32]>) -> bool {
    running.is_some_and(|pids| pids.iter().any(|other| *other != pid))
}

/// One start at a time, across every Portal process. The lock is held only
/// around the check and `CreateProcess`, not while waiting for the port.
#[cfg(windows)]
struct KodiStartLock(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl KodiStartLock {
    fn acquire() -> Result<Self, EngineError> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        let name: Vec<u16> = std::ffi::OsStr::new(r"Local\PortalKodiStart")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // The creator does not own it yet. Waiting once, whoever created it,
        // keeps the release count at one.
        unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            if handle.is_null() {
                return Err(EngineError::Message("Could not start Kodi.".into()));
            }
            let waited = WaitForSingleObject(handle, 15_000);
            if waited != WAIT_OBJECT_0 && waited != WAIT_ABANDONED {
                CloseHandle(handle);
                return Err(EngineError::Unavailable(ALREADY_RUNNING));
            }
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for KodiStartLock {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::ReleaseMutex;
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

/// Starts Kodi when, and only when, this call can see that none is running.
/// The web server login is the one written for this start, if any.
fn launch_if_alone(
    exe: &Path,
    guisettings: &Path,
    marker: &Path,
) -> Result<(u32, Option<WebCreds>), EngineError> {
    let _lock = KodiStartLock::acquire()?;
    refuse_start(scan_kodi().as_deref())?;
    // No Kodi is running, so none can write its settings over this change.
    let web = prepare_web_server(guisettings, marker);
    let pid = spawn_kodi(exe)?;
    guard_start(pid);
    if extra_started(pid, scan_kodi().as_deref()) {
        stop_pid(pid);
        return Err(EngineError::Unavailable(ALREADY_RUNNING));
    }
    Ok((pid, web))
}

/// Turns Kodi's web server on in its settings file, for artwork. Kodi starts
/// a server named there without asking; turned on while Kodi runs, it asks
/// yes or no in Kodi's own window instead, which a hidden Kodi never shows.
/// A missing file is created with just these settings, and Kodi fills in the
/// rest. A file that cannot be read as settings is left alone. `marker`
/// records that Portal turned the server on once: found off after that, the
/// viewer turned it off, and it stays off. Some(login) when this call turned
/// the server on.
fn prepare_web_server(path: &Path, marker: &Path) -> Option<WebCreds> {
    let current = match fs::read_to_string(path) {
        // An empty file is what a crash mid-save leaves; Kodi starts over.
        Ok(text) if text.trim().is_empty() => None,
        Ok(text) => Some(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return None,
    };
    if current.is_some() && marker.is_file() {
        return None;
    }
    let password = random_password()?;
    let (text, creds) = web_server_settings(current.as_deref(), &password)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).ok()?;
    }
    let temp = path.with_extension("xml.portal");
    if fs::write(&temp, text).is_err() || fs::rename(&temp, path).is_err() {
        let _ = fs::remove_file(&temp);
        return None;
    }
    let _ = fs::write(marker, b"Portal turned on Kodi's web server for artwork.\n");
    Some(creds)
}

/// The settings text with the web server on, and the login it uses. None
/// when the server is on already or the text is not Kodi's settings file.
/// An existing user name and password are kept; an empty password gets
/// `password`.
fn web_server_settings(current: Option<&str>, password: &str) -> Option<(String, WebCreds)> {
    let Some(current) = current else {
        let creds = WebCreds {
            port: 8080,
            username: "kodi".to_string(),
            password: password.to_string(),
        };
        let text = format!(
            "<settings version=\"2\">\n    <setting id=\"services.webserver\">true</setting>\n    <setting id=\"services.webserverusername\">kodi</setting>\n    <setting id=\"services.webserverpassword\">{password}</setting>\n</settings>\n"
        );
        return Some((text, creds));
    };
    // A cut-off file parses loosely; only a whole document is edited.
    if !crate::settings::xml_complete(current) {
        return None;
    }
    let root = parse_xml(current).ok()?;
    if root.name != "settings" {
        return None;
    }
    let value = |id: &str| {
        root.children
            .iter()
            .find(|node| node.name == "setting" && node.attr("id") == Some(id))
            .map(setting_value)
    };
    if value("services.webserver").as_deref() == Some("true") {
        return None;
    }
    let mut text = with_setting(current, "services.webserver", "true")?;
    let username = value("services.webserverusername")
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "kodi".to_string());
    if value("services.webserverusername").unwrap_or_default().is_empty() {
        text = with_setting(&text, "services.webserverusername", &username)?;
    }
    let kept = value("services.webserverpassword").filter(|word| !word.is_empty());
    let password = match kept {
        Some(word) => word,
        None => {
            text = with_setting(&text, "services.webserverpassword", password)?;
            password.to_string()
        }
    };
    let port = value("services.webserverport")
        .and_then(|port| port.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(8080);
    // Never hand Kodi a file it would throw away along with every setting.
    if !crate::settings::xml_complete(&text) {
        return None;
    }
    let check = parse_xml(&text).ok()?;
    let on = check
        .children
        .iter()
        .any(|node| node.attr("id") == Some("services.webserver") && setting_value(node) == "true");
    on.then_some((
        text,
        WebCreds {
            port,
            username,
            password,
        },
    ))
}

/// `xml` with setting `id` replaced by `value`, or added before the closing
/// `</settings>` when it is not there yet.
fn with_setting(xml: &str, id: &str, value: &str) -> Option<String> {
    let element = format!("<setting id=\"{id}\">{}</setting>", xml_escape(value));
    let open = format!("<setting id=\"{id}\"");
    let mut search = 0;
    while let Some(found) = xml[search..].find(&open) {
        let start = search + found;
        let after = xml[start + open.len()..].chars().next();
        if matches!(after, Some(' ' | '\t' | '\r' | '\n' | '/' | '>')) {
            let tag_end = start + xml[start..].find('>')?;
            let end = if xml[..tag_end].ends_with('/') {
                tag_end + 1
            } else {
                let close = xml[tag_end..].find("</setting>")?;
                tag_end + close + "</setting>".len()
            };
            return Some(format!("{}{element}{}", &xml[..start], &xml[end..]));
        }
        search = start + open.len();
    }
    let close = xml.rfind("</settings>")?;
    Some(format!("{}    {element}\n{}", &xml[..close], &xml[close..]))
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Kodi leaves Portal's job object so it outlives a Portal crash and is
/// attached again on the next launch. A parent job may forbid breakaway; then
/// Kodi starts inside the job instead. The second attempt runs only when the
/// first did not create a process and the machine still shows no Kodi.
#[cfg(windows)]
fn spawn_kodi(exe: &Path) -> Result<u32, EngineError> {
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
    match spawn_hidden(exe, base | CREATE_BREAKAWAY_FROM_JOB) {
        Some(pid) => Ok(pid),
        None => {
            refuse_start(scan_kodi().as_deref())?;
            spawn_hidden(exe, base)
                .ok_or_else(|| EngineError::Message("Could not start Kodi.".into()))
        }
    }
}

/// Starts a program whose first window stays hidden. Windows applies the
/// start-up "hidden" to the program's first show, so Kodi's window never
/// appears while it starts, rather than showing until Portal can hide it.
/// No handles are passed on, so Kodi has no console input or output.
#[cfg(windows)]
fn spawn_hidden(exe: &Path, flags: u32) -> Option<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let quote = u16::from(b'"');
    let path: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    // The program's own name, quoted, as its first argument.
    let mut line: Vec<u16> = std::iter::once(quote)
        .chain(exe.as_os_str().encode_wide())
        .chain([quote, 0])
        .collect();
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESHOWWINDOW;
    startup.wShowWindow = SW_HIDE as u16;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: both strings end in a zero and outlive the call; the
    // structures are sized and zeroed as the call expects.
    let ok = unsafe {
        CreateProcessW(
            path.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            flags,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut info,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: the handles were just returned to this process and are closed once.
    unsafe {
        CloseHandle(info.hThread);
        CloseHandle(info.hProcess);
    }
    Some(info.dwProcessId)
}

async fn hide_when_ready(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let windows = windows_of(pid);
        if !windows.is_empty() {
            for hwnd in windows {
                show_hwnd(hwnd, false);
            }
            return;
        }
        if Instant::now() >= deadline {
            return;
        }
        sleep(Duration::from_millis(200)).await;
    }
}

fn show_pid(pid: u32, visible: bool) {
    for hwnd in windows_of(pid) {
        show_hwnd(hwnd, visible);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlacedWindow {
    hwnd: isize,
    left: i32,
    top: i32,
    width: i32,
    height: i32,
}

/// Show the only running Kodi without activating it, so a dialog close
/// animation can finish. Drop hides it again. Two Kodi processes are left alone.
pub(crate) fn reveal_player_window() -> Option<InputWindow> {
    let pids = kodi_pids();
    if pids.len() != 1 {
        return None;
    }
    Some(InputWindow::reveal(pids[0]))
}

/// Show Kodi for a yes/no answer without first parking it off the monitor.
/// Parking it there makes Select look successful while the dialog never receives it.
pub(crate) fn hide_player() {
    let pids = kodi_pids();
    if pids.len() == 1 {
        show_pid(pids[0], false);
    }
}

pub(crate) fn reveal_player_for_answer() -> Option<InputWindow> {
    let pids = kodi_pids();
    if pids.len() != 1 {
        return None;
    }
    let window = InputWindow::open_for_answer(pids[0]);
    window.show_activated();
    Some(window)
}

/// Holds a Kodi window on screen for one dialog action.
pub struct InputWindow {
    was_visible: bool,
    origin: Vec<PlacedWindow>,
    restored: bool,
}

impl InputWindow {
    fn reveal(pid: u32) -> Self {
        let was_visible = window_visible(pid);
        // Every top-level window. A titled splash can be the only "Kodi"
        // hwnd while the dialog's window stays hidden and never animates.
        let origin = windows_of(pid)
            .into_iter()
            .filter_map(|hwnd| {
                let (left, top, right, bottom) = window_rect(hwnd)?;
                Some(PlacedWindow {
                    hwnd,
                    left,
                    top,
                    width: (right - left).max(1),
                    height: (bottom - top).max(1),
                })
            })
            .collect();
        let window = Self {
            was_visible,
            origin,
            restored: false,
        };
        if begin_reveal(&window.origin, was_visible) && !was_visible {
            window.present();
        }
        window
    }

    /// Remember where Kodi was, and leave it there. The caller shows it on
    /// the work area. `reveal` parks a hidden player off-screen first, which
    /// a yes/no click must not do.
    fn open_for_answer(pid: u32) -> Self {
        let was_visible = window_visible(pid);
        let origin = windows_of(pid)
            .into_iter()
            .filter_map(|hwnd| {
                let (left, top, right, bottom) = window_rect(hwnd)?;
                Some(PlacedWindow {
                    hwnd,
                    left,
                    top,
                    width: (right - left).max(1),
                    height: (bottom - top).max(1),
                })
            })
            .collect();
        let window = Self {
            was_visible,
            origin,
            restored: false,
        };
        let _ = begin_reveal(&window.origin, was_visible);
        window
    }

    /// Kodi was hidden before this reveal, so it goes back to hidden.
    pub fn was_hidden(&self) -> bool {
        !self.was_visible
    }

    pub fn assert_shown(&self) {
        if !self.was_visible {
            self.present();
        }
    }

    /// Put Kodi back where it was and show it, without activating it.
    /// `present` parks the window off the monitor so a busy add-on does not flash
    /// a black box. A yes/no click from there starts to close and then stays
    /// open, so the same question comes back.
    pub fn show_for_answer(&self) {
        for window in &self.origin {
            let (x, y, width, height) = onscreen_place(window);
            place_window_inactive(window.hwnd, x, y, width, height);
            show_inactive(window.hwnd);
        }
    }

    /// Show Kodi and let it take focus. A yes/no button ignores Left and
    /// Select while the window is only shown without activation. This is not
    /// used while an add-on is busy: activating Kodi then makes Windows close it.
    pub fn show_activated(&self) {
        for window in &self.origin {
            let (x, y, width, height) = onscreen_place(window);
            place_window_with(window.hwnd, x, y, width, height, false);
            show_hwnd(window.hwnd, true);
        }
    }

    fn present(&self) {
        // Activation makes Windows close a Kodi whose screen thread is busy in an add-on.
        for window in &self.origin {
            let width = window.width.max(960);
            let height = window.height.max(540);
            // Shown is what lets Kodi animate; on screen it only flashes a black box.
            let (x, y) = offscreen_spot(window, width, height).unwrap_or_else(beyond_desktop);
            place_window_inactive(window.hwnd, x, y, width, height);
            show_inactive(window.hwnd);
        }
    }

    fn restore_now(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        // The outermost reveal's places: an inner reveal saw Kodi already
        // moved on screen, and may be the last one to end.
        let Some(origin) = end_reveal() else {
            return;
        };
        for window in &origin {
            // Hidden first, then moved back while hidden: a move that also
            // shows the window would put Kodi on screen at its old place
            // for a moment. Inactive, as in present(): an activating move
            // during a busy add-on makes Windows close Kodi.
            show_hwnd(window.hwnd, false);
            place_hidden(
                window.hwnd,
                window.left,
                window.top,
                window.width,
                window.height,
            );
        }
    }
}

impl Drop for InputWindow {
    fn drop(&mut self) {
        self.restore_now();
    }
}

/// Counts a reveal. The outermost one keeps where Kodi's windows were and
/// whether Kodi was hidden. Returns true for the outermost reveal.
fn begin_reveal(origin: &[PlacedWindow], was_visible: bool) -> bool {
    let mut saved = REVEAL_ORIGIN.lock().unwrap_or_else(|err| err.into_inner());
    let depth = REVEAL_DEPTH.fetch_add(1, Ordering::SeqCst);
    if depth == 0 {
        REVEAL_HIDE.store(!was_visible, Ordering::SeqCst);
        *saved = origin.to_vec();
    }
    depth == 0
}

/// Ends a reveal. When the last one ends and Kodi was hidden before the
/// first, returns the places the outermost reveal saw.
fn end_reveal() -> Option<Vec<PlacedWindow>> {
    let mut saved = REVEAL_ORIGIN.lock().unwrap_or_else(|err| err.into_inner());
    let prev = REVEAL_DEPTH.fetch_sub(1, Ordering::SeqCst);
    if prev != 1 {
        return None;
    }
    let origin = std::mem::take(&mut *saved);
    REVEAL_HIDE.load(Ordering::SeqCst).then_some(origin)
}

/// A place just past an edge of the monitor the window was on, clear of every
/// screen. Kodi still finds that monitor nearest, so it keeps its screen
/// setting. None when every side runs into another screen.
#[cfg(windows)]
fn offscreen_spot(window: &PlacedWindow, width: i32, height: i32) -> Option<(i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        MONITOR_DEFAULTTONULL,
    };
    const GAP: i32 = 64;
    let rect = |x: i32, y: i32, w: i32, h: i32| RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    };
    let was = rect(window.left, window.top, window.width, window.height);
    let home = unsafe { MonitorFromRect(&was, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if home.is_null() || unsafe { GetMonitorInfoW(home, &mut info) } == 0 {
        return None;
    }
    let screen = info.rcMonitor;
    [
        (screen.right + GAP, screen.top),
        (screen.left - width - GAP, screen.top),
        (screen.left, screen.bottom + GAP),
        (screen.left, screen.top - height - GAP),
    ]
    .into_iter()
    .find(|&(x, y)| {
        let spot = rect(x, y, width, height);
        unsafe {
            MonitorFromRect(&spot, MONITOR_DEFAULTTONULL).is_null()
                && MonitorFromRect(&spot, MONITOR_DEFAULTTONEAREST) == home
        }
    })
}

/// Just past the right edge of every monitor together, for when each side
/// of Kodi's own monitor has another one beside it.
#[cfg(windows)]
fn beyond_desktop() -> (i32, i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    let (left, width, top) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        )
    };
    (left.saturating_add(width).saturating_add(64), top)
}

/// A spot on the monitor work area. The saved place is kept when it already
/// sits on that area. A window parked off every screen is brought back,
/// because a dialog there never finishes.
#[cfg(windows)]
fn onscreen_place(window: &PlacedWindow) -> (i32, i32, i32, i32) {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let saved = RECT {
        left: window.left,
        top: window.top,
        right: window.left.saturating_add(window.width),
        bottom: window.top.saturating_add(window.height),
    };
    let home = unsafe { MonitorFromRect(&saved, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if home.is_null() || unsafe { GetMonitorInfoW(home, &mut info) } == 0 {
        return (
            window.left,
            window.top,
            window.width.max(1),
            window.height.max(1),
        );
    }
    let work = info.rcWork;
    let on_work = window.width > 80
        && window.height > 80
        && window.left < work.right
        && saved.right > work.left
        && window.top < work.bottom
        && saved.bottom > work.top;
    if on_work {
        return (window.left, window.top, window.width, window.height);
    }
    let span_x = (work.right - work.left).max(1);
    let span_y = (work.bottom - work.top).max(1);
    let width = window.width.max(960).min(span_x);
    let height = window.height.max(540).min(span_y);
    let x = work.left + ((span_x - width) / 2).max(0);
    let y = work.top + ((span_y - height) / 2).max(0);
    (x, y, width, height)
}

#[cfg(windows)]
fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    let ok = unsafe { GetWindowRect(hwnd as usize as _, &mut rect) };
    if ok == 0 {
        None
    } else {
        Some((rect.left, rect.top, rect.right, rect.bottom))
    }
}

fn place_window_inactive(hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    place_window_with(hwnd, x, y, width, height, true);
}

#[cfg(windows)]
fn place_window_with(hwnd: isize, x: i32, y: i32, width: i32, height: i32, inactive: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    };
    let mut flags = SWP_NOZORDER | SWP_SHOWWINDOW;
    if inactive {
        flags |= SWP_NOACTIVATE;
    }
    unsafe {
        SetWindowPos(
            hwnd as usize as _,
            std::ptr::null_mut(),
            x,
            y,
            width.max(1),
            height.max(1),
            flags,
        );
    }
}

/// Moves and sizes a window without showing or activating it.
#[cfg(windows)]
fn place_hidden(hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
    };
    unsafe {
        SetWindowPos(
            hwnd as usize as _,
            std::ptr::null_mut(),
            x,
            y,
            width.max(1),
            height.max(1),
            SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

#[cfg(windows)]
fn show_inactive(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNA};
    unsafe {
        ShowWindow(hwnd as usize as _, SW_SHOWNA);
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct OwnedFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    owned_pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    started: Option<u64>,
}

/// The Kodi this Portal started, when that same process still runs. A saved
/// id alone could name a later Kodi that reused it after a restart, so the
/// process's start time must match too.
fn load_owned(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let file: OwnedFile = serde_json::from_str(&text).ok()?;
    let pid = file.owned_pid.filter(|pid| *pid > 0)?;
    let started = file.started?;
    (process_started(pid) == Some(started)).then_some(pid)
}

fn save_owned(path: &Path, pid: Option<u32>) -> Result<(), EngineError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|err| EngineError::Message(err.to_string()))?;
    }
    let pid = pid.filter(|value| *value > 0);
    let body = serde_json::to_string(&OwnedFile {
        owned_pid: pid,
        started: pid.and_then(process_started),
    })
    .map_err(|err| EngineError::Message(err.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, body).map_err(|err| EngineError::Message(err.to_string()))?;
    fs::rename(&tmp, path).map_err(|err| EngineError::Message(err.to_string()))?;
    Ok(())
}

/// When a process started, as Windows counts time. None when it is gone or
/// cannot be read.
#[cfg(windows)]
fn process_started(pid: u32) -> Option<u64> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(handle);
        (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// The Windows session a process runs in. None when it cannot be read.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
fn session_of(pid: u32) -> Option<u32> {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let mut session = 0u32;
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

/// 32 random letters and digits, for Kodi's web server login.
fn random_password() -> Option<String> {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789";
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).ok()?;
    Some(
        bytes
            .iter()
            .map(|byte| CHARS[usize::from(*byte) % CHARS.len()] as char)
            .collect(),
    )
}

#[derive(Clone)]
struct WebCreds {
    port: u16,
    username: String,
    password: String,
}

struct ImageState {
    client: Option<reqwest::Client>,
    creds: Option<WebCreds>,
    entries: VecDeque<CachedImage>,
    total: usize,
}

struct CachedImage {
    path: String,
    bytes: Vec<u8>,
    content_type: String,
}

impl ImageState {
    fn new() -> Self {
        Self {
            client: None,
            creds: None,
            entries: VecDeque::new(),
            total: 0,
        }
    }
}

fn parse_web_creds(xml: &str) -> Result<WebCreds, EngineError> {
    let root = parse_xml(xml)
        .map_err(|_| EngineError::Message("Could not read Kodi's web server settings.".into()))?;
    let mut password: Option<String> = None;
    let mut username: Option<String> = None;
    let mut port: Option<String> = None;
    walk_creds(&root, &mut password, &mut username, &mut port);
    let Some(password) = password else {
        return Err(EngineError::Message(
            "The web server password element is missing.".into(),
        ));
    };
    let port = port
        .as_deref()
        .and_then(|text| text.trim().parse::<u16>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(8080);
    Ok(WebCreds {
        port,
        username: username
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "kodi".to_string()),
        password,
    })
}

fn walk_creds(
    node: &XmlNode,
    password: &mut Option<String>,
    username: &mut Option<String>,
    port: &mut Option<String>,
) {
    let name = node.name.as_str();
    if name == "setting" {
        match node.attr("id") {
            Some("services.webserverpassword") => *password = Some(setting_value(node)),
            Some("services.webserverusername") => *username = Some(setting_value(node)),
            Some("services.webserverport") => *port = Some(setting_value(node)),
            _ => {}
        }
    } else if name == "webserverpassword" {
        *password = Some(setting_value(node));
    } else if name == "webserverusername" {
        *username = Some(setting_value(node));
    } else if name == "webserverport" {
        *port = Some(setting_value(node));
    }
    for child in &node.children {
        walk_creds(child, password, username, port);
    }
}

/// Attribute `value` wins. Element text is next. The `default` attribute is never a password.
fn setting_value(node: &XmlNode) -> String {
    if let Some(value) = node.attr("value") {
        return value.to_string();
    }
    node.text.trim().to_string()
}

fn image_failure(status: u16) -> EngineError {
    if status == 401 || status == 403 {
        EngineError::Message("Kodi setup failed.".into())
    } else {
        EngineError::Message("Kodi image could not be read.".into())
    }
}

async fn fetch_image(
    client: &reqwest::Client,
    creds: &WebCreds,
    path: &str,
) -> Result<(Vec<u8>, String), EngineError> {
    let url = format!(
        "http://127.0.0.1:{}/image/{}",
        creds.port,
        percent_encode(path)
    );
    let response = client
        .get(url)
        .basic_auth(&creds.username, Some(&creds.password))
        .send()
        .await
        .map_err(|_| EngineError::Message("Kodi image could not be read.".into()))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(image_failure(status));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.split(';').next().unwrap_or(value).trim().to_string());
    let bytes = response
        .bytes()
        .await
        .map_err(|_| EngineError::Message("Kodi image could not be read.".into()))?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(EngineError::Message("Kodi image could not be read.".into()));
    }
    let sniffed = sniff_image(&bytes);
    Ok((bytes.to_vec(), content_type.unwrap_or(sniffed)))
}

fn sniff_image(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg".to_string()
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        "image/png".to_string()
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp".to_string()
    } else {
        "image/jpeg".to_string()
    }
}

fn percent_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct XmlNode {
    name: String,
    attrs: Vec<(String, String)>,
    text: String,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

fn parse_xml(xml: &str) -> Result<XmlNode, String> {
    let xml = xml.trim_start_matches('\u{feff}');
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                let owned = event.into_owned();
                buf.clear();
                let name = local_name(owned.name().as_ref());
                let attrs = collect_attrs(&owned);
                return read_element(&mut reader, &mut buf, name, attrs);
            }
            Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                let attrs = collect_attrs(&event);
                return Ok(XmlNode {
                    name,
                    attrs,
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Eof) => return Err("empty document".into()),
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }
}

fn read_element(
    reader: &mut Reader<&[u8]>,
    buf: &mut Vec<u8>,
    name: String,
    attrs: Vec<(String, String)>,
) -> Result<XmlNode, String> {
    let mut node = XmlNode {
        name,
        attrs,
        text: String::new(),
        children: Vec::new(),
    };
    loop {
        match reader.read_event_into(buf) {
            Ok(Event::Start(event)) => {
                let owned = event.into_owned();
                buf.clear();
                let child_name = local_name(owned.name().as_ref());
                let child_attrs = collect_attrs(&owned);
                node.children
                    .push(read_element(reader, buf, child_name, child_attrs)?);
            }
            Ok(Event::Empty(event)) => {
                node.children.push(XmlNode {
                    name: local_name(event.name().as_ref()),
                    attrs: collect_attrs(&event),
                    text: String::new(),
                    children: Vec::new(),
                });
            }
            Ok(Event::Text(event)) => {
                if let Ok(text) = event.unescape() {
                    node.text.push_str(&text);
                }
            }
            Ok(Event::CData(event)) => {
                node.text.push_str(&String::from_utf8_lossy(&event));
            }
            Ok(Event::End(_)) | Ok(Event::Eof) => break,
            Err(err) => return Err(err.to_string()),
            _ => {}
        }
        buf.clear();
    }
    Ok(node)
}

fn local_name(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let text = text.rsplit('}').next().unwrap_or(text.as_ref());
    text.rsplit(':').next().unwrap_or(text).to_string()
}

fn collect_attrs(event: &quick_xml::events::BytesStart<'_>) -> Vec<(String, String)> {
    event
        .attributes()
        .filter_map(|attr| attr.ok())
        .map(|attr| {
            let key = local_name(attr.key.as_ref());
            let value = attr
                .unescape_value()
                .map(|text| text.into_owned())
                .unwrap_or_default();
            (key, value)
        })
        .collect()
}

fn kodi_pids() -> Vec<u32> {
    scan_kodi().unwrap_or_default()
}

/// `None` when the process list could not be read. Callers that start Kodi
/// must treat that as "do not start", never as "nothing is running".
#[cfg(test)]
fn scan_kodi() -> Option<Vec<u32>> {
    // Tests never see a real Kodi, so they cannot show, hide, start or stop
    // the one the user is running.
    None
}

#[cfg(all(windows, not(test)))]
fn scan_kodi() -> Option<Vec<u32>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut pids = Vec::new();
        if Process32FirstW(snap, &mut entry) == 0 {
            CloseHandle(snap);
            return None;
        }
        // Another Windows user's Kodi, in their own session, is not ours to
        // attach to, show, hide or stop.
        let own = session_of(std::process::id());
        loop {
            if exe_is_kodi(&entry.szExeFile) && session_of(entry.th32ProcessID) == own {
                pids.push(entry.th32ProcessID);
            }
            if Process32NextW(snap, &mut entry) == 0 {
                break;
            }
        }
        CloseHandle(snap);
        Some(pids)
    }
}

#[cfg(all(windows, not(test)))]
fn exe_is_kodi(name: &[u16]) -> bool {
    let end = name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len());
    String::from_utf16_lossy(&name[..end]).eq_ignore_ascii_case("kodi.exe")
}

#[cfg(windows)]
static WINDOW_GATE: Mutex<()> = Mutex::new(());
#[cfg(windows)]
static WINDOW_FOUND: Mutex<Vec<isize>> = Mutex::new(Vec::new());
#[cfg(windows)]
static WINDOW_PID: AtomicU32 = AtomicU32::new(0);
/// Overlapping reveals share one show. Only the last drop hides Kodi.
static REVEAL_DEPTH: AtomicU32 = AtomicU32::new(0);
static REVEAL_HIDE: AtomicBool = AtomicBool::new(false);
/// Where Kodi's windows were before the outermost reveal.
static REVEAL_ORIGIN: Mutex<Vec<PlacedWindow>> = Mutex::new(Vec::new());
/// The Kodi Portal just started, kept hidden while it starts; 0 for none.
static START_GUARD: AtomicU32 = AtomicU32::new(0);
/// How long a starting Kodi is kept hidden: past the port wait, for the
/// skin and add-ons that load after the port opens.
const START_GUARD_FOR: Duration = Duration::from_secs(45);
const START_GUARD_EVERY: Duration = Duration::from_millis(10);

/// Keeps a Kodi that Portal started hidden while it starts. Starting it
/// hidden covers its first show, but Kodi can show its main window again as
/// it starts, placing it on a monitor as it does. Windows reports each show
/// to this thread as it happens, and the window is hidden at once, before
/// the screen is next drawn; a look every few milliseconds backs that up.
/// A reveal for a dialog, or the viewer showing Kodi, is left alone.
#[cfg(windows)]
fn guard_start(pid: u32) {
    START_GUARD.store(pid, Ordering::SeqCst);
    std::thread::spawn(move || {
        use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjects, PeekMessageW, TranslateMessage,
            EVENT_OBJECT_SHOW, MSG, PM_REMOVE, QS_ALLINPUT, WINEVENT_OUTOFCONTEXT,
        };
        // SAFETY: the hook calls back on this thread, which pumps its
        // messages below and removes the hook before it ends.
        let hook = unsafe {
            SetWinEventHook(
                EVENT_OBJECT_SHOW,
                EVENT_OBJECT_SHOW,
                std::ptr::null_mut(),
                Some(on_start_show),
                pid,
                0,
                WINEVENT_OUTOFCONTEXT,
            )
        };
        let deadline = std::time::Instant::now() + START_GUARD_FOR;
        let wait = START_GUARD_EVERY.as_millis() as u32;
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while std::time::Instant::now() < deadline && START_GUARD.load(Ordering::SeqCst) == pid {
            unsafe {
                MsgWaitForMultipleObjects(0, std::ptr::null(), 0, wait, QS_ALLINPUT);
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if REVEAL_DEPTH.load(Ordering::SeqCst) == 0 {
                hide_shown(pid);
            }
        }
        if !hook.is_null() {
            unsafe {
                UnhookWinEvent(hook);
            }
        }
        let _ = START_GUARD.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
    });
}

/// A window of the starting Kodi was shown: hide it again, unless a reveal
/// has it on screen on purpose or the guard has ended.
#[cfg(windows)]
unsafe extern "system" fn on_start_show(
    _hook: windows_sys::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: windows_sys::Win32::Foundation::HWND,
    object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetAncestor, GetWindowThreadProcessId, ShowWindow, GA_ROOT, OBJID_WINDOW, SW_HIDE,
    };
    if object != OBJID_WINDOW || hwnd.is_null() {
        return;
    }
    // Only Kodi's own top-level windows; a control inside one is not hidden.
    if GetAncestor(hwnd, GA_ROOT) != hwnd {
        return;
    }
    let guarded = START_GUARD.load(Ordering::SeqCst);
    let mut owner = 0u32;
    GetWindowThreadProcessId(hwnd, &mut owner);
    if guarded == 0 || owner != guarded || REVEAL_DEPTH.load(Ordering::SeqCst) != 0 {
        return;
    }
    ShowWindow(hwnd, SW_HIDE);
}

/// Hides those of a process's windows that are on screen.
#[cfg(windows)]
fn hide_shown(pid: u32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    for hwnd in windows_of(pid) {
        if unsafe { IsWindowVisible(hwnd as usize as _) } != 0 {
            show_hwnd(hwnd, false);
        }
    }
}

/// Stops keeping a starting Kodi hidden: the viewer chose to see it.
fn end_start_guard() {
    START_GUARD.store(0, Ordering::SeqCst);
}

#[cfg(windows)]
fn windows_of(pid: u32) -> Vec<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;
    let _gate = WINDOW_GATE.lock().unwrap_or_else(|err| err.into_inner());
    WINDOW_PID.store(pid, Ordering::SeqCst);
    {
        let mut found = WINDOW_FOUND.lock().unwrap_or_else(|err| err.into_inner());
        found.clear();
    }
    unsafe {
        EnumWindows(Some(on_window), 0);
    }
    WINDOW_FOUND
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
}

#[cfg(windows)]
unsafe extern "system" fn on_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    _: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::BOOL {
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
    let mut owner = 0u32;
    GetWindowThreadProcessId(hwnd, &mut owner);
    if owner == WINDOW_PID.load(Ordering::SeqCst) {
        if let Ok(mut found) = WINDOW_FOUND.lock() {
            found.push(hwnd as isize);
        }
    }
    TRUE
}

#[cfg(windows)]
fn window_visible(pid: u32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    windows_of(pid)
        .into_iter()
        .any(|hwnd| unsafe { IsWindowVisible(hwnd as usize as _) != 0 })
}

#[cfg(windows)]
fn show_hwnd(hwnd: isize, visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOW};
    let flag = if visible { SW_SHOW } else { SW_HIDE };
    unsafe {
        ShowWindow(hwnd as usize as _, flag);
    }
}

#[cfg(windows)]
fn terminate_owned(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    let pids = kodi_pids();
    if pids.len() != 1 || pids[0] != pid {
        return;
    }
    unsafe {
        // The wait needs SYNCHRONIZE. Without it the wait fails at once.
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return;
        }
        TerminateProcess(handle, 1);
        WaitForSingleObject(handle, 3000);
        CloseHandle(handle);
    }
}

/// Ends a process this start created after the snapshot showed another Kodi.
/// Unlike `terminate_owned`, it still runs when more than one `kodi.exe` exists,
/// because the extra one is the process just created.
#[cfg(windows)]
fn stop_pid(pid: u32) {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    if pid == 0 {
        return;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return;
        }
        TerminateProcess(handle, 1);
        WaitForSingleObject(handle, 3000);
        CloseHandle(handle);
    }
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/process.rs"]
mod tests;
