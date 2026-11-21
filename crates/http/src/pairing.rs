//! Other devices pair once before they may use Portal.
//!
//! A phone or TV that is not paired asks for a number and shows it. That
//! number is typed into Portal on this PC, which pairs the device. The device
//! then keeps a long random key in a cookie. Only the key's SHA-256 is saved,
//! in `devices.json`, and removing a device on this PC ends its access.
//!
//! This PC never pairs: loopback and the PC's own addresses always pass.
//! Every other device, paired or trusted, may only watch and cast: browse,
//! play, answer the add-on's questions during a play, cast, and keep its
//! watchlist. Settings, add-ons, the engine and updates change only on this
//! PC. [`remote_may`] is an allowlist, so a new route is PC-only until it is
//! added there.
//! Video and the few files a cast receiver loads stay open, since a TV's
//! built-in player cannot pair. Their addresses carry a random play id.

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{ConnectInfo, Path as UrlPath, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{fail, from_this_pc, ApiError, AppState};

const COOKIE: &str = "portal_device";
/// A shown number works this long.
const CODE_LIFE: Duration = Duration::from_secs(10 * 60);
const MAX_PENDING: usize = 16;
const MAX_PENDING_PER_ADDRESS: usize = 3;
const MAX_DEVICES: usize = 64;
/// A device's last-seen time is written at most this often.
const SEEN_EVERY_MS: i64 = 60 * 60 * 1000;
const NOT_PAIRED: &str = "Pair this device with Portal first.";
const THIS_PC_ONLY: &str = "Devices can only be paired on the PC running Portal.";
const PC_ONLY: &str = "That can only be changed on the PC running Portal.";

pub struct Pairing {
    path: PathBuf,
    trust_lan: bool,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    devices: Vec<Device>,
    pending: Vec<Pending>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Device {
    id: String,
    name: String,
    key_hash: String,
    paired_at: i64,
    #[serde(default)]
    last_seen: i64,
    #[serde(default)]
    address: String,
}

struct Pending {
    code: String,
    ticket: String,
    name: String,
    address: IpAddr,
    created: Instant,
    /// Set when this PC typed the number in; handed to the device once.
    key: Option<String>,
    /// The device that approval added, until it collects its key.
    device: Option<String>,
}

#[derive(Serialize)]
pub struct DevicePublic {
    id: String,
    name: String,
    paired_at: i64,
    last_seen: i64,
    address: String,
}

#[derive(Serialize)]
pub struct Me {
    paired: bool,
    this_pc: bool,
}

#[derive(Serialize)]
pub struct Started {
    code: String,
    ticket: String,
    expires_in: u64,
}

#[derive(Deserialize)]
pub struct TicketQuery {
    ticket: String,
}

#[derive(Deserialize)]
pub struct Approve {
    code: String,
}

impl Pairing {
    pub fn open(portal_dir: &Path) -> Self {
        let path = portal_dir.join("devices.json");
        let devices = match crate::paths::read_settings(&path) {
            Some(text) => serde_json::from_str::<Vec<Device>>(&text).unwrap_or_else(|_| {
                crate::paths::set_aside(&path);
                Vec::new()
            }),
            None => Vec::new(),
        };
        Self {
            path,
            trust_lan: std::env::var("PORTAL_TRUST_LAN").is_ok_and(|value| value == "1"),
            inner: Mutex::new(Inner {
                devices,
                pending: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|err| err.into_inner())
    }

    /// True when the request may use the API: from this PC, from a paired
    /// device, or with pairing turned off. A request a proxy on this PC
    /// passed along is not from this PC: it needs a paired device's cookie.
    fn allowed(&self, peer: SocketAddr, headers: &HeaderMap) -> bool {
        if self.trust_lan || this_pc(peer, headers) {
            return true;
        }
        let Some(key) = cookie_key(headers) else {
            return false;
        };
        let hash = digest(&key);
        let now = now_ms();
        let address = peer.ip().to_canonical().to_string();
        let mut inner = self.lock();
        let Some(index) = inner.devices.iter().position(|device| device.key_hash == hash) else {
            return false;
        };
        let device = &mut inner.devices[index];
        if now - device.last_seen > SEEN_EVERY_MS || device.address != address {
            device.last_seen = now;
            device.address = address;
            self.save(&inner.devices);
        }
        true
    }

    fn start(&self, peer: SocketAddr, name: String) -> Result<Started, &'static str> {
        let address = peer.ip().to_canonical();
        let mut inner = self.lock();
        self.prune(&mut inner);
        // Full lists drop their oldest unapproved number rather than refuse,
        // so one device asking again and again cannot lock everyone out.
        let from_here = inner
            .pending
            .iter()
            .filter(|pending| pending.address == address && pending.key.is_none())
            .count();
        if from_here >= MAX_PENDING_PER_ADDRESS {
            if let Some(oldest) = inner
                .pending
                .iter()
                .position(|pending| pending.address == address && pending.key.is_none())
            {
                inner.pending.remove(oldest);
            }
        }
        if inner.pending.len() >= MAX_PENDING {
            let Some(oldest) = inner.pending.iter().position(|pending| pending.key.is_none()) else {
                return Err("Too many numbers are waiting. Try again in a few minutes.");
            };
            inner.pending.remove(oldest);
        }
        let code = loop {
            let code = random_code().ok_or("Portal could not make a number.")?;
            if !inner.pending.iter().any(|pending| pending.code == code) {
                break code;
            }
        };
        let ticket = random_key().ok_or("Portal could not make a number.")?;
        inner.pending.push(Pending {
            code: code.clone(),
            ticket: ticket.clone(),
            name,
            address,
            created: Instant::now(),
            key: None,
            device: None,
        });
        Ok(Started {
            code,
            ticket,
            expires_in: CODE_LIFE.as_secs(),
        })
    }

    /// The key once this PC typed the number in, handed out only once.
    /// `Ok(None)` while the number is still waiting.
    fn status(&self, ticket: &str) -> Result<Option<String>, ()> {
        let mut inner = self.lock();
        self.prune(&mut inner);
        let Some(index) = inner.pending.iter().position(|pending| pending.ticket == ticket) else {
            return Err(());
        };
        if inner.pending[index].key.is_none() {
            return Ok(None);
        }
        Ok(inner.pending.remove(index).key)
    }

    fn approve(&self, code: &str) -> Result<String, &'static str> {
        let code: String = code.chars().filter(char::is_ascii_digit).collect();
        let mut inner = self.lock();
        self.prune(&mut inner);
        if inner.devices.len() >= MAX_DEVICES {
            return Err("Remove a device before pairing another.");
        }
        let Some(index) = inner
            .pending
            .iter()
            .position(|pending| pending.code == code && pending.key.is_none())
        else {
            return Err("No device is showing that number. Check it, or show a new one.");
        };
        let key = random_key().ok_or("Portal could not pair the device.")?;
        let id = random_key().ok_or("Portal could not pair the device.")?[..12].to_string();
        let now = now_ms();
        let pending = &mut inner.pending[index];
        pending.key = Some(key.clone());
        let device = Device {
            id,
            name: pending.name.clone(),
            key_hash: digest(&key),
            paired_at: now,
            last_seen: now,
            address: pending.address.to_string(),
        };
        let name = device.name.clone();
        pending.device = Some(device.id.clone());
        inner.devices.push(device);
        self.save(&inner.devices);
        Ok(name)
    }

    fn list(&self) -> Vec<DevicePublic> {
        self.lock()
            .devices
            .iter()
            .map(|device| DevicePublic {
                id: device.id.clone(),
                name: device.name.clone(),
                paired_at: device.paired_at,
                last_seen: device.last_seen,
                address: device.address.clone(),
            })
            .collect()
    }

    fn remove(&self, id: &str) -> bool {
        let mut inner = self.lock();
        let before = inner.devices.len();
        inner.devices.retain(|device| device.id != id);
        let removed = inner.devices.len() != before;
        if removed {
            self.save(&inner.devices);
        }
        removed
    }

    /// Numbers past their time go. A device approved on this PC that never
    /// came back for its key is removed too, so no key nobody holds stays valid.
    fn prune(&self, inner: &mut Inner) {
        let mut abandoned = Vec::new();
        inner.pending.retain(|pending| {
            let live = pending.created.elapsed() < CODE_LIFE;
            if !live {
                if let Some(id) = &pending.device {
                    abandoned.push(id.clone());
                }
            }
            live
        });
        if !abandoned.is_empty() {
            inner.devices.retain(|device| !abandoned.contains(&device.id));
            self.save(&inner.devices);
        }
    }

    /// Written while the caller holds the lock, so an older list never lands
    /// after a newer one.
    fn save(&self, devices: &[Device]) {
        let Ok(text) = serde_json::to_string_pretty(devices) else {
            return;
        };
        if let Err(err) = crate::paths::write_settings(&self.path, text.as_bytes()) {
            tracing::warn!("Could not save paired devices: {err}");
        }
    }
}

/// Paths a device that is not paired may still reach: pairing itself, and
/// what a cast receiver loads. Video under `/media` is outside `/api`.
fn open_path(path: &str) -> bool {
    matches!(path, "/api/pair/start" | "/api/pair/status" | "/api/pair/me" | "/api/ping")
        || path.starts_with("/api/subtitles/file/")
}

/// What a device other than this PC may ask for. Reads that show what can
/// be watched, and changes that only play, cast or keep a watchlist.
/// Everything else (settings, add-ons, repositories, the engine, updates,
/// pairing others, clearing history) is refused.
fn remote_may(method: &Method, path: &str) -> bool {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').skip(1).collect();
    match (method, parts.as_slice()) {
        // Reads, except settings forms and add-on management.
        (&Method::GET | &Method::HEAD, ["api", "addons", _, "settings"])
        | (&Method::GET | &Method::HEAD, ["api", "app"])
        | (&Method::GET | &Method::HEAD, ["api", "package-install"])
        | (&Method::GET | &Method::HEAD, ["api", "repos", ..])
        | (&Method::GET | &Method::HEAD, ["api", "metadata"])
        | (&Method::GET | &Method::HEAD, ["api", "devices", ..]) => false,
        (&Method::GET | &Method::HEAD, _) => true,
        // Watching.
        (&Method::POST, ["api", "play"])
        | (&Method::DELETE, ["api", "play", _])
        | (&Method::POST, ["api", "play", _, "seek" | "quality" | "audio"])
        | (&Method::POST, ["api", "prompt"])
        | (&Method::POST, ["api", "progress"])
        | (&Method::POST, ["api", "saved" | "info"])
        | (&Method::POST, ["api", "subtitles", "search"])
        | (&Method::PUT, ["api", "prefs"])
        | (&Method::POST | &Method::PUT | &Method::DELETE, ["api", "watchlist" | "bookmarks", _])
        | (&Method::PUT | &Method::DELETE, ["api", "history", _]) => true,
        // Casting.
        (&Method::POST, ["api", "cast", "presence"])
        | (&Method::PUT | &Method::DELETE, ["api", "cast", "session"])
        | (&Method::POST, ["api", "cast", "session", "place" | "command"])
        | (
            &Method::POST,
            ["api", "cast", "devices", _, "play" | "stop" | "pause" | "seek" | "volume" | "subtitles"],
        ) => true,
        // Pairing itself.
        (&Method::POST, ["api", "pair", "start"]) => true,
        _ => false,
    }
}

/// Refuses the API to a device that is not paired, and anything but
/// watching and casting to every device other than this PC. Pages and
/// scripts still load, so a device can show its number.
pub async fn require(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let path = request.uri().path().to_string();
    let path = path.as_str();
    if !path.starts_with("/api/") {
        return next.run(request).await;
    }
    // No address means the request did not come over the network listener.
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<SocketAddr>>().copied()
    else {
        return next.run(request).await;
    };
    // A request a proxy on this PC passed along came from somewhere else.
    if this_pc(peer, request.headers()) {
        return next.run(request).await;
    }
    let mut request = request;
    let paired = state.0.pairing.allowed(peer, request.headers());
    if !paired {
        if !open_path(path) {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "detail": NOT_PAIRED, "pair": true })),
            )
                .into_response();
        }
        request.extensions_mut().insert(Unpaired);
    }
    if !remote_may(request.method(), request.uri().path()) {
        return fail(StatusCode::FORBIDDEN, PC_ONLY).into_response();
    }
    next.run(request).await
}

/// Marks a request from a device that is not paired, which only reached an
/// open path (pairing itself, or a subtitle a cast receiver loads).
#[derive(Clone, Copy)]
pub struct Unpaired;

/// From this PC itself: a local address, and not passed along by a proxy.
pub(crate) fn this_pc(peer: SocketAddr, headers: &HeaderMap) -> bool {
    from_this_pc(peer) && !forwarded(headers)
}

/// Headers a proxy adds when it passes a request along.
pub(crate) fn forwarded(headers: &HeaderMap) -> bool {
    ["forwarded", "x-forwarded-for", "x-real-ip", "x-forwarded-host"]
        .iter()
        .any(|name| headers.contains_key(*name))
}

pub async fn me(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Json<Me> {
    Json(Me {
        paired: state.0.pairing.allowed(peer, &headers),
        this_pc: this_pc(peer, &headers),
    })
}

pub async fn start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<Started>, ApiError> {
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    state
        .0
        .pairing
        .start(peer, device_name(agent).to_string())
        .map(Json)
        .map_err(|detail| fail(StatusCode::TOO_MANY_REQUESTS, detail))
}

pub async fn status(
    State(state): State<AppState>,
    Query(query): Query<TicketQuery>,
) -> Result<Response, ApiError> {
    match state.0.pairing.status(&query.ticket) {
        Ok(None) => Ok(Json(json!({ "state": "waiting" })).into_response()),
        Ok(Some(key)) => {
            let cookie = format!(
                "{COOKIE}={key}; Path=/; Max-Age=315360000; HttpOnly; SameSite=Strict"
            );
            let mut response = Json(json!({ "state": "paired" })).into_response();
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                response.headers_mut().insert(header::SET_COOKIE, value);
            }
            Ok(response)
        }
        Err(()) => Ok(Json(json!({ "state": "expired" })).into_response()),
    }
}

pub async fn approve(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Approve>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !from_this_pc(peer) {
        return Err(fail(StatusCode::FORBIDDEN, THIS_PC_ONLY));
    }
    let name = state
        .0
        .pairing
        .approve(&body.code)
        .map_err(|detail| fail(StatusCode::BAD_REQUEST, detail))?;
    tracing::info!(device = %name, "Paired a device");
    Ok(Json(json!({ "ok": true, "name": name })))
}

pub async fn devices(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<Json<Vec<DevicePublic>>, ApiError> {
    if !from_this_pc(peer) {
        return Err(fail(StatusCode::FORBIDDEN, THIS_PC_ONLY));
    }
    Ok(Json(state.0.pairing.list()))
}

pub async fn device_remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if !from_this_pc(peer) {
        return Err(fail(StatusCode::FORBIDDEN, THIS_PC_ONLY));
    }
    if !state.0.pairing.remove(&id) {
        return Err(fail(StatusCode::NOT_FOUND, "That device is not paired."));
    }
    Ok(Json(json!({ "ok": true })))
}

fn cookie_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| (32..=128).contains(&value.len()))
}

/// A plain name for the list on this PC, from the browser's description.
fn device_name(agent: &str) -> &'static str {
    let lower = agent.to_ascii_lowercase();
    if ["smart-tv", "smarttv", "tizen", "web0s", "webos", "bravia", "android tv", "googletv", "crkey", "; aft"]
        .iter()
        .any(|word| lower.contains(word))
    {
        "TV"
    } else if lower.contains("ipad") || lower.contains("tablet") {
        "Tablet"
    } else if lower.contains("iphone") || lower.contains("android") || lower.contains("mobile") {
        "Phone"
    } else if lower.contains("windows") || lower.contains("macintosh") || lower.contains("linux") {
        "Computer"
    } else {
        "Device"
    }
}

fn digest(key: &str) -> String {
    Sha256::digest(key.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn random_key() -> Option<String> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Six digits, without the bias a plain modulo would add.
fn random_code() -> Option<String> {
    loop {
        let mut bytes = [0u8; 4];
        getrandom::getrandom(&mut bytes).ok()?;
        let value = u32::from_le_bytes(bytes);
        if value < u32::MAX - u32::MAX % 1_000_000 {
            return Some(format!("{:06}", value % 1_000_000));
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/pairing.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../tests-private/http/src/pairing_safety.rs"]
mod safety_tests;
