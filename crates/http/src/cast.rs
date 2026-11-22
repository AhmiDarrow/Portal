//! LAN cast: which address port 8490 uses, TV receivers, and the shared play job.
//!
//! Browser receivers stay on `/api/cast/receivers`. Chromecast and Vizio Cast
//! devices are discovered by `tools/cast/cast_agent.py` (pychromecast, Default
//! Media Receiver). `seen_ms` is Unix epoch milliseconds of the last presence
//! post, not an age.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

mod watch;

use portal_api::{
    CastBody, CastCommand, CastCommandPut as CommandPut, CastDevice as DeviceItem,
    CastDeviceList as DeviceList, CastDeviceView as DeviceView, CastLanPut as LanPut, CastPlace,
    CastPlacePut as PlacePut, CastPlayBody as DevicePlayBody, CastPresencePut as PresencePut,
    CastReceiver as ReceiverItem, CastReceiverList as ReceiverList, CastReplay,
    CastSessionPut as SessionPut, CastSessionView as SessionView, CastSubtitle,
    CastVolumeBody as DeviceVolumeBody, LostCast, ReplayFields, SeekBody as DeviceSeekBody,
};
use watch::{DeviceRef, DeviceState, Update, Watched};

const DISCOVER_LIMIT: Duration = Duration::from_secs(30);
const CONTROL_LIMIT: Duration = Duration::from_secs(45);

/// A scan this recent answers the next ask too.
const DISCOVERY_REUSE: Duration = Duration::from_secs(10);
/// A failed scan answers tabs that waited on it for this long, so they do
/// not each run another 30 s scan in turn.
const DISCOVERY_FAIL_REUSE: Duration = Duration::from_secs(10);
type Discovery = Result<serde_json::Value, AgentFail>;
static DISCOVERY: tokio::sync::Mutex<Option<(Instant, Discovery)>> =
    tokio::sync::Mutex::const_new(None);

fn discovery_reusable(found: &Discovery, age: Duration) -> bool {
    let limit = if found.is_ok() {
        DISCOVERY_REUSE
    } else {
        DISCOVERY_FAIL_REUSE
    };
    age < limit
}

const DEFAULT_PORT: u16 = 8490;
const RECEIVER_TTL: Duration = Duration::from_secs(45);
/// A browser TV says it is there every 5 s. One that missed four in a row
/// (turned off, tab closed) no longer holds the cast.
const SESSION_TTL: Duration = Duration::from_secs(20);
/// Browser TVs remembered at once; the oldest goes first.
const RECEIVER_LIMIT: usize = 64;
const NAME_LIMIT: usize = 200;
/// Library ids are an add-on's own links, often thousands of characters. Cut
/// short, the progress a TV reports would land on a title of its own.
const ID_LIMIT: usize = 16 * 1024;
/// Cast devices remembered across scans.
const DEVICE_LIMIT: usize = 32;

fn listen_port() -> u16 {
    std::env::var("PORTAL_PORT")
        .ok()
        .and_then(|raw| raw.trim().parse::<u16>().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_PORT)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct CastSettings {
    #[serde(default = "default_lan")]
    pub lan: bool,
}

fn default_lan() -> bool {
    true
}

impl Default for CastSettings {
    fn default() -> Self {
        Self { lan: true }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReceiverStamp {
    id: String,
    name: String,
    seen: Instant,
    seen_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceRecord {
    id: String,
    name: String,
    host: String,
    model: String,
    port: u16,
}

#[derive(Debug, Clone, PartialEq)]
struct CastSession {
    receiver_id: String,
    job_id: String,
    title: Option<String>,
    replay: Option<CastReplay>,
    place: Option<CastPlace>,
    command: Option<CastCommand>,
    next_seq: u64,
}

#[derive(Debug, PartialEq, Eq)]
enum SessionSetError {
    UnknownReceiver,
    MissingField,
}

#[derive(Debug)]
enum PlaceError {
    NoSession,
    Invalid,
}

#[derive(Debug)]
enum CommandError {
    NoSession,
    Invalid,
}

/// A position or duration the clock can show. Zero is an unknown length.
fn finite_seconds(value: f64) -> Option<f64> {
    if value.is_finite() && (0.0..=200_000.0).contains(&value) {
        Some(value)
    } else {
        None
    }
}

#[derive(Debug)]
enum PresenceError {
    Invalid,
}

pub(super) struct CastHub {
    path: PathBuf,
    settings: CastSettings,
    /// LAN flag this process actually bound. Puts compare against it.
    bound_lan: bool,
    receivers: Vec<ReceiverStamp>,
    session: Option<CastSession>,
    /// The last session that ended because its TV went quiet.
    lost: Option<LostCast>,
    /// Last Chromecast discovery, so play can reconnect by host.
    devices: Vec<DeviceRecord>,
    /// The Chromecast-type cast Portal is watching, or last watched.
    device: Option<DeviceSlot>,
    device_gen: u64,
    /// Counts subtitle picks for the device cast, so the latest one wins over
    /// an earlier one (or a subtitle following the video) still downloading.
    subtitle_asks: u64,
}

struct DeviceSlot {
    gen: u64,
    /// Sending true ends the watch.
    cancel: tokio::sync::watch::Sender<bool>,
    view: DeviceView,
}

fn same_lang(track: Option<&str>, wanted: &str) -> bool {
    track.is_some_and(|track| crate::subtitles::same_language(track, wanted))
}

/// A full (not forced) track in `lang`: the exact one ("pt-BR") before one of
/// the same family ("pt").
fn track_in<'a>(
    tracks: &'a [portal_api::SubtitleTrack],
    lang: &str,
) -> Option<&'a portal_api::SubtitleTrack> {
    let full = || tracks.iter().filter(|track| !track.forced);
    full()
        .find(|track| {
            track
                .lang
                .as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case(lang))
        })
        .or_else(|| full().find(|track| same_lang(track.lang.as_deref(), lang)))
}

/// An online subtitle is fetched and converted before a TV is given its
/// address: a broken one then fails here, where Portal can say so, instead
/// of leaving the TV silently without text, and the TV's load is quick.
async fn subtitle_ready(
    state: &super::AppState,
    job: &portal_api::PlayJob,
    subtitle: &CastSubtitle,
) -> bool {
    subtitle_check(state, job, subtitle).await.is_ok()
}

/// As `subtitle_ready`, with the subtitles service's own reason when it fails.
async fn subtitle_check(
    state: &super::AppState,
    job: &portal_api::PlayJob,
    subtitle: &CastSubtitle,
) -> Result<(), super::ApiError> {
    match subtitle {
        CastSubtitle::Embedded { .. } => Ok(()),
        CastSubtitle::Online { id, .. } => {
            let origin = job
                .position
                .filter(|at| at.is_finite() && *at > 0.0)
                .unwrap_or(0.0);
            state.0.subtitles.file(id, origin).await.map(|_| ())
        }
    }
}

/// A track tag that names no language.
fn untagged(code: &str) -> bool {
    let code = code.trim();
    code.is_empty() || code.eq_ignore_ascii_case("und")
}

/// The language a pick asks for from now on: its own, else its track's,
/// else `preferred` (the first of the subtitle settings' languages), so an
/// untagged track still leaves the next episode a language to look for.
fn pick_lang(
    choice: &CastSubtitle,
    tracks: &[portal_api::SubtitleTrack],
    preferred: Option<String>,
) -> Option<String> {
    let track = || match choice {
        CastSubtitle::Embedded { id, .. } => tracks
            .iter()
            .find(|track| track.id == *id)
            .and_then(|track| track.lang.clone()),
        CastSubtitle::Online { .. } => None,
    };
    choice
        .lang()
        .map(str::to_string)
        .filter(|code| !untagged(code))
        .or_else(|| track().filter(|code| !untagged(code)))
        .or(preferred)
}

fn preferred_lang(state: &super::AppState) -> Option<String> {
    state.0.subtitles.public().languages.into_iter().next()
}

/// The tag a receiver is given for a subtitle's language: a short BCP-47
/// tag ("en" for ffprobe's "eng"), or "und" when it is not known.
fn receiver_lang(code: Option<&str>) -> String {
    let Some(code) = code.map(str::trim).filter(|code| !untagged(code)) else {
        return "und".into();
    };
    if let Some(known) = crate::subtitles::language_code(code) {
        return known.into();
    }
    // A two-letter code Portal has no name for is still a valid tag.
    if code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
        return code.to_ascii_lowercase();
    }
    "und".into()
}

/// `id` percent-encoded for one path segment.
fn path_segment(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for byte in id.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The address a TV loads `subtitle` from for `job`, timed to its stream,
/// and the subtitle's language.
/// The address for the TV the job is cast to. A whole-title stream runs on
/// the title's own clock, whatever place the job's own stream starts at.
fn tv_subtitle_address(
    player: &portal_engine::Player,
    base: &str,
    job: &portal_api::PlayJob,
    subtitle: &CastSubtitle,
) -> Option<(String, String)> {
    subtitle_address_in(base, job, subtitle, player.cast_session(&job.id).as_deref())
}

/// `whole` is the session of the job's whole-title stream, when the TV plays that.
fn subtitle_address_in(
    base: &str,
    job: &portal_api::PlayJob,
    subtitle: &CastSubtitle,
    whole: Option<&str>,
) -> Option<(String, String)> {
    let lang = receiver_lang(subtitle.lang());
    match subtitle {
        CastSubtitle::Embedded { id, lang: wanted } => {
            let track = job
                .subtitles
                .iter()
                .find(|track| track.id == *id)
                .or_else(|| track_in(&job.subtitles, wanted.as_deref()?))?;
            track.url.starts_with("/media/").then(|| {
                let lang = match track.lang.as_deref() {
                    Some(code) if !untagged(code) => receiver_lang(Some(code)),
                    _ => lang,
                };
                let address = match whole {
                    Some(session) => format!("{base}/media/{session}/sub{:05}.vtt", track.id),
                    None => format!("{base}{}", track.url),
                };
                (address, lang)
            })
        }
        CastSubtitle::Online { id, .. } => {
            if !crate::subtitles::valid_file_id(id) {
                return None;
            }
            // Online files are shifted to the stream's start, like the dashboard's.
            let origin = job
                .position
                .filter(|at| at.is_finite() && *at > 0.0 && whole.is_none())
                .unwrap_or(0.0);
            Some((
                format!(
                    "{base}/api/subtitles/file/{}?offset={origin:.3}",
                    path_segment(id)
                ),
                lang,
            ))
        }
    }
}

fn session_view(session: &CastSession) -> SessionView {
    let mut view = SessionView {
        receiver_id: Some(session.receiver_id.clone()),
        job_id: Some(session.job_id.clone()),
        title: session.title.clone(),
        kind: None,
        id: None,
        file: None,
        season: None,
        episode: None,
        art: None,
        show_title: None,
        show_id: None,
        place: session.place.clone(),
        command: session.command.clone(),
        lost: None,
    };
    if let Some(replay) = &session.replay {
        view.kind = Some(replay.kind.clone());
        view.id = Some(replay.id.clone());
        view.file = Some(replay.file.clone());
        view.title = session
            .title
            .clone()
            .filter(|text| !text.is_empty())
            .or_else(|| Some(replay.title.clone()));
        view.season = replay.season;
        view.episode = replay.episode;
        view.art = replay.art.clone();
        view.show_title = replay.show_title.clone();
        view.show_id = replay.show_id.clone();
    }
    view
}

impl CastHub {
    pub(super) fn open(dir: &Path) -> Self {
        let path = dir.join("cast.json");
        let settings = if path.is_file() {
            match std::fs::read_to_string(&path) {
                Ok(raw) => settings_from_text(Some(&raw)),
                Err(err) => {
                    // Unreadable is not "never chosen": stay off the network.
                    tracing::warn!("cast settings were not read, so Portal stays on this PC: {err}");
                    CastSettings { lan: false }
                }
            }
        } else {
            let settings = CastSettings::default();
            if let Err(err) = save_cast_settings(&path, &settings) {
                tracing::warn!("cast settings were not created: {err}");
            }
            settings
        };
        Self {
            path,
            bound_lan: settings.lan,
            settings,
            receivers: Vec::new(),
            session: None,
            lost: None,
            devices: Vec::new(),
            device: None,
            device_gen: 0,
            subtitle_asks: 0,
        }
    }

    pub(super) fn bind_addr(&self) -> String {
        listen_addr(self.settings.lan)
    }

    /// The address TVs can use now: what this process is listening on, not
    /// a saved setting that only applies after a restart.
    pub(super) fn suggested_url(&self) -> Option<String> {
        cast_url(self.bound_lan, detect_lan_ipv4().as_deref())
    }

    /// The address a particular TV should use: this PC's on the route to it.
    fn url_for_device(&self, endpoint: Option<&DeviceEndpoint>) -> Option<String> {
        let host = endpoint.map(|endpoint| endpoint.host.as_str());
        cast_url(self.bound_lan, lan_ipv4_toward(host).as_deref())
    }

    fn snapshot(&mut self, restart_needed: Option<bool>) -> CastBody {
        let now = Instant::now();
        self.expire_session(now);
        let receivers = self.receiver_items(now);
        let session = self.session.as_ref().map(session_view);
        let restart_needed = restart_needed.or_else(|| {
            let pending = self::restart_needed(self.bound_lan, self.settings.lan);
            pending.then_some(true)
        });
        CastBody {
            lan: self.settings.lan,
            bind: listen_addr(self.settings.lan),
            url: self.suggested_url(),
            receivers,
            session,
            restart_needed,
        }
    }

    fn put_lan(&mut self, lan: bool) -> std::io::Result<CastBody> {
        let next = CastSettings { lan };
        save_cast_settings(&self.path, &next)?;
        let restart = restart_needed(self.bound_lan, lan);
        self.settings = next;
        Ok(self.snapshot(Some(restart)))
    }

    fn presence(&mut self, id: &str, name: &str, now: Instant) -> Result<(), PresenceError> {
        let id = id.trim();
        let name = clip(name.trim());
        if id.is_empty() || id.len() > NAME_LIMIT {
            return Err(PresenceError::Invalid);
        }
        touch_receiver(
            &mut self.receivers,
            id.to_string(),
            name.to_string(),
            now,
            unix_ms(),
            RECEIVER_TTL,
        );
        Ok(())
    }

    fn receiver_items(&mut self, now: Instant) -> Vec<ReceiverItem> {
        expire_receivers(&mut self.receivers, now, RECEIVER_TTL);
        self.receivers
            .iter()
            .map(|row| ReceiverItem {
                id: row.id.clone(),
                name: row.name.clone(),
                seen_ms: row.seen_ms,
            })
            .collect()
    }

    fn session_view(&self) -> SessionView {
        self.session
            .as_ref()
            .map(session_view)
            .unwrap_or_else(|| SessionView {
                lost: self.lost.clone(),
                ..SessionView::default()
            })
    }

    /// A browser TV that stopped saying it is there (turned off, tab closed)
    /// no longer holds the cast. Its last reported place is kept, so the
    /// dashboard resumes from there rather than from a guess.
    fn expire_session(&mut self, now: Instant) {
        let Some(session) = &self.session else {
            return;
        };
        let live = self.receivers.iter().any(|row| {
            row.id == session.receiver_id && now.saturating_duration_since(row.seen) < SESSION_TTL
        });
        if live {
            return;
        }
        if let Some(session) = self.session.take() {
            self.lost = Some(LostCast {
                receiver_id: session.receiver_id,
                job_id: session.job_id,
                place: session.place,
            });
        }
    }

    fn put_session(
        &mut self,
        receiver_id: &str,
        job_id: &str,
        title: Option<String>,
        replay: Option<CastReplay>,
        now: Instant,
    ) -> Result<(), SessionSetError> {
        expire_receivers(&mut self.receivers, now, RECEIVER_TTL);
        let mut session = assign_session(&self.receivers, receiver_id, job_id, title, replay)?;
        // The TV announces the same job again as it plays. Keep the place and
        // any command still waiting. A new job starts from the beginning.
        if let Some(existing) = &self.session {
            if existing.receiver_id == session.receiver_id && existing.job_id == session.job_id {
                session.place = existing.place.clone();
                session.command = existing.command.clone();
                session.next_seq = existing.next_seq;
            }
        }
        self.session = Some(session);
        self.lost = None;
        Ok(())
    }

    fn put_place(&mut self, position: f64, duration: f64, paused: bool) -> Result<(), PlaceError> {
        let position = finite_seconds(position).ok_or(PlaceError::Invalid)?;
        let duration = finite_seconds(duration).ok_or(PlaceError::Invalid)?;
        let session = self.session.as_mut().ok_or(PlaceError::NoSession)?;
        session.place = Some(CastPlace {
            position,
            duration,
            paused,
            updated_ms: unix_ms(),
        });
        Ok(())
    }

    fn put_command(&mut self, action: &str, seconds: Option<f64>) -> Result<(), CommandError> {
        let action = action.trim();
        let seconds = match action {
            "play" | "pause" => None,
            "seek" => {
                Some(finite_seconds(seconds.unwrap_or(f64::NAN)).ok_or(CommandError::Invalid)?)
            }
            _ => return Err(CommandError::Invalid),
        };
        self.expire_session(Instant::now());
        let session = self.session.as_mut().ok_or(CommandError::NoSession)?;
        session.next_seq = session.next_seq.saturating_add(1);
        session.command = Some(CastCommand {
            seq: session.next_seq,
            action: action.to_string(),
            seconds,
        });
        Ok(())
    }

    fn delete_session(&mut self) {
        clear_session(&mut self.session);
        self.lost = None;
    }

    /// mDNS often misses a device in one scan; a device found before keeps
    /// its address, so controls for a TV that is still casting connect
    /// directly instead of searching the network again each time.
    fn remember_devices(&mut self, devices: Vec<DeviceRecord>) {
        let mut merged = devices;
        for old in self.devices.drain(..) {
            if merged.len() >= DEVICE_LIMIT {
                break;
            }
            if !merged.iter().any(|row| row.id == old.id) {
                merged.push(old);
            }
        }
        self.devices = merged;
    }

    fn device_name(&self, id: &str) -> Option<String> {
        self.devices
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.name.clone())
    }

    /// A new watched cast; any earlier watch ends.
    fn begin_device(&mut self, view: DeviceView) -> (u64, tokio::sync::watch::Receiver<bool>) {
        if let Some(slot) = &self.device {
            slot.cancel.send_replace(true);
        }
        self.device_gen += 1;
        let (cancel, receiver) = tokio::sync::watch::channel(false);
        self.device = Some(DeviceSlot {
            gen: self.device_gen,
            cancel,
            view,
        });
        (self.device_gen, receiver)
    }

    fn active_device(&mut self, device_id: &str) -> Option<&mut DeviceSlot> {
        self.device
            .as_mut()
            .filter(|slot| slot.view.active && slot.view.device_id.as_deref() == Some(device_id))
    }

    /// Portal stopped the device: the watch ends and plays nothing next.
    fn set_device_origin(&mut self, device_id: &str, origin: f64) {
        if let Some(slot) = self.active_device(device_id) {
            slot.view.origin = origin;
        }
    }

    fn end_device(&mut self, device_id: &str) {
        if let Some(slot) = self.active_device(device_id) {
            slot.cancel.send_replace(true);
            slot.view.active = false;
            slot.view.state = Some(DeviceState::Stopped);
            slot.view.note = None;
            slot.view.updated_ms = unix_ms();
        }
    }

    /// Pause and resume show at once; the next status line confirms them.
    /// Records the subtitle the TV now shows for `job_id`, and the language
    /// wanted from now on (`lang`, else the subtitle's own).
    fn set_device_subtitle(
        &mut self,
        device_id: &str,
        subtitle: Option<CastSubtitle>,
        job_id: Option<String>,
        lang: Option<String>,
    ) {
        if let Some(slot) = self.active_device(device_id) {
            // A change that finished after a seek or the next episode moved
            // the cast on describes a job the TV no longer plays.
            if job_id.is_some() && slot.view.job_id != job_id {
                return;
            }
            slot.view.subtitle_job = subtitle.as_ref().and(job_id);
            // A pick sets the language wanted from now on, or keeps the one
            // before when it names none; turning them off clears it.
            slot.view.subtitle_lang = match &subtitle {
                Some(choice) => lang
                    .or_else(|| choice.lang().map(str::to_string))
                    .or_else(|| slot.view.subtitle_lang.take()),
                None => None,
            };
            slot.view.subtitle = subtitle;
            slot.view.subtitle_pending = false;
        }
    }

    /// A new job (the next episode, or a cast whose subtitle is still being
    /// readied) starts without one; the wanted language stays, and one in it
    /// is on its way.
    fn pend_device_subtitle(&mut self, device_id: &str, job_id: &str, lang: Option<String>) {
        if let Some(slot) = self.active_device(device_id) {
            if slot.view.job_id.as_deref() != Some(job_id) {
                return;
            }
            slot.view.subtitle = None;
            slot.view.subtitle_job = Some(job_id.to_string());
            if lang.is_some() {
                slot.view.subtitle_lang = lang;
            }
            slot.view.subtitle_pending = true;
        }
    }

    /// Nothing more is coming for `job_id`: what was being found for pick
    /// `ask` was not. A later pick has its own say.
    fn settle_device_subtitle(&mut self, device_id: &str, job_id: &str, ask: u64) {
        if self.subtitle_asks != ask {
            return;
        }
        if let Some(slot) = self.active_device(device_id) {
            if slot.view.job_id.as_deref() == Some(job_id) {
                slot.view.subtitle_pending = false;
            }
        }
    }

    /// A new subtitle pick, which replaces any subtitle still on its way;
    /// answers its number, for `latest_subtitle_ask`.
    fn ask_subtitle(&mut self) -> u64 {
        self.subtitle_asks += 1;
        if let Some(slot) = self.device.as_mut() {
            slot.view.subtitle_pending = false;
        }
        self.subtitle_asks
    }

    fn latest_subtitle_ask(&self) -> u64 {
        self.subtitle_asks
    }

    /// The subtitle state, to carry over a watch that starts afresh.
    fn device_subtitle_state(&self) -> CarriedSubtitle {
        self.device
            .as_ref()
            .map_or_else(CarriedSubtitle::default, |slot| CarriedSubtitle {
                subtitle: slot.view.subtitle.clone(),
                lang: slot.view.subtitle_lang.clone(),
                pending: slot.view.subtitle_pending,
            })
    }

    fn restore_device_subtitle(&mut self, device_id: &str, state: CarriedSubtitle, job_id: &str) {
        if let Some(slot) = self.active_device(device_id) {
            slot.view.subtitle_job = Some(job_id.to_string());
            slot.view.subtitle = state.subtitle;
            slot.view.subtitle_lang = state.lang;
            slot.view.subtitle_pending = state.pending;
        }
    }

    /// The TV's volume as last seen, for a watch of the same device that starts afresh.
    fn device_volume(&self, device_id: &str) -> (Option<f64>, Option<bool>) {
        self.device
            .as_ref()
            .filter(|slot| slot.view.device_id.as_deref() == Some(device_id))
            .map_or((None, None), |slot| (slot.view.volume, slot.view.muted))
    }

    /// A volume change shows at once; the next status line confirms it.
    fn set_device_volume(&mut self, device_id: &str, level: Option<f64>, muted: Option<bool>) {
        if let Some(slot) = self.active_device(device_id) {
            if level.is_some() {
                slot.view.volume = level;
            }
            if muted.is_some() {
                slot.view.muted = muted;
            }
        }
    }

    fn mark_device(&mut self, device_id: &str, state: DeviceState) {
        if let Some(slot) = self.active_device(device_id) {
            slot.view.state = Some(state);
            slot.view.updated_ms = unix_ms();
        }
    }

    fn apply_device_update(&mut self, gen: u64, update: Update) {
        let Some(slot) = self.device.as_mut().filter(|slot| slot.gen == gen) else {
            return;
        };
        apply_update(&mut slot.view, update, unix_ms());
    }

    fn device_view(&self) -> DeviceView {
        self.device
            .as_ref()
            .map(|slot| slot.view.clone())
            .unwrap_or_else(DeviceView::default)
    }

    fn device_endpoint(&self, id: &str) -> Option<DeviceEndpoint> {
        self.devices
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| {
                let host = clean_host(&row.host)?;
                Some(DeviceEndpoint {
                    host,
                    port: row.port,
                    name: row.name.clone(),
                    model: row.model.clone(),
                })
            })
    }
}

fn apply_update(view: &mut DeviceView, update: Update, now_ms: u64) {
    if !view.active {
        return;
    }
    view.updated_ms = now_ms;
    match update {
        Update::Status {
            state,
            position,
            duration,
            volume,
            muted,
        } => {
            view.state = Some(state);
            if position.is_some() {
                view.position = position;
            }
            if duration.is_some() {
                view.duration = duration;
            }
            if volume.is_some() {
                view.volume = volume;
            }
            if muted.is_some() {
                view.muted = muted;
            }
        }
        Update::Next {
            job_id,
            title,
            item,
        } => {
            view.job_id = Some(job_id);
            view.title = Some(clean_label(&title)).filter(|text| !text.is_empty());
            view.item = item;
            view.state = Some(DeviceState::Next);
            view.position = None;
            view.duration = None;
            view.origin = 0.0;
            view.note = None;
        }
        Update::Casting(watched) => {
            view.job_id = Some(watched.job_id.clone());
            if let Some(item) = &watched.item {
                view.title = Some(item.title.clone());
            }
            view.item = watched.item.clone();
            view.state = Some(DeviceState::Starting);
            view.origin = watched.origin;
            view.position = Some(watched.origin);
            view.duration = watched.job_duration;
        }
        Update::Ended { state, note } => {
            view.active = false;
            view.state = Some(state);
            view.note = Some(note);
        }
    }
}

/// The subtitle state a watch that starts afresh (after a seek) carries over.
#[derive(Debug, Clone, Default, PartialEq)]
struct CarriedSubtitle {
    subtitle: Option<CastSubtitle>,
    lang: Option<String>,
    pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceEndpoint {
    host: String,
    port: u16,
    name: String,
    model: String,
}

fn lock_cast(state: &super::AppState) -> std::sync::MutexGuard<'_, CastHub> {
    state
        .0
        .cast
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

pub(super) async fn cast_get(State(state): State<super::AppState>) -> Json<CastBody> {
    Json(lock_cast(&state).snapshot(None))
}

pub(super) async fn cast_put(
    State(state): State<super::AppState>,
    Json(body): Json<LanPut>,
) -> Result<Json<CastBody>, super::ApiError> {
    lock_cast(&state)
        .put_lan(body.lan)
        .map(Json)
        .map_err(|err| {
            tracing::warn!("cast settings were not saved: {err}");
            super::fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not save cast settings.",
            )
        })
}

pub(super) async fn cast_presence(
    State(state): State<super::AppState>,
    Json(body): Json<PresencePut>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    lock_cast(&state)
        .presence(&body.id, &body.name, Instant::now())
        .map_err(|_| super::fail(StatusCode::BAD_REQUEST, "A receiver id is required."))?;
    Ok(Json(portal_api::OkBody { ok: true }))
}

pub(super) async fn cast_receivers(State(state): State<super::AppState>) -> Json<ReceiverList> {
    Json(ReceiverList {
        items: lock_cast(&state).receiver_items(Instant::now()),
    })
}

pub(super) async fn cast_session_get(State(state): State<super::AppState>) -> Json<SessionView> {
    let mut hub = lock_cast(&state);
    hub.expire_session(Instant::now());
    Json(hub.session_view())
}

pub(super) async fn cast_session_put(
    State(state): State<super::AppState>,
    Json(body): Json<SessionPut>,
) -> Result<Json<SessionView>, super::ApiError> {
    let mut hub = lock_cast(&state);
    let replay = cast_replay(&body.replay, body.title.as_deref());
    match hub.put_session(
        &body.receiver_id,
        &body.job_id,
        body.title,
        replay,
        Instant::now(),
    ) {
        Ok(()) => Ok(Json(hub.session_view())),
        Err(SessionSetError::UnknownReceiver) => Err(super::fail(
            StatusCode::NOT_FOUND,
            "That receiver is not on the network.",
        )),
        Err(SessionSetError::MissingField) => Err(super::fail(
            StatusCode::BAD_REQUEST,
            "A receiver and a play job are required.",
        )),
    }
}

fn cast_replay(body: &ReplayFields, title: Option<&str>) -> Option<CastReplay> {
    let kind = body.kind.as_deref()?.trim();
    let id = body.id.as_deref()?.trim();
    let file = body.file.as_deref()?.trim();
    if kind.is_empty() || id.is_empty() || file.is_empty() || id.len() > ID_LIMIT {
        return None;
    }
    if kind != "movie" && kind != "episode" {
        return None;
    }
    let title = title
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Playback");
    Some(CastReplay {
        kind: kind.to_string(),
        id: id.to_string(),
        file: file.to_string(),
        title: clip(title).to_string(),
        // Season 0 is a special. Episode 0 is not its own episode.
        season: body.season,
        episode: body.episode.filter(|value| *value > 0),
        art: body
            .art
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        show_title: body
            .show_title
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
            .map(|value| clip(value).to_string()),
        show_id: body
            .show_id
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty() && value.len() <= ID_LIMIT)
            .map(str::to_string),
    })
}

pub(super) async fn cast_session_delete(
    State(state): State<super::AppState>,
) -> Json<portal_api::OkBody> {
    lock_cast(&state).delete_session();
    Json(portal_api::OkBody { ok: true })
}

pub(super) async fn cast_session_place(
    State(state): State<super::AppState>,
    Json(body): Json<PlacePut>,
) -> Result<Json<SessionView>, super::ApiError> {
    let mut hub = lock_cast(&state);
    match hub.put_place(body.position, body.duration, body.paused) {
        Ok(()) => Ok(Json(hub.session_view())),
        Err(PlaceError::NoSession) => {
            Err(super::fail(StatusCode::NOT_FOUND, "Nothing is casting."))
        }
        Err(PlaceError::Invalid) => Err(super::fail(
            StatusCode::BAD_REQUEST,
            "That place in the video is not valid.",
        )),
    }
}

pub(super) async fn cast_session_command(
    State(state): State<super::AppState>,
    Json(body): Json<CommandPut>,
) -> Result<Json<SessionView>, super::ApiError> {
    let mut hub = lock_cast(&state);
    match hub.put_command(&body.action, body.seconds) {
        Ok(()) => Ok(Json(hub.session_view())),
        Err(CommandError::NoSession) => {
            Err(super::fail(StatusCode::NOT_FOUND, "Nothing is casting."))
        }
        Err(CommandError::Invalid) => Err(super::fail(
            StatusCode::BAD_REQUEST,
            "That cast control is not valid.",
        )),
    }
}

pub(super) async fn cast_devices(
    State(state): State<super::AppState>,
) -> Result<Json<DeviceList>, super::ApiError> {
    // One scan at a time; tabs asking together share its answer.
    let found = {
        let mut last = DISCOVERY.lock().await;
        match last.as_ref() {
            Some((at, found)) if discovery_reusable(found, at.elapsed()) => found.clone(),
            _ => {
                let found = run_agent(discover_args(), DISCOVER_LIMIT).await;
                *last = Some((Instant::now(), found.clone()));
                found
            }
        }
    }
    .map_err(agent_error)?;
    let records = devices_from_value(&found);
    let items = records
        .iter()
        .map(|row| DeviceItem {
            id: row.id.clone(),
            name: row.name.clone(),
            host: row.host.clone(),
            model: row.model.clone(),
        })
        .collect();
    lock_cast(&state).remember_devices(records);
    Ok(Json(DeviceList { items }))
}

pub(super) async fn cast_device_play(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<DevicePlayBody>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let id = clean_id(&id).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    let endpoint = lock_cast(&state).device_endpoint(&id);
    let url = body
        .url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let Some(url) = url else {
        let args = control_args("resume", &id, endpoint.as_ref(), None);
        run_agent(args, CONTROL_LIMIT).await.map_err(agent_error)?;
        lock_cast(&state).mark_device(&id, DeviceState::Playing);
        return Ok(Json(portal_api::OkBody { ok: true }));
    };
    let url = cast_media_url(url).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    let url = toward_device(&state, endpoint.as_ref(), url);
    let title = body
        .title
        .as_deref()
        .map(clean_label)
        .filter(|value| !value.is_empty());
    let thumb = body.thumb.as_deref().and_then(cast_thumb_url);
    let start = body
        .start
        .filter(|at| at.is_finite() && (0.0..=86_400.0).contains(at))
        .unwrap_or(0.0);
    let job_id = body
        .job_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= NAME_LIMIT);
    let job = job_id.and_then(|job_id| state.0.player.job(job_id));
    let (url, start) = cast_target(&state.0.player, &url, job.as_ref(), start).await?;
    let base = lock_cast(&state).url_for_device(endpoint.as_ref());
    let lang = match (&body.subtitle, &job) {
        (Some(choice), job) => pick_lang(
            choice,
            job.as_ref().map_or(&[][..], |job| job.subtitles.as_slice()),
            preferred_lang(&state),
        ),
        (None, _) => None,
    };
    // A subtitle that is ready in a moment goes with the cast; a slower one
    // follows once the video plays, so the TV never waits on a download.
    let mut later = None;
    let subtitle = match (&body.subtitle, &job, &base) {
        (Some(subtitle), Some(job), Some(base)) => {
            match tokio::time::timeout(QUICK_SUBTITLE, subtitle_ready(&state, job, subtitle)).await
            {
                Ok(true) => match tv_subtitle_address(&state.0.player, base, job, subtitle) {
                    Some(address) => Some((subtitle.clone(), address)),
                    // The video's own tracks are not read yet.
                    None => {
                        later = Some(Want::Choice(subtitle.clone()));
                        None
                    }
                },
                // A file that failed is not the last word: one in the same
                // language is looked for once the video plays.
                Ok(false) => {
                    later = lang.clone().map(Want::Language);
                    None
                }
                Err(_) => {
                    later = Some(Want::Choice(subtitle.clone()));
                    None
                }
            }
        }
        _ => None,
    };
    let args = play_args(
        &id,
        endpoint.as_ref(),
        &url,
        title.as_deref(),
        thumb.as_deref(),
        start,
        subtitle
            .as_ref()
            .map(|(_, (address, lang))| (address.as_str(), lang.as_str())),
    );
    run_agent(args, CONTROL_LIMIT).await.map_err(agent_error)?;
    if let Some(job_id) = job_id {
        let item = cast_replay(&body.replay, title.as_deref());
        let name = lock_cast(&state)
            .device_name(&id)
            .unwrap_or_else(|| "Cast device".into());
        let device = DeviceRef {
            id: id.clone(),
            name,
            endpoint,
        };
        watch_device(&state, device.clone(), url, job_id, title, item, start);
        // A new cast replaces whatever an earlier one was still sending.
        lock_cast(&state).ask_subtitle();
        match (later, job, base) {
            (Some(want), Some(job), Some(base)) => {
                lock_cast(&state).pend_device_subtitle(&id, job_id, lang);
                send_subtitle_later(state.clone(), device, job, base, want);
            }
            _ => lock_cast(&state).set_device_subtitle(
                &id,
                subtitle.map(|(choice, _)| choice),
                Some(job_id.to_string()),
                lang,
            ),
        }
    }
    Ok(Json(portal_api::OkBody { ok: true }))
}

/// Starts watching a device cast of `job_id`, replacing any earlier watch.
fn watch_device(
    state: &super::AppState,
    device: DeviceRef,
    media_url: String,
    job_id: &str,
    title: Option<String>,
    item: Option<CastReplay>,
    start: f64,
) {
    let Some(job) = state.0.player.job(job_id) else {
        return;
    };
    let quality = state
        .0
        .player
        .quality(&job.id)
        .unwrap_or_else(|| crate::prefs::current(state).quality);
    let origin = job
        .position
        .filter(|at| at.is_finite() && *at >= 0.0 && !is_cast_playlist(&media_url))
        .unwrap_or(0.0);
    let (volume, muted) = lock_cast(state).device_volume(&device.id);
    let view = DeviceView {
        active: true,
        device_id: Some(device.id.clone()),
        device_name: Some(device.name.clone()),
        job_id: Some(job.id.clone()),
        title: title.or_else(|| job.title.clone()),
        item: item.clone(),
        state: Some(DeviceState::Starting),
        position: Some(origin + start),
        subtitle: None,
        subtitle_job: None,
        subtitle_lang: None,
        subtitle_pending: false,
        volume,
        muted,
        duration: job.duration,
        origin,
        note: None,
        updated_ms: unix_ms(),
    };
    let watched = Watched {
        device,
        job_id: job.id.clone(),
        media_url,
        origin,
        job_duration: job.duration,
        item,
        next: job.next.clone(),
        quality,
    };
    let (gen, cancel) = lock_cast(state).begin_device(view);
    let backend = AppBackend {
        state: state.clone(),
    };
    let cast_job = watched.job_id.clone();
    tokio::spawn(async move {
        let outcome = watch::drive(&backend, gen, watched, cancel).await;
        tracing::info!("device cast watch ended: {}", outcome_label(&outcome));
        // No TV plays the job now, unless a new cast of it has just begun.
        let view = lock_cast(&backend.state).device_view();
        if !(view.active && view.job_id.as_deref() == Some(cast_job.as_str())) {
            backend.state.0.player.cast_rest(&cast_job);
        }
    });
}

fn outcome_label(outcome: &watch::Outcome) -> String {
    match outcome {
        watch::Outcome::Cancelled => "stopped from Portal".into(),
        watch::Outcome::Ended { ending, chained } => format!("{ending:?} after {chained} next"),
        watch::Outcome::Done { chained, autoplay } => {
            format!("finished after {chained} next (autoplay {autoplay})")
        }
        watch::Outcome::NextFailed { chained, .. } => {
            format!("next episode failed after {chained} next")
        }
    }
}

pub(super) async fn cast_device_get(State(state): State<super::AppState>) -> Json<DeviceView> {
    Json(lock_cast(&state).device_view())
}

pub(super) async fn cast_device_stop(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    // The watch ends first, so a stop never turns into the next episode.
    if let Ok(clean) = clean_id(&id) {
        lock_cast(&state).end_device(&clean);
    }
    device_control(&state, &id, "stop", None).await
}

pub(super) async fn cast_device_pause(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let done = device_control(&state, &id, "pause", None).await?;
    if let Ok(clean) = clean_id(&id) {
        lock_cast(&state).mark_device(&clean, DeviceState::Paused);
    }
    Ok(done)
}

/// `POST /api/cast/devices/{id}/volume`: sets the TV's own volume (0 to 1)
/// and mute, either or both.
pub(super) async fn cast_device_volume(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<DeviceVolumeBody>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let level = volume_level(body.level)
        .ok_or_else(|| super::fail(StatusCode::BAD_REQUEST, "That volume is not valid."))?;
    if level.is_none() && body.muted.is_none() {
        return Err(super::fail(
            StatusCode::BAD_REQUEST,
            "That volume is not valid.",
        ));
    }
    let id = clean_id(&id).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    let endpoint = lock_cast(&state).device_endpoint(&id);
    let args = volume_args(&id, endpoint.as_ref(), level, body.muted);
    run_agent(args, CONTROL_LIMIT).await.map_err(agent_error)?;
    lock_cast(&state).set_device_volume(&id, level, body.muted);
    Ok(Json(portal_api::OkBody { ok: true }))
}

/// A volume within 0 to 1; None for one that is not a number.
fn volume_level(level: Option<f64>) -> Option<Option<f64>> {
    match level {
        Some(level) if !level.is_finite() => None,
        level => Some(level.map(|level| level.clamp(0.0, 1.0))),
    }
}

pub(super) async fn cast_device_seek(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<DeviceSeekBody>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    if !body.seconds.is_finite() || body.seconds < 0.0 || body.seconds > 86_400.0 {
        return Err(super::fail(
            StatusCode::BAD_REQUEST,
            "That time is not valid.",
        ));
    }
    let view = lock_cast(&state).device_view();
    let watched_job = view
        .job_id
        .clone()
        .filter(|_| view.active && view.device_id.as_deref() == Some(id.as_str()));
    // `seconds` is a place in the title. The TV's stream starts at the view's
    // origin, which only Portal knows for sure right after a restart.
    let relative = (body.seconds - view.origin).max(0.0);
    if let Some(job_id) = watched_job {
        // While a seek is moving the TV to a new stream, the old one is going away.
        let Some(_moving) = SeekGuard::take() else {
            return Err(super::fail(
                StatusCode::CONFLICT,
                "Portal is still moving the TV. Try again in a moment.",
            ));
        };
        use portal_api::JobState;
        match state.0.player.job(&job_id).map(|job| job.state) {
            Some(JobState::Ready) => {}
            Some(JobState::Working | JobState::Prompt) => {
                return Err(super::fail(
                    StatusCode::CONFLICT,
                    "The video is still loading. Try again in a moment.",
                ));
            }
            // The job is gone or failed; the TV may still have the video, so ask it directly.
            _ => return device_control(&state, &id, "seek", Some(relative)).await,
        }
        // A whole-title stream holds every place: the TV asks for the part
        // and Portal makes it.
        if state.0.player.cast_session(&job_id).is_some() {
            return device_control(&state, &id, "seek", Some(relative)).await;
        }
        // Before the stream's start, or past what is transcoded, needs a new stream from there.
        if body.seconds < view.origin || !stream_reaches(&state.0.player, &job_id, relative) {
            return seek_ahead(&state, &id, &job_id, &view, body.seconds, true).await;
        }
        // Transcoded, but past the end of the file the TV was given.
        if !cast_file_reaches(&state.0.player, &job_id, relative) {
            return seek_ahead(&state, &id, &job_id, &view, body.seconds, false).await;
        }
        return device_control(&state, &id, "seek", Some(relative)).await;
    }
    device_control(&state, &id, "seek", Some(relative)).await
}

/// One device seek at a time: a second one during a restart would go to the
/// stream being replaced and be lost.
struct SeekGuard;

static SEEKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

impl SeekGuard {
    fn take() -> Option<Self> {
        use std::sync::atomic::Ordering;
        SEEKING
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| SeekGuard)
    }
}

impl Drop for SeekGuard {
    fn drop(&mut self) {
        SEEKING.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// `POST /api/cast/devices/{id}/subtitles`: shows a subtitle on the cast, or
/// none for `null`. The TV keeps it through seeks, and the next episode gets
/// one in the same language.
pub(super) async fn cast_device_subtitles(
    State(state): State<super::AppState>,
    UrlPath(id): UrlPath<String>,
    Json(subtitle): Json<Option<CastSubtitle>>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let id = clean_id(&id).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    let (view, endpoint, base) = {
        let hub = lock_cast(&state);
        let endpoint = hub.device_endpoint(&id);
        let base = hub.url_for_device(endpoint.as_ref());
        (hub.device_view(), endpoint, base)
    };
    let job_id = view
        .job_id
        .clone()
        .filter(|_| view.active && view.device_id.as_deref() == Some(id.as_str()))
        .ok_or_else(|| super::fail(StatusCode::CONFLICT, "Portal is not casting to that TV."))?;
    let ok = || Ok(Json(portal_api::OkBody { ok: true }));
    let ask = lock_cast(&state).ask_subtitle();
    let job = state.0.player.job(&job_id);
    let lang = subtitle.as_ref().and_then(|choice| {
        let tracks = job.as_ref().map_or(&[][..], |job| job.subtitles.as_slice());
        pick_lang(choice, tracks, preferred_lang(&state))
    });
    // The TV still plays the last episode while the next one starts: the
    // pick is only recorded, and goes with the new episode when it is cast.
    if view.state == Some(DeviceState::Next) {
        lock_cast(&state).set_device_subtitle(&id, subtitle, Some(job_id), lang);
        return ok();
    }
    let not_ready = || {
        super::fail(
            StatusCode::NOT_FOUND,
            "That subtitle is not ready to show on the TV.",
        )
    };
    if let Some(choice) = &subtitle {
        let ready = match &job {
            Some(job) => subtitle_check(&state, job, choice).await,
            None => Err(not_ready()),
        };
        // A later pick made during the download wins; this one is dropped.
        if lock_cast(&state).latest_subtitle_ask() != ask {
            return ok();
        }
        // A problem with the file itself is 422, apart from a TV that did not
        // answer (5xx), so the dashboard knows to try another file.
        ready.map_err(|(code, body)| {
            let code = if code.is_client_error() {
                code
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            (code, body)
        })?;
    }
    // Only the command to the TV waits its turn with seeks; the download above did not.
    let Some(_moving) = SeekGuard::take() else {
        return Err(super::fail(
            StatusCode::CONFLICT,
            "Portal is still moving the TV. Try again in a moment.",
        ));
    };
    // The next episode may have started during the download.
    let now = lock_cast(&state).device_view();
    if !now.active || now.job_id.as_deref() != Some(job_id.as_str()) {
        return Err(super::fail(
            StatusCode::CONFLICT,
            "The TV moved on. Pick the subtitle again.",
        ));
    }
    // A seek during the download may have restarted the stream at a new
    // origin, so the address is timed to the job as it is now.
    let address = match &subtitle {
        None => None,
        Some(choice) => {
            let found = state
                .0
                .player
                .job(&job_id)
                .zip(base.as_ref())
                .and_then(|(job, base)| tv_subtitle_address(&state.0.player, base, &job, choice));
            Some(found.ok_or_else(not_ready)?)
        }
    };
    let mut args = vec!["subtitles".to_string(), "--id".into(), id.clone()];
    push_endpoint(&mut args, endpoint.as_ref());
    if let Some((address, lang)) = &address {
        args.extend([
            "--subtitles".into(),
            address.clone(),
            format!("--subtitles-lang={lang}"),
        ]);
    }
    args.extend(["--timeout".into(), "20".into()]);
    run_agent(args, CONTROL_LIMIT).await.map_err(agent_error)?;
    lock_cast(&state).set_device_subtitle(&id, subtitle, Some(job_id), lang);
    ok()
}

/// Slack below the transcoded end, so the TV is not sent to the very edge.
const SEEK_MARGIN: f64 = 8.0;

/// Whether the job's stream is transcoded far enough for the device to seek to `at`.
fn stream_reaches(player: &portal_engine::Player, job_id: &str, at: f64) -> bool {
    let Some(session) = player
        .job(job_id)
        .and_then(|job| job.playlist)
        .and_then(|path| playlist_session(&path))
    else {
        return true;
    };
    let text = player
        .session_file(&session, "index.m3u8")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok());
    text.is_none_or(|text| playlist_reaches(&text, at))
}

/// Whether the cast file made for the job's stream holds `at`. A stream with
/// no cast file is not held back.
fn cast_file_reaches(player: &portal_engine::Player, job_id: &str, at: f64) -> bool {
    let span = player
        .job(job_id)
        .and_then(|job| job.playlist)
        .and_then(|path| playlist_session(&path))
        .and_then(|session| player.session_file(&session, "index.m3u8").ok())
        .and_then(|playlist| read_span(&playlist.with_file_name("cast.span")));
    span.is_none_or(|span| at <= span - SPAN_SLACK)
}

fn playlist_session(path: &str) -> Option<String> {
    let rest = path.split(['?', '#']).next()?.strip_prefix("/media/")?;
    let (session, name) = rest.split_once('/')?;
    (name == "index.m3u8" && !session.is_empty()).then(|| session.to_string())
}

fn playlist_reaches(text: &str, at: f64) -> bool {
    if playlist_ended(text) {
        return true;
    }
    at <= playlist_coverage(text) - SEEK_MARGIN
}

fn playlist_ended(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "#EXT-X-ENDLIST")
}

/// Seconds of media the playlist lists. A torn last line adds nothing.
fn playlist_coverage(text: &str) -> f64 {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("#EXTINF:"))
        .filter_map(|rest| rest.split(',').next()?.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .sum()
}

/// A growing stream cannot seek past what is transcoded, so restart the
/// transcode at the time (as the browser's time bar does) and cast that.
/// With `restart` off the stream already holds the time, and only the file
/// on the TV is too short: a longer one is cast at that time.
/// `at` is a place in the title.
async fn seek_ahead(
    state: &super::AppState,
    id: &str,
    job_id: &str,
    view: &DeviceView,
    at: f64,
    restart: bool,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let id = clean_id(id).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    // The seek replaces the stream the TV is playing. Retire its watch first,
    // so the TV losing the old stream is not reported as a failed cast.
    let mut held = view.clone();
    held.state = Some(DeviceState::Buffering);
    held.position = Some(at);
    held.note = None;
    held.updated_ms = unix_ms();
    let _ = lock_cast(state).begin_device(held);
    let recast = recast_at(state, &id, job_id, view, at, restart).await;
    if recast.is_err() {
        lock_cast(state).end_device(&id);
    }
    recast
}

async fn recast_at(
    state: &super::AppState,
    id: &str,
    job_id: &str,
    view: &DeviceView,
    at: f64,
    restart: bool,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let job = if restart {
        state
            .0
            .player
            .seek(job_id, at)
            .await
            .map_err(super::from_engine)?
    } else {
        state.0.player.job(job_id).ok_or_else(|| {
            super::fail(StatusCode::NOT_FOUND, "That stream is not ready to cast.")
        })?
    };
    // The new stream starts here. A seek right after this one is placed in
    // it, before the TV has said so itself.
    let origin = job
        .position
        .filter(|start| start.is_finite() && *start >= 0.0)
        .unwrap_or(at);
    lock_cast(state).set_device_origin(id, origin);
    let id = id.to_string();
    let endpoint = lock_cast(state).device_endpoint(&id);
    let name = view
        .device_name
        .clone()
        .unwrap_or_else(|| "Cast device".into());
    let device = DeviceRef { id, name, endpoint };
    let title = view.title.clone().unwrap_or_default();
    let art = view.item.as_ref().and_then(|item| item.art.clone());
    let backend = AppBackend {
        state: state.clone(),
    };
    // A restarted stream begins at the time asked for.
    let start = if restart {
        0.0
    } else {
        (at - origin).max(0.0)
    };
    let url = backend
        .cast_from(&device, &job, &title, art.as_deref(), start)
        .await
        .map_err(|text| super::fail(StatusCode::BAD_GATEWAY, &text))?;
    // A whole-title stream is entered at the place in the title.
    let start = if is_cast_playlist(&url) { at } else { start };
    let title = view.title.clone();
    let device_id = device.id.clone();
    // The fresh watch starts without a subtitle; the cast just made carries one over.
    let carried = lock_cast(state).device_subtitle_state();
    watch_device(state, device, url, &job.id, title, view.item.clone(), start);
    lock_cast(state).restore_device_subtitle(&device_id, carried, &job.id);
    Ok(Json(portal_api::OkBody { ok: true }))
}

async fn device_control(
    state: &super::AppState,
    id: &str,
    command: &str,
    seconds: Option<f64>,
) -> Result<Json<portal_api::OkBody>, super::ApiError> {
    let id = clean_id(id).map_err(|msg| super::fail(StatusCode::BAD_REQUEST, msg))?;
    let endpoint = lock_cast(state).device_endpoint(&id);
    let args = control_args(command, &id, endpoint.as_ref(), seconds);
    run_agent(args, CONTROL_LIMIT).await.map_err(agent_error)?;
    Ok(Json(portal_api::OkBody { ok: true }))
}

/// A file that does not parse keeps Portal on this PC: the viewer may have
/// turned the network off, and a damaged file must not turn it back on.
fn settings_from_text(text: Option<&str>) -> CastSettings {
    match text {
        Some(raw) => serde_json::from_str(raw).unwrap_or_else(|err| {
            tracing::warn!("cast settings could not be read, so Portal stays on this PC: {err}");
            CastSettings { lan: false }
        }),
        None => CastSettings::default(),
    }
}

fn save_cast_settings(path: &Path, settings: &CastSettings) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut body =
        serde_json::to_vec_pretty(settings).unwrap_or_else(|_| br#"{"lan":true}"#.to_vec());
    if !body.ends_with(b"\n") {
        body.push(b'\n');
    }
    // Written beside it and swapped in, so a cut-off write never resets the setting.
    let part = path.with_extension("json.part");
    std::fs::write(&part, body)?;
    std::fs::rename(&part, path)
}

fn listen_host(lan: bool) -> &'static str {
    if lan {
        "0.0.0.0"
    } else {
        "127.0.0.1"
    }
}

fn listen_addr(lan: bool) -> String {
    format!("{}:{}", listen_host(lan), listen_port())
}

/// A Portal media address rebased onto this PC's address on the route to the
/// TV. The dashboard builds it from the general address, which can be a VPN's.
fn toward_device(
    state: &super::AppState,
    endpoint: Option<&DeviceEndpoint>,
    url: String,
) -> String {
    let hub = lock_cast(state);
    let (Some(general), Some(device)) = (hub.suggested_url(), hub.url_for_device(endpoint)) else {
        return url;
    };
    match url.strip_prefix(&general) {
        Some(rest) if general != device && rest.starts_with('/') => format!("{device}{rest}"),
        _ => url,
    }
}

fn restart_needed(listening_lan: bool, requested_lan: bool) -> bool {
    listen_host(listening_lan) != listen_host(requested_lan)
}

fn cast_url(lan: bool, ip: Option<&str>) -> Option<String> {
    if !lan {
        return None;
    }
    let ip = ip?;
    if ip.is_empty() {
        return None;
    }
    Some(format!("http://{ip}:{}", listen_port()))
}

/// Best-effort LAN address: UDP connect selects the route's local IPv4 and sends nothing.
fn detect_lan_ipv4() -> Option<String> {
    lan_ipv4_toward(None)
}

/// This PC's address on the route to `target` (a TV), so a VPN, WSL or
/// Hyper-V adapter holding the internet route doesn't hand the TV an address
/// it can't reach. The internet route is the fallback, and works offline too
/// as long as the LAN has a default route.
fn lan_ipv4_toward(target: Option<&str>) -> Option<String> {
    let local = |peer: &str| -> Option<Ipv4Addr> {
        let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
        socket.connect(peer).ok()?;
        match socket.local_addr().ok()?.ip() {
            IpAddr::V4(ip) if usable_lan(ip) => Some(ip),
            _ => None,
        }
    };
    let toward = target
        .and_then(|host| host.parse::<Ipv4Addr>().ok())
        .and_then(|host| local(&format!("{host}:8009")));
    toward
        .or_else(|| local("8.8.8.8:80"))
        .map(|ip| ip.to_string())
}

fn usable_lan(ip: Ipv4Addr) -> bool {
    !ip.is_unspecified() && !ip.is_loopback() && !ip.is_broadcast() && !ip.is_multicast()
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

fn expire_receivers(receivers: &mut Vec<ReceiverStamp>, now: Instant, ttl: Duration) {
    receivers.retain(|row| now.saturating_duration_since(row.seen) < ttl);
}

fn touch_receiver(
    receivers: &mut Vec<ReceiverStamp>,
    id: String,
    name: String,
    now: Instant,
    seen_ms: u64,
    ttl: Duration,
) {
    if let Some(row) = receivers.iter_mut().find(|row| row.id == id) {
        row.name = name;
        row.seen = now;
        row.seen_ms = seen_ms;
    } else {
        receivers.push(ReceiverStamp {
            id,
            name,
            seen: now,
            seen_ms,
        });
    }
    expire_receivers(receivers, now, ttl);
    if receivers.len() > RECEIVER_LIMIT {
        receivers.sort_by_key(|row| std::cmp::Reverse(row.seen));
        receivers.truncate(RECEIVER_LIMIT);
    }
}

fn assign_session(
    receivers: &[ReceiverStamp],
    receiver_id: &str,
    job_id: &str,
    title: Option<String>,
    replay: Option<CastReplay>,
) -> Result<CastSession, SessionSetError> {
    let receiver_id = receiver_id.trim();
    let job_id = job_id.trim();
    if receiver_id.is_empty()
        || job_id.is_empty()
        || receiver_id.len() > NAME_LIMIT
        || job_id.len() > NAME_LIMIT
    {
        return Err(SessionSetError::MissingField);
    }
    if !receivers.iter().any(|row| row.id == receiver_id) {
        return Err(SessionSetError::UnknownReceiver);
    }
    let title = title.and_then(|value| {
        let trimmed = clip(value.trim());
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    });
    Ok(CastSession {
        receiver_id: receiver_id.to_string(),
        job_id: job_id.to_string(),
        title,
        replay,
        place: None,
        command: None,
        next_seq: 0,
    })
}

fn clear_session(slot: &mut Option<CastSession>) {
    *slot = None;
}

fn clip(text: &str) -> &str {
    if text.len() <= NAME_LIMIT {
        return text;
    }
    let mut end = NAME_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn clean_label(text: &str) -> String {
    let mut out = String::new();
    for ch in text.trim().chars() {
        if ch.is_control() {
            continue;
        }
        out.push(ch);
        if out.len() >= NAME_LIMIT {
            break;
        }
    }
    while !out.is_char_boundary(out.len()) {
        out.pop();
    }
    let end = clip(&out).len();
    out.truncate(end);
    out
}

fn clean_id(raw: &str) -> Result<String, &'static str> {
    let id = raw.trim();
    if id.is_empty() || id.len() > 80 {
        return Err("A Cast device is required.");
    }
    if id.contains("..")
        || !id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | ':' | '-'))
    {
        return Err("That Cast device is not valid.");
    }
    Ok(id.to_string())
}

fn clean_host(raw: &str) -> Option<String> {
    let host = raw.trim();
    if host.is_empty() || host.len() > 253 || host.starts_with('-') || host.contains("..") {
        return None;
    }
    if host
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | ':' | '-' | '%' | '[' | ']'))
    {
        Some(host.to_string())
    } else {
        None
    }
}

/// A Portal HLS (or segment) URL the TV can fetch. Loopback is rejected.
fn cast_media_url(raw: &str) -> Result<String, &'static str> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2000 {
        return Err("That video address is not valid.");
    }
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))
        .ok_or("The video address must be an http link the TV can reach.")?;
    if rest.contains('@') {
        return Err("The video address cannot include a login.");
    }
    let slash = rest
        .find('/')
        .ok_or("The video address must be a Portal media link.")?;
    let hostport = &rest[..slash];
    let path = &rest[slash..];
    if hostport.is_empty() || path.contains('\\') || path.contains("..") {
        return Err("The video address must be a Portal media link.");
    }
    let host = host_of(hostport);
    if !tv_can_reach(host) {
        return Err("The TV cannot open a loopback address. Use the network address.");
    }
    if !path.starts_with("/media/") || !own_host(host) {
        return Err("Only a Portal media link can be cast.");
    }
    Ok(raw.to_string())
}

/// This PC, by one of its own addresses or by its computer name, so a cast
/// can only put Portal's own stream on the TV.
fn own_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = bare.parse::<std::net::IpAddr>() {
        return crate::from_this_pc(std::net::SocketAddr::new(ip, 0));
    }
    let name = std::env::var("COMPUTERNAME").unwrap_or_default();
    let lower = host.trim_end_matches('.').to_ascii_lowercase();
    !name.is_empty()
        && lower
            .split('.')
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(&name))
}

struct MediaUrlParts {
    session: String,
}

const PREPARE_NOTE: &str = "Could not prepare the video for the TV.";
/// A cast file smaller than this is a failed remux, not a video.
const MIN_CAST_BYTES: u64 = 1000;
/// New media this short is the same file the TV already has.
const SPAN_SLACK: f64 = 2.0;
/// A still-encoding title is continued once at least this much is new.
const EXTEND_TAIL: f64 = 30.0;
/// An ended playlist this close to what already played is the real ending.
const EXTEND_NEAR: f64 = 15.0;
/// After waiting, continue for a smaller tail so the picture does not stop.
const EXTEND_FLOOR: f64 = 8.0;
const EXTEND_WAIT: Duration = Duration::from_secs(90);
/// The TV is looked at every few seconds, so a file that ended was last
/// seen this close to its end.
const FINISH_GAP: f64 = 10.0;
/// The first file of a stream that is still encoding should hold this much.
const FIRST_SPAN: f64 = 20.0;
const FIRST_WAIT: Duration = Duration::from_secs(15);
const STALLED_NOTE: &str = "Portal stopped preparing the video before its end.";
const COPY_LIMIT: Duration = Duration::from_secs(10 * 60);
const ENCODE_LIMIT: Duration = Duration::from_secs(20 * 60);

enum PrepareFail {
    NotReady,
    Failed,
}

/// What to do when a cast file reaches its end before the title does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtendChoice {
    /// Build a longer file and keep playing.
    Extend,
    /// The title itself ended.
    Finish,
    /// The encode is still working and the new tail is still short.
    Wait,
}

fn extend_choice(coverage: f64, played: f64, ended: bool) -> ExtendChoice {
    if !coverage.is_finite() || !played.is_finite() || played < 1.0 || coverage <= 0.0 {
        return ExtendChoice::Finish;
    }
    let tail = coverage - played;
    if ended && tail <= EXTEND_NEAR {
        return ExtendChoice::Finish;
    }
    if tail >= EXTEND_TAIL {
        return ExtendChoice::Extend;
    }
    if ended {
        return ExtendChoice::Finish;
    }
    ExtendChoice::Wait
}

/// Whether the address is a job's whole-title playlist.
fn is_cast_playlist(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.ends_with(&format!("/{}", portal_engine::CAST_PLAYLIST)))
}

/// What the TV is given for a job's stream, and where in it the TV starts.
/// `start` is a place in the job's own stream.
///
/// A title with a picture and a known length gets its whole-title playlist,
/// entered at the place in the title. Anything else gets a file of what is
/// ready, as before.
async fn cast_target(
    player: &portal_engine::Player,
    url: &str,
    job: Option<&portal_api::PlayJob>,
    start: f64,
) -> Result<(String, f64), super::ApiError> {
    let whole = job
        .filter(|_| url_targets_m3u8(url))
        .and_then(|job| Some((job, player.cast_stream(&job.id)?)));
    let Some((job, session)) = whole else {
        return Ok((prepare_cast_media(player, url).await?, start));
    };
    let origin = job
        .position
        .filter(|at| at.is_finite() && *at >= 0.0)
        .unwrap_or(0.0);
    let at = origin + start.max(0.0);
    // The encoder starts now, so the first part is there when the TV asks.
    player.cast_warm(&session, at);
    let base = url.find("/media/").map_or("", |end| &url[..end]);
    Ok((
        format!("{base}/media/{session}/{}", portal_engine::CAST_PLAYLIST),
        at,
    ))
}

/// The TV plays a progressive MP4. Some receivers reject a growing HLS
/// playlist, so a Portal `.m3u8` is remuxed to `/media/{session}/cast.mp4`.
async fn prepare_cast_media(
    player: &portal_engine::Player,
    url: &str,
) -> Result<String, super::ApiError> {
    if !url_targets_m3u8(url) {
        return Ok(url.to_string());
    }
    let parts = media_url_parts(url).ok_or_else(|| {
        super::fail(
            StatusCode::BAD_REQUEST,
            "The video address must be a Portal media link.",
        )
    })?;
    let playlist = player
        .session_file(&parts.session, "index.m3u8")
        .map_err(|_| super::fail(StatusCode::NOT_FOUND, "That stream is not ready to cast."))?;
    if !playlist.is_file() {
        return Err(super::fail(
            StatusCode::NOT_FOUND,
            "That stream is not ready to cast.",
        ));
    }
    wait_for_first_span(player, &parts.session, &playlist).await;
    match ensure_cast_mp4(&playlist).await {
        Ok(version) => Ok(mp4_url(url, version)),
        Err(PrepareFail::NotReady) => Err(super::fail(
            StatusCode::NOT_FOUND,
            "That stream is not ready to cast.",
        )),
        Err(PrepareFail::Failed) => {
            tracing::warn!("{PREPARE_NOTE}");
            Err(super::fail(StatusCode::BAD_GATEWAY, PREPARE_NOTE))
        }
    }
}

/// A stream cast the moment it starts would give the TV a few seconds and
/// then stop to load more, so a working encode gets a moment to get ahead.
async fn wait_for_first_span(player: &portal_engine::Player, session: &str, playlist: &Path) {
    let deadline = Instant::now() + FIRST_WAIT;
    while Instant::now() < deadline && player.session_producing(session) {
        let text = std::fs::read_to_string(playlist).unwrap_or_default();
        if playlist_ended(&text) || playlist_coverage(&text) >= FIRST_SPAN {
            return;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

/// Covered seconds of the file the TV should load. Reuses one that already
/// matches this playlist.
async fn ensure_cast_mp4(playlist: &Path) -> Result<f64, PrepareFail> {
    let _remux = REMUX.lock().await;
    let text = std::fs::read_to_string(playlist).map_err(|_| PrepareFail::NotReady)?;
    let coverage = playlist_coverage(&text);
    if coverage <= 0.0 {
        return Err(PrepareFail::NotReady);
    }
    let ended = playlist_ended(&text);
    let dir = playlist.parent().ok_or(PrepareFail::Failed)?;
    let mp4 = dir.join("cast.mp4");
    let span_path = dir.join("cast.span");
    if cast_mp4_fresh(&mp4, &span_path, playlist, coverage, ended) {
        return Ok(read_span(&span_path).unwrap_or(coverage));
    }
    // A file made ahead was for the one being replaced here.
    drop_staged(dir);
    let part = dir.join("cast.mp4.part");
    let snap_coverage = remux_snapshot(dir, &text, &part).await?;
    if !replace_file(&part, &mp4).await {
        let _ = std::fs::remove_file(&part);
        // Whatever is there now is not known to match the saved length.
        let _ = std::fs::remove_file(&span_path);
        return Err(PrepareFail::Failed);
    }
    let _ = std::fs::write(&span_path, format!("{snap_coverage:.3}\n"));
    Ok(snap_coverage)
}

/// One remux at a time: two would write the same files.
static REMUX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Writes the playlist's segments so far to `part` as one MP4 and answers
/// the seconds it holds. The caller holds `REMUX`.
async fn remux_snapshot(dir: &Path, text: &str, part: &Path) -> Result<f64, PrepareFail> {
    // ffmpeg follows an EVENT playlist until it ends. A sibling VOD copy
    // closes the file on the segments that exist now.
    let snapshot = vod_snapshot(text);
    let snap_coverage = playlist_coverage(&snapshot);
    if snap_coverage <= 0.0 {
        return Err(PrepareFail::NotReady);
    }
    let vod = dir.join("cast.vod.m3u8");
    std::fs::write(&vod, &snapshot).map_err(|_| PrepareFail::Failed)?;
    let _ = std::fs::remove_file(part);
    let copied = remux_file(&remux_copy_args(&vod, part), COPY_LIMIT).await;
    let ready = copied == Remux::Done && file_len(part) > MIN_CAST_BYTES;
    if !ready {
        let _ = std::fs::remove_file(part);
        // A copy that ran out of time would only take longer encoded.
        if copied == Remux::TimedOut {
            return Err(PrepareFail::Failed);
        }
        let encoded = remux_file(&remux_encode_args(&vod, part), ENCODE_LIMIT).await;
        if encoded != Remux::Done || file_len(part) <= MIN_CAST_BYTES {
            let _ = std::fs::remove_file(part);
            return Err(PrepareFail::Failed);
        }
    }
    Ok(snap_coverage)
}

/// The next cast file, made while the TV still plays the one before it.
const STAGED_FILE: &str = "cast.next.mp4";
const STAGED_SPAN: &str = "cast.next.span";
/// The next file is started this long before the one playing ends, and
/// earlier for a long file, which takes longer to write.
const STAGE_LEAD_MIN: f64 = 30.0;
const STAGE_LEAD_MAX: f64 = 240.0;
const STAGE_LEAD_SHARE: f64 = 0.15;

fn stage_lead(span: f64) -> f64 {
    (span * STAGE_LEAD_SHARE).clamp(STAGE_LEAD_MIN, STAGE_LEAD_MAX)
}

fn drop_staged(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(STAGED_SPAN));
    let _ = std::fs::remove_file(dir.join(STAGED_FILE));
}

/// Whether the next file should be made now: the TV is near the end of a
/// cast file `span` seconds long, and the stream holds a real tail past it.
fn stage_due(span: f64, at: f64, coverage: f64, ended: bool, staged: Option<f64>) -> bool {
    if !at.is_finite() || span - at > stage_lead(span) {
        return false;
    }
    if staged.is_some_and(|staged| staged >= span + EXTEND_FLOOR) {
        return false;
    }
    extend_choice(coverage, span, ended) == ExtendChoice::Extend
}

/// Makes the file that follows the one on the TV shortly before that one
/// ends, so the TV waits only for the new file to load and not for it to be
/// written. Called with every look at the TV; starts one remux at most.
fn stage_next_file(player: &portal_engine::Player, watched: &Watched, at: f64) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STAGING: AtomicBool = AtomicBool::new(false);
    struct Release;
    impl Drop for Release {
        fn drop(&mut self) {
            STAGING.store(false, Ordering::SeqCst);
        }
    }
    if !is_cast_file(&watched.media_url) {
        return;
    }
    let Some(span) = url_span(&watched.media_url) else {
        return;
    };
    if span - at > stage_lead(span) {
        return;
    }
    let Some(parts) = media_url_parts(&watched.media_url) else {
        return;
    };
    let Ok(playlist) = player.session_file(&parts.session, "index.m3u8") else {
        return;
    };
    if STAGING.swap(true, Ordering::SeqCst) {
        return;
    }
    let release = Release;
    tokio::spawn(async move {
        let _release = release;
        let Some(dir) = playlist.parent() else {
            return;
        };
        let _remux = REMUX.lock().await;
        let Ok(text) = std::fs::read_to_string(&playlist) else {
            return;
        };
        let staged = read_span(&dir.join(STAGED_SPAN));
        let coverage = playlist_coverage(&text);
        if !stage_due(span, at, coverage, playlist_ended(&text), staged) {
            return;
        }
        drop_staged(dir);
        let part = dir.join("cast.next.part");
        let Ok(made) = remux_snapshot(dir, &text, &part).await else {
            return;
        };
        if replace_file(&part, &dir.join(STAGED_FILE)).await {
            let _ = std::fs::write(dir.join(STAGED_SPAN), format!("{made:.3}\n"));
        } else {
            let _ = std::fs::remove_file(&part);
        }
    });
}

/// Puts a file made ahead in place as the cast file, when it holds a real
/// tail past `played`. Answers the seconds it holds. Waits for one that is
/// still being written.
async fn take_staged(playlist: &Path, played: f64) -> Option<f64> {
    let _remux = REMUX.lock().await;
    let dir = playlist.parent()?;
    let staged = read_span(&dir.join(STAGED_SPAN))?;
    let text = std::fs::read_to_string(playlist).ok()?;
    // A file longer than the stream is left from a stream since replaced.
    let ours = staged <= playlist_coverage(&text) + SPAN_SLACK;
    let _ = std::fs::remove_file(dir.join(STAGED_SPAN));
    if !ours || staged < played + EXTEND_FLOOR {
        drop_staged(dir);
        return None;
    }
    let span_path = dir.join("cast.span");
    if !replace_file(&dir.join(STAGED_FILE), &dir.join("cast.mp4")).await {
        drop_staged(dir);
        let _ = std::fs::remove_file(&span_path);
        return None;
    }
    let _ = std::fs::write(&span_path, format!("{staged:.3}\n"));
    Some(staged)
}

fn cast_mp4_fresh(mp4: &Path, span: &Path, playlist: &Path, coverage: f64, ended: bool) -> bool {
    if file_len(mp4) <= MIN_CAST_BYTES {
        return false;
    }
    if let Some(saved) = read_span(span) {
        return coverage <= saved + SPAN_SLACK;
    }
    ended && newer_than(mp4, playlist)
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0)
}

fn read_span(path: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value = text.trim().parse::<f64>().ok()?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn newer_than(file: &Path, other: &Path) -> bool {
    let Ok(file) = std::fs::metadata(file).and_then(|meta| meta.modified()) else {
        return false;
    };
    let Ok(other) = std::fs::metadata(other).and_then(|meta| meta.modified()) else {
        return false;
    };
    file > other
}

/// A closed playlist of the segments already written, beside those segments.
fn vod_snapshot(text: &str) -> String {
    let mut lines = Vec::new();
    let mut saw_type = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed == "#EXT-X-ENDLIST" {
            continue;
        }
        if trimmed.starts_with("#EXT-X-PLAYLIST-TYPE:") {
            if !saw_type {
                lines.push("#EXT-X-PLAYLIST-TYPE:VOD");
                saw_type = true;
            }
            continue;
        }
        lines.push(trimmed);
    }
    if !saw_type {
        let at = lines
            .iter()
            .position(|line| *line == "#EXTM3U")
            .map(|index| index + 1)
            .unwrap_or(0);
        lines.insert(at, "#EXT-X-PLAYLIST-TYPE:VOD");
    }
    lines.push("#EXT-X-ENDLIST");
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn mp4_url(url: &str, version: f64) -> String {
    let path = url_path_only(url);
    let Some((dir, _)) = path.rsplit_once('/') else {
        return url.to_string();
    };
    let Some(at) = url.find(path) else {
        return url.to_string();
    };
    format!("{}{dir}/cast.mp4?v={version:.3}", &url[..at])
}

/// The seconds a cast file's address says it holds.
fn url_span(url: &str) -> Option<f64> {
    let query = url.split('#').next()?.split_once('?')?.1;
    let value = query.split('&').find_map(|pair| pair.strip_prefix("v="))?;
    value
        .parse::<f64>()
        .ok()
        .filter(|span| span.is_finite() && *span > 0.0)
}

/// A start inside the file, so the TV is not sent past its end.
fn start_within(url: &str, start: f64) -> f64 {
    let start = if start.is_finite() {
        start.max(0.0)
    } else {
        0.0
    };
    match url_span(url) {
        Some(span) => start.min((span - 0.5).max(0.0)),
        None => start,
    }
}

fn versioned_media(url: &str, version: f64) -> String {
    let base = url.split(['?', '#']).next().unwrap_or(url);
    format!("{base}?v={version:.3}")
}

/// Copy the picture and re-encode the audio. Portal HLS often carries AAC
/// packets some receivers play as silence. `-f mp4` is required because
/// the temporary name does not end in `.mp4`, so ffmpeg will not guess the muxer.
fn remux_copy_args(playlist: &Path, dest: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-analyzeduration".into(),
        "100M".into(),
        "-probesize".into(),
        "50M".into(),
        "-i".into(),
        path_arg(playlist),
        "-map".into(),
        "0:v:0".into(),
        "-c:v".into(),
        "copy".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "192k".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-f".into(),
        "mp4".into(),
        path_arg(dest),
    ]
}

fn remux_encode_args(playlist: &Path, dest: &Path) -> Vec<String> {
    vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
        "-y".into(),
        "-analyzeduration".into(),
        "100M".into(),
        "-probesize".into(),
        "50M".into(),
        "-i".into(),
        path_arg(playlist),
        "-map".into(),
        "0:v:0".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "21".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-c:a".into(),
        "aac".into(),
        "-ac".into(),
        "2".into(),
        "-ar".into(),
        "48000".into(),
        "-b:a".into(),
        "192k".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-f".into(),
        "mp4".into(),
        path_arg(dest),
    ]
}

fn path_arg(path: &Path) -> String {
    path.display().to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Remux {
    Done,
    Failed,
    TimedOut,
}

async fn remux_file(args: &[String], limit: Duration) -> Remux {
    let program = crate::paths::ffmpeg();
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::null());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    hide_remux_window(&mut command);
    let child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            tracing::warn!("ffmpeg did not start for the cast file: {err}");
            return Remux::Failed;
        }
    };
    match tokio::time::timeout(limit, child.wait_with_output()).await {
        Ok(Ok(output)) if output.status.success() => Remux::Done,
        Ok(Ok(output)) => {
            let said = String::from_utf8_lossy(&output.stderr);
            let last = said.lines().rev().find(|line| !line.trim().is_empty());
            tracing::warn!(
                "ffmpeg could not make the cast file: {}",
                last.unwrap_or("no reason given").trim()
            );
            Remux::Failed
        }
        Ok(Err(err)) => {
            tracing::warn!("ffmpeg could not make the cast file: {err}");
            Remux::Failed
        }
        Err(_) => {
            tracing::warn!("ffmpeg took too long to make the cast file");
            Remux::TimedOut
        }
    }
}

fn hide_remux_window(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x00004000;
        command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// The TV may still have the previous file open. Try until it lets go.
/// The old file is only removed when it cannot be renamed over, so the
/// address keeps answering in between.
async fn replace_file(part: &Path, dest: &Path) -> bool {
    for _ in 0..40 {
        if std::fs::rename(part, dest).is_ok() {
            return file_len(dest) > MIN_CAST_BYTES;
        }
        let _ = std::fs::remove_file(dest);
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Whether the address is a cast file made from a Portal stream.
fn is_cast_file(url: &str) -> bool {
    url.split(['?', '#'])
        .next()
        .is_some_and(|path| path.ends_with("/cast.mp4"))
}

/// The same media file, whatever query or fragment either address carries.
fn same_media_path(a: &str, b: &str) -> bool {
    let strip = |url: &str| {
        url.trim()
            .split(['?', '#'])
            .next()
            .unwrap_or("")
            .to_string()
    };
    let left = strip(a);
    !left.is_empty() && left == strip(b)
}

fn url_targets_m3u8(url: &str) -> bool {
    let path = url_path_only(url);
    path.len() >= 5 && path[path.len() - 5..].eq_ignore_ascii_case(".m3u8")
}

fn url_path_only(url: &str) -> &str {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    let path = rest.find('/').map(|index| &rest[index..]).unwrap_or("");
    path.split(['?', '#']).next().unwrap_or(path)
}

fn media_url_parts(url: &str) -> Option<MediaUrlParts> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))?;
    let slash = rest.find('/')?;
    let hostport = &rest[..slash];
    if hostport.is_empty() {
        return None;
    }
    let path = url_path_only(url);
    let mut pieces = path.split('/').filter(|part| !part.is_empty());
    if pieces.next()? != "media" {
        return None;
    }
    let session = pieces.next()?;
    let name = pieces.next()?;
    if pieces.next().is_some() || session.is_empty() || name.is_empty() {
        return None;
    }
    Some(MediaUrlParts {
        session: session.to_string(),
    })
}

fn cast_thumb_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 2000 || raw.contains('@') || raw.contains('\\') {
        return None;
    }
    let rest = raw
        .strip_prefix("http://")
        .or_else(|| raw.strip_prefix("https://"))?;
    let hostport = rest.split('/').next().unwrap_or("");
    if hostport.is_empty() || !tv_can_reach(host_of(hostport)) {
        return None;
    }
    Some(raw.to_string())
}

fn host_of(hostport: &str) -> &str {
    if let Some(inside) = hostport.strip_prefix('[') {
        return inside.split(']').next().unwrap_or("");
    }
    hostport.split(':').next().unwrap_or("")
}

fn tv_can_reach(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']);
    if host.is_empty() {
        return false;
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower == "127.0.0.1" || lower == "::1" || lower == "0.0.0.0" {
        return false;
    }
    if let Ok(ip) = lower.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(ip) => usable_lan(ip),
            IpAddr::V6(ip) => !ip.is_loopback() && !ip.is_unspecified(),
        };
    }
    true
}

fn discover_args() -> Vec<String> {
    vec!["discover".into(), "--timeout".into(), "12".into()]
}

fn play_args(
    id: &str,
    endpoint: Option<&DeviceEndpoint>,
    url: &str,
    title: Option<&str>,
    thumb: Option<&str>,
    start: f64,
    subtitle: Option<(&str, &str)>,
) -> Vec<String> {
    let mut args = vec!["play".into(), "--id".into(), id.to_string()];
    if let Some((address, lang)) = subtitle {
        args.extend([
            "--subtitles".into(),
            address.to_string(),
            format!("--subtitles-lang={lang}"),
        ]);
    }
    push_endpoint(&mut args, endpoint);
    args.push("--url".into());
    args.push(url.to_string());
    // Always sent: a growing playlist without a start time begins at its newest segment.
    args.push("--seconds".into());
    args.push(format!("{start:.3}"));
    if let Some(title) = title.map(str::trim).filter(|value| !value.is_empty()) {
        // Joined with `=`: the helper's parser takes a separate value that
        // starts with a hyphen ("-Pilot", "--") for an option and refuses it.
        args.push(format!("--title={title}"));
    }
    if let Some(thumb) = thumb.map(str::trim).filter(|value| !value.is_empty()) {
        args.push("--thumb".into());
        args.push(thumb.to_string());
    }
    args.push("--timeout".into());
    args.push("20".into());
    args
}

fn control_args(
    command: &str,
    id: &str,
    endpoint: Option<&DeviceEndpoint>,
    seconds: Option<f64>,
) -> Vec<String> {
    let mut args = vec![command.to_string(), "--id".into(), id.to_string()];
    push_endpoint(&mut args, endpoint);
    if let Some(seconds) = seconds {
        args.push("--seconds".into());
        args.push(format!("{seconds:.3}"));
    }
    args.push("--timeout".into());
    args.push("20".into());
    args
}

fn volume_args(
    id: &str,
    endpoint: Option<&DeviceEndpoint>,
    level: Option<f64>,
    muted: Option<bool>,
) -> Vec<String> {
    let mut args = vec!["volume".into(), "--id".into(), id.to_string()];
    push_endpoint(&mut args, endpoint);
    if let Some(level) = level {
        args.push(format!("--level={level:.3}"));
    }
    if let Some(muted) = muted {
        args.push(format!("--muted={muted}"));
    }
    args.push("--timeout".into());
    args.push("20".into());
    args
}

fn push_endpoint(args: &mut Vec<String>, endpoint: Option<&DeviceEndpoint>) {
    let Some(endpoint) = endpoint else {
        return;
    };
    if endpoint.host.is_empty() {
        return;
    }
    args.push("--host".into());
    args.push(endpoint.host.clone());
    if endpoint.port > 0 {
        args.push("--port".into());
        args.push(endpoint.port.to_string());
    }
    // Free text goes joined with `=`, like a title.
    if !endpoint.model.is_empty() {
        args.push(format!("--model={}", endpoint.model));
    }
    if !endpoint.name.is_empty() {
        args.push(format!("--name={}", endpoint.name));
    }
}

fn devices_from_value(value: &serde_json::Value) -> Vec<DeviceRecord> {
    let Some(items) = value.get("items").and_then(|item| item.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|value| value.as_str()) else {
            continue;
        };
        let Ok(id) = clean_id(id) else {
            continue;
        };
        if out.iter().any(|row: &DeviceRecord| row.id == id) {
            continue;
        }
        let name = item
            .get("name")
            .and_then(|value| value.as_str())
            .map(clean_label)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Cast device".into());
        let host = item
            .get("host")
            .and_then(|value| value.as_str())
            .and_then(clean_host)
            .unwrap_or_default();
        let model = item
            .get("model")
            .and_then(|value| value.as_str())
            .map(clean_label)
            .unwrap_or_default();
        let port = item
            .get("port")
            .and_then(|value| value.as_u64())
            .and_then(|value| u16::try_from(value).ok())
            .filter(|value| *value > 0)
            .unwrap_or(8009);
        out.push(DeviceRecord {
            id,
            name,
            host,
            model,
            port,
        });
    }
    out
}

#[derive(Debug, Clone)]
enum AgentFail {
    Missing,
    Timeout,
    NotFound(String),
    BadRequest(String),
    Unavailable(String),
}

fn agent_error(err: AgentFail) -> super::ApiError {
    match err {
        AgentFail::Missing => super::fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Cast support is not installed.",
        ),
        AgentFail::Timeout => super::fail(
            StatusCode::GATEWAY_TIMEOUT,
            "The Cast device did not answer in time.",
        ),
        AgentFail::NotFound(message) => super::fail(StatusCode::NOT_FOUND, message),
        AgentFail::BadRequest(message) => super::fail(StatusCode::BAD_REQUEST, message),
        AgentFail::Unavailable(message) => super::fail(StatusCode::BAD_GATEWAY, message),
    }
}

fn agent_fail_value(value: &serde_json::Value) -> AgentFail {
    let message = value
        .get("error")
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or("Cast failed.");
    let message = clean_label(message);
    // The helper passes some Python exception text through; paths and
    // addresses in it are not for the page.
    let raw = ["Traceback", "\\", "://", ".py", "Errno"]
        .iter()
        .any(|mark| message.contains(mark));
    let message = if message.is_empty() {
        "Cast failed.".to_string()
    } else if raw {
        "The Cast device did not answer.".to_string()
    } else {
        message
    };
    match value.get("code").and_then(|item| item.as_str()) {
        Some("not_found") => AgentFail::NotFound(message),
        Some("bad_request") => AgentFail::BadRequest(message),
        _ => AgentFail::Unavailable(message),
    }
}

fn parse_agent_stdout(text: &str) -> Option<serde_json::Value> {
    let mut last = None;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            last = Some(value);
        }
    }
    last
}

fn public_agent_text(text: &str) -> Option<String> {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    if line.contains("Traceback") || line.contains('\\') || line.contains('/') {
        return Some("Cast helper failed.".into());
    }
    let cleaned = clean_label(line);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

fn agent_output(
    status_ok: bool,
    stdout: &str,
    stderr: &str,
) -> Result<serde_json::Value, AgentFail> {
    let parsed = parse_agent_stdout(stdout);
    if status_ok {
        if let Some(value) = parsed {
            if value.get("ok").and_then(|item| item.as_bool()) == Some(false) {
                return Err(agent_fail_value(&value));
            }
            return Ok(value);
        }
        return Err(AgentFail::Unavailable(
            public_agent_text(stderr).unwrap_or_else(|| "Cast helper returned nothing.".into()),
        ));
    }
    if let Some(value) = &parsed {
        return Err(agent_fail_value(value));
    }
    Err(AgentFail::Unavailable(
        public_agent_text(stderr)
            .or_else(|| public_agent_text(stdout))
            .unwrap_or_else(|| "Cast helper failed.".into()),
    ))
}

type HelperLines = tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>;
type Pending = std::sync::Mutex<
    std::collections::HashMap<u64, tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
>;

/// The one cast helper process (`cast_agent.py serve`): started on first use,
/// started again after it dies or wedges, and gone with Portal (its stdin
/// closes, or `stop_helper`). It keeps each TV's connection between commands,
/// so a command no longer pays for starting Python and connecting.
struct Helper {
    send: tokio::sync::mpsc::UnboundedSender<String>,
    pending: std::sync::Arc<Pending>,
    dead: std::sync::Arc<std::sync::atomic::AtomicBool>,
    child: std::sync::Mutex<Option<tokio::process::Child>>,
}

static HELPER: std::sync::Mutex<Option<std::sync::Arc<Helper>>> = std::sync::Mutex::new(None);
static NEXT_RID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// How long a helper that let a command time out has to answer a ping.
const PING_WAIT: Duration = Duration::from_secs(5);

fn helper() -> Result<std::sync::Arc<Helper>, AgentFail> {
    let mut slot = HELPER.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(helper) = slot.as_ref().filter(|helper| !helper.dead.load(std::sync::atomic::Ordering::SeqCst)) {
        return Ok(helper.clone());
    }
    let script = crate::paths::cast_agent_script().ok_or(AgentFail::Missing)?;
    let python = crate::paths::cast_python();
    let mut command = agent_command(&python, &script);
    command.arg("serve");
    command.stdin(std::process::Stdio::piped());
    // Nothing reads stderr for the helper's life; a full pipe would stall it.
    command.stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|_| AgentFail::Missing)?;
    let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        let _ = child.start_kill();
        return Err(AgentFail::Unavailable("Cast helper failed.".into()));
    };
    let (send, mut outbox) = tokio::sync::mpsc::unbounded_channel::<String>();
    let pending: std::sync::Arc<Pending> = Default::default();
    tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        while let Some(text) = outbox.recv().await {
            if stdin.write_all(text.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
                break;
            }
        }
    });
    let routes = pending.clone();
    let dead = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gone = dead.clone();
    tokio::spawn(async move {
        let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stdout));
        while let Ok(Some(text)) = lines.next_line().await {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
                continue;
            };
            let Some(rid) = value
                .get("rid")
                .or_else(|| value.get("pong"))
                .and_then(|rid| rid.as_u64())
            else {
                continue;
            };
            let mut routes = routes.lock().unwrap_or_else(|err| err.into_inner());
            let last = value.get("end").is_some() || value.get("pong").is_some();
            if let Some(route) = routes.get(&rid) {
                let _ = route.send(value);
            }
            if last {
                routes.remove(&rid);
            }
        }
        // Dead: every waiting command and watch hears it, and the next use starts another.
        gone.store(true, std::sync::atomic::Ordering::SeqCst);
        routes.lock().unwrap_or_else(|err| err.into_inner()).clear();
    });
    let helper = std::sync::Arc::new(Helper {
        send,
        pending,
        dead,
        child: std::sync::Mutex::new(Some(child)),
    });
    *slot = Some(helper.clone());
    Ok(helper)
}

impl Helper {
    fn post(&self, value: serde_json::Value) {
        let _ = self.send.send(format!("{value}\n"));
    }

    /// Sends a command; its lines arrive on the receiver, which closes after its end line.
    fn request(&self, args: &[String]) -> (u64, HelperLines) {
        let rid = NEXT_RID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        self.pending
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(rid, tx);
        self.post(serde_json::json!({ "rid": rid, "argv": args }));
        (rid, rx)
    }

    fn cancel(&self, rid: u64) {
        self.pending
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&rid);
        self.post(serde_json::json!({ "cancel": rid }));
    }

    fn kill(&self) {
        if let Some(mut child) = self.child.lock().unwrap_or_else(|err| err.into_inner()).take() {
            let _ = child.start_kill();
        }
        let mut slot = HELPER.lock().unwrap_or_else(|err| err.into_inner());
        if slot.as_ref().is_some_and(|held| std::ptr::eq(&**held, self)) {
            *slot = None;
        }
    }
}

/// After a command ran out of time: drop that TV's kept connection, and kill
/// the helper if it does not answer a ping either.
fn after_timeout(helper: std::sync::Arc<Helper>, rid: u64, args: &[String]) {
    helper.cancel(rid);
    if let Some(id) = args.iter().position(|arg| arg == "--id").and_then(|at| args.get(at + 1)) {
        helper.post(serde_json::json!({ "forget": id }));
    }
    tokio::spawn(async move {
        let (ping, mut pong) = {
            let rid = NEXT_RID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
            helper
                .pending
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .insert(rid, tx);
            (rid, rx)
        };
        helper.post(serde_json::json!({ "ping": ping }));
        if !matches!(tokio::time::timeout(PING_WAIT, pong.recv()).await, Ok(Some(_))) {
            tracing::warn!("Cast helper did not answer; starting another");
            helper.kill();
        }
    });
}

/// Ends the helper with Portal.
pub(crate) fn stop_helper() {
    let held = HELPER.lock().unwrap_or_else(|err| err.into_inner()).take();
    if let Some(helper) = held {
        helper.kill();
    }
}

async fn run_agent(args: Vec<String>, limit: Duration) -> Result<serde_json::Value, AgentFail> {
    let helper = helper()?;
    let (rid, mut lines) = helper.request(&args);
    let mut out = String::new();
    let ended = tokio::time::timeout(limit, async {
        while let Some(mut value) = lines.recv().await {
            if value.get("end").is_some() {
                return Some(value.get("code").and_then(|code| code.as_i64()) == Some(0));
            }
            if let Some(map) = value.as_object_mut() {
                map.remove("rid");
            }
            out.push_str(&value.to_string());
            out.push('\n');
        }
        None
    })
    .await;
    match ended {
        Ok(Some(ok)) => agent_output(ok, &out, ""),
        // The helper died during the command.
        Ok(None) => agent_output(false, &out, ""),
        Err(_) => {
            after_timeout(helper, rid, &args);
            Err(AgentFail::Timeout)
        }
    }
}

fn agent_command(python: &Path, script: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(python);
    command.arg(script);
    command.stdin(std::process::Stdio::null());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    command.kill_on_drop(true);
    command.env("PYTHONIOENCODING", "utf-8");
    command.env("PYTHONUTF8", "1");
    hide_agent_window(&mut command);
    command
}

fn hide_agent_window(command: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

fn watch_args(device: &DeviceRef) -> Vec<String> {
    let mut args = vec!["watch".into(), "--id".into(), device.id.clone()];
    push_endpoint(&mut args, device.endpoint.as_ref());
    args.push("--interval".into());
    args.push(watch::WATCH_INTERVAL.as_secs().to_string());
    args.push("--timeout".into());
    args.push("20".into());
    args
}

/// The longest wait for a status line, including the helper's first connection.
const WATCH_LINE_WAIT: Duration = Duration::from_secs(45);

/// The helper's `watch` command, started again (after a pause) when it stops.
struct AgentWatch {
    args: Vec<String>,
    running: Option<(std::sync::Arc<Helper>, u64)>,
    lines: Option<HelperLines>,
    started: bool,
}

impl AgentWatch {
    fn new(device: &DeviceRef) -> Self {
        Self {
            args: watch_args(device),
            running: None,
            lines: None,
            started: false,
        }
    }

    fn spawn(&mut self) -> Result<(), String> {
        let helper = helper().map_err(|_| "Cast support is not installed.".to_string())?;
        let (rid, lines) = helper.request(&self.args);
        self.lines = Some(lines);
        self.running = Some((helper, rid));
        Ok(())
    }

    fn close(&mut self) {
        self.lines = None;
        if let Some((helper, rid)) = self.running.take() {
            helper.cancel(rid);
        }
    }
}

impl Drop for AgentWatch {
    fn drop(&mut self) {
        self.close();
    }
}

impl watch::StatusSource for AgentWatch {
    async fn next(&mut self) -> Result<watch::DeviceStatus, String> {
        loop {
            if self.lines.is_none() {
                if self.started {
                    tokio::time::sleep(watch::WATCH_INTERVAL).await;
                }
                self.started = true;
                self.spawn()?;
            }
            let Some(lines) = self.lines.as_mut() else {
                continue;
            };
            match tokio::time::timeout(WATCH_LINE_WAIT, lines.recv()).await {
                Ok(Some(value)) if value.get("end").is_none() => {
                    return watch::status_from_value(&value);
                }
                Ok(_) => {
                    self.close();
                    return Err("The cast helper stopped.".into());
                }
                Err(_) => {
                    if let Some((helper, rid)) = self.running.take() {
                        after_timeout(helper, rid, &self.args);
                    }
                    self.close();
                    return Err("The cast helper stopped.".into());
                }
            }
        }
    }
}

/// The watch's view of Portal: play jobs, the library, prefs, and the cast helper.
struct AppBackend {
    state: super::AppState,
}

fn api_detail(err: &super::ApiError) -> String {
    Some(err.1 .0.detail.as_str())
        .map(clean_label)
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| "Playback failed.".into())
}

/// A poster the TV can load itself: the web address a Kodi `image://`
/// wraps, or a plain link. A TV does not pair, so it never gets Portal's
/// image proxy; a picture that is only a local file is left out.
fn next_thumb(_base: &str, art: &str) -> Option<String> {
    let art = art.trim();
    let inner = art
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("image://"))
        .map(|_| decode_component(art[8..].split('@').next().unwrap_or("").trim_end_matches('/')));
    match inner {
        Some(inner) => cast_thumb_url(&inner),
        None => cast_thumb_url(art),
    }
}

/// Percent-decoding, for the address inside an `image://`.
fn decode_component(text: &str) -> String {
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

/// A subtitle that takes longer than this to download follows the cast.
const QUICK_SUBTITLE: Duration = Duration::from_secs(3);
/// How long a subtitle that follows the cast waits for a seek to finish:
/// longer than one helper command (CONTROL_LIMIT).
const LATER_GUARD_WAIT: Duration = Duration::from_millis(500);
const LATER_GUARD_TRIES: u32 = 100;
/// How often, and how long, a subtitle that follows the cast waits for the
/// video's own tracks to be read before it looks online.
const TRACKS_POLL: Duration = Duration::from_secs(3);
const TRACKS_WAIT: Duration = Duration::from_secs(120);
/// How often a cast that is about to start looks for the video's own tracks
/// within QUICK_SUBTITLE.
const QUICK_POLL: Duration = Duration::from_millis(500);

/// What a subtitle sent after the video started should be.
enum Want {
    /// This one, as the dashboard picked it, else one in the wanted language.
    Choice(CastSubtitle),
    /// One in this language for the job (the next episode).
    Language(String),
}

/// Finds and readies a subtitle for `job` after its video started on the TV,
/// then shows it, unless the cast moved on, stopped, or the user picked
/// something else (or turned subtitles off) meanwhile.
fn send_subtitle_later(
    state: super::AppState,
    device: DeviceRef,
    job: portal_api::PlayJob,
    base: String,
    want: Want,
) {
    let (ask, wanted_lang) = {
        let hub = lock_cast(&state);
        (hub.latest_subtitle_ask(), hub.device_view().subtitle_lang)
    };
    tokio::spawn(async move {
        let job_id = job.id.clone();
        let still_wanted = || {
            let hub = lock_cast(&state);
            let view = hub.device_view();
            hub.latest_subtitle_ask() == ask
                && view.active
                && view.device_id.as_deref() == Some(device.id.as_str())
                && view.job_id.as_deref() == Some(job_id.as_str())
                && view.subtitle.is_none()
        };
        let (pick, lang) = match want {
            Want::Choice(choice) => (Some(choice), wanted_lang),
            Want::Language(lang) => (None, Some(lang)),
        };
        let found = later_subtitle(&state, &base, job, pick, lang, &still_wanted).await;
        let shown = match found {
            Some((choice, job)) => {
                show_later(&state, &device, &base, &job, choice, &still_wanted).await
            }
            None => false,
        };
        if !shown {
            lock_cast(&state).settle_device_subtitle(&device.id, &job_id, ask);
        }
    });
}

/// The subtitle to send after the video started, with the job as it is
/// then: the pick if it can be shown, else one in `lang`. None when there
/// is none, or the cast moved on.
async fn later_subtitle(
    state: &super::AppState,
    base: &str,
    job: portal_api::PlayJob,
    pick: Option<CastSubtitle>,
    lang: Option<String>,
    still_wanted: &impl Fn() -> bool,
) -> Option<(CastSubtitle, portal_api::PlayJob)> {
    if let Some(choice @ CastSubtitle::Online { .. }) = &pick {
        if subtitle_ready(state, &job, choice).await {
            return Some((choice.clone(), job));
        }
    }
    // The video's own tracks are read a while after it starts; one of them
    // in the wanted language is better than any online file.
    let job = tracks_read(state, job, still_wanted).await?;
    if let Some(choice @ CastSubtitle::Embedded { .. }) = &pick {
        if tv_subtitle_address(&state.0.player, base, &job, choice).is_some() {
            return Some((choice.clone(), job));
        }
    }
    let choice = subtitle_in_language(state, &job, &lang?).await?;
    // Another online file can be broken too; it is not sent then.
    if pick.as_ref() == Some(&choice) || !subtitle_ready(state, &job, &choice).await {
        return None;
    }
    Some((choice, job))
}

/// The job once the video's own subtitle tracks are read, or as it is after
/// TRACKS_WAIT; None when the cast moved on meanwhile.
async fn tracks_read(
    state: &super::AppState,
    job: portal_api::PlayJob,
    still_wanted: &impl Fn() -> bool,
) -> Option<portal_api::PlayJob> {
    let mut job = state.0.player.job(&job.id).unwrap_or(job);
    let begun = Instant::now();
    while job.subtitles_pending && begun.elapsed() < TRACKS_WAIT {
        tokio::time::sleep(TRACKS_POLL).await;
        if !still_wanted() {
            return None;
        }
        job = state.0.player.job(&job.id)?;
    }
    Some(job)
}

/// The job's own full track in `lang`, looked for until QUICK_SUBTITLE while
/// its tracks are being read, so it goes with the cast rather than following
/// it with a reload. Answers the job as it is then too.
async fn quick_track(
    state: &super::AppState,
    job: &portal_api::PlayJob,
    lang: &str,
) -> Option<(CastSubtitle, portal_api::PlayJob)> {
    let begun = Instant::now();
    let mut now = job.clone();
    loop {
        if let Some(track) = track_in(&now.subtitles, lang) {
            let choice = CastSubtitle::Embedded {
                id: track.id,
                lang: Some(lang.to_string()),
            };
            return Some((choice, now));
        }
        if !now.subtitles_pending || begun.elapsed() >= QUICK_SUBTITLE {
            return None;
        }
        tokio::time::sleep(QUICK_POLL).await;
        now = state.0.player.job(&job.id)?;
    }
}

/// Sends `choice` to the TV once no seek holds it. Answers whether it is shown.
async fn show_later(
    state: &super::AppState,
    device: &DeviceRef,
    base: &str,
    job: &portal_api::PlayJob,
    choice: CastSubtitle,
    still_wanted: &impl Fn() -> bool,
) -> bool {
    // A seek or skip holds the TV for a few seconds; this waits its turn
    // instead of dropping the subtitle for good.
    let mut tries = 0;
    let _moving = loop {
        if !still_wanted() {
            return false;
        }
        if let Some(guard) = SeekGuard::take() {
            break guard;
        }
        tries += 1;
        if tries >= LATER_GUARD_TRIES {
            return false;
        }
        tokio::time::sleep(LATER_GUARD_WAIT).await;
    };
    // A seek past the transcoded part restarts the stream at a new origin
    // meanwhile; an online file's offset follows the job as it is now.
    let now = state.0.player.job(&job.id);
    let Some((address, track_lang)) =
        tv_subtitle_address(&state.0.player, base, now.as_ref().unwrap_or(job), &choice)
    else {
        return false;
    };
    let mut args = vec!["subtitles".to_string(), "--id".into(), device.id.clone()];
    push_endpoint(&mut args, device.endpoint.as_ref());
    args.extend([
        "--subtitles".into(),
        address,
        format!("--subtitles-lang={track_lang}"),
    ]);
    args.extend(["--timeout".into(), "20".into()]);
    if run_agent(args, CONTROL_LIMIT).await.is_ok() && still_wanted() {
        lock_cast(state).set_device_subtitle(&device.id, Some(choice), Some(job.id.clone()), None);
        return true;
    }
    false
}

/// A subtitle in `lang` for `job`: the video's own if it is already read,
/// else the best online result for the episode the TV plays.
async fn subtitle_in_language(
    state: &super::AppState,
    job: &portal_api::PlayJob,
    lang: &str,
) -> Option<CastSubtitle> {
    if let Some(track) = track_in(&job.subtitles, lang) {
        return Some(CastSubtitle::Embedded {
            id: track.id,
            lang: Some(lang.to_string()),
        });
    }
    let item = lock_cast(state).device_view().item?;
    // An episode is searched by its show; without one (or for a film) the
    // title alone is the search, never the episode's name standing in as a show.
    let episode = item.show_title.is_some() && item.season.is_some() && item.episode.is_some();
    if !episode && item.kind != "movie" {
        return None;
    }
    let search = portal_api::SubtitleSearch {
        kind: Some(if episode {
            portal_api::MediaKind::Episode
        } else {
            portal_api::MediaKind::Movie
        }),
        title: Some(item.title.clone()),
        show_title: item.show_title.clone(),
        season: item.season,
        episode: item.episode,
        release: job.release.clone(),
        languages: vec![lang.to_string()],
        ..portal_api::SubtitleSearch::default()
    };
    let found = state.0.subtitles.search(&state.0.metadata, search).await;
    Some(CastSubtitle::Online {
        id: found.best?,
        lang: Some(lang.to_string()),
    })
}

/// The file sent to the TV ended. When the stream has a real tail left,
/// build a longer one and start it where the short file stopped.
async fn continue_cast_file(
    backend: &AppBackend,
    gen: u64,
    watched: &Watched,
    played: Option<f64>,
) -> watch::CastContinue {
    if !is_cast_file(&watched.media_url) {
        return watch::CastContinue::Done;
    }
    let Some(parts) = media_url_parts(&watched.media_url) else {
        return watch::CastContinue::Done;
    };
    let playlist = match backend
        .state
        .0
        .player
        .session_file(&parts.session, "index.m3u8")
    {
        Ok(path) if path.is_file() => path,
        _ => return watch::CastContinue::Done,
    };
    let span = url_span(&watched.media_url);
    // A file never seen playing still played through to be finished.
    let played_stream = played
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|at| at - watched.origin)
        .or(span);
    let Some(mut played_stream) = played_stream else {
        return watch::CastContinue::Done;
    };
    // The last look came a moment before the end. The file played through.
    if let Some(span) = span {
        if (0.0..=FINISH_GAP).contains(&(span - played_stream)) {
            played_stream = span;
        }
    }
    let played_source = watched.origin + played_stream;
    let staged = take_staged(&playlist, played_stream).await;
    let started = Instant::now();
    while staged.is_none() {
        let text = match std::fs::read_to_string(&playlist) {
            Ok(text) => text,
            Err(_) => return watch::CastContinue::Done,
        };
        let coverage = playlist_coverage(&text);
        let ended = playlist_ended(&text);
        let producing = backend
            .state
            .0
            .player
            .session_producing(&parts.session);
        match extend_choice(coverage, played_stream, ended) {
            ExtendChoice::Extend => break,
            ExtendChoice::Finish => return watch::CastContinue::Done,
            // Nothing more is coming, so whatever is left is cast now.
            ExtendChoice::Wait if !producing || started.elapsed() >= EXTEND_WAIT => {
                if coverage >= played_stream + EXTEND_FLOOR {
                    break;
                }
                // The encode stopped short of the end: the title did not
                // finish, so it is not marked watched and nothing follows.
                tracing::warn!("{STALLED_NOTE}");
                return watch::CastContinue::Failed(STALLED_NOTE.into());
            }
            ExtendChoice::Wait => tokio::time::sleep(Duration::from_secs(3)).await,
        }
    }
    watch::Backend::publish(
        backend,
        gen,
        Update::Status {
            state: DeviceState::Buffering,
            position: Some(played_source),
            duration: watched.job_duration,
            volume: None,
            muted: None,
        },
    );
    let version = match staged {
        Some(version) => version,
        None => match ensure_cast_mp4(&playlist).await {
            Ok(version) => version,
            Err(PrepareFail::NotReady) => return watch::CastContinue::Done,
            Err(PrepareFail::Failed) => {
                tracing::warn!("{PREPARE_NOTE}");
                return watch::CastContinue::Failed(PREPARE_NOTE.into());
            }
        },
    };
    let url = versioned_media(&watched.media_url, version);
    let start = start_within(&url, played_stream);
    let endpoint = watched.device.endpoint.clone();
    let title = watched
        .item
        .as_ref()
        .map(|item| clean_label(&item.title))
        .filter(|text| !text.is_empty());
    let art = watched.item.as_ref().and_then(|item| item.art.clone());
    let (base, choice) = {
        let hub = lock_cast(&backend.state);
        let base = hub.url_for_device(endpoint.as_ref());
        let view = hub.device_view();
        let choice = if view.subtitle_job.as_deref() == Some(watched.job_id.as_str()) {
            view.subtitle.clone()
        } else {
            None
        };
        (base, choice)
    };
    let job = backend.state.0.player.job(&watched.job_id);
    let shown = match (&choice, &job, &base) {
        (Some(choice), Some(job), Some(base)) => {
            tv_subtitle_address(&backend.state.0.player, base, job, choice)
        }
        _ => None,
    };
    let thumb = base
        .as_deref()
        .zip(art.as_deref())
        .and_then(|(base, art)| next_thumb(base, art));
    let args = play_args(
        &watched.device.id,
        endpoint.as_ref(),
        &url,
        title.as_deref(),
        thumb.as_deref(),
        start,
        shown
            .as_ref()
            .map(|(address, lang)| (address.as_str(), lang.as_str())),
    );
    if let Err(err) = run_agent(args, CONTROL_LIMIT).await {
        return watch::CastContinue::Failed(api_detail(&agent_error(err)));
    }
    let mut next = watched.clone();
    next.media_url = url;
    watch::CastContinue::Same(Box::new(next))
}

impl AppBackend {
    /// Casts a ready job to the device, `start` seconds into its stream.
    async fn cast_from(
        &self,
        device: &DeviceRef,
        job: &portal_api::PlayJob,
        title: &str,
        art: Option<&str>,
        start: f64,
    ) -> Result<String, String> {
        let playlist = job
            .playlist
            .as_deref()
            .ok_or("The stream is not ready to cast.")?;
        let base = {
            let hub = lock_cast(&self.state);
            if !hub.bound_lan {
                return Err("Turn on Allow TVs on this network, then cast again.".into());
            }
            hub.url_for_device(device.endpoint.as_ref())
                .ok_or("Portal could not find this PC's address on the TV's network.")?
        };
        let url = cast_media_url(&format!("{base}{playlist}")).map_err(str::to_string)?;
        let (url, start) = cast_target(&self.state.0.player, &url, Some(job), start)
            .await
            .map_err(|err| api_detail(&err))?;
        let title = clean_label(title);
        let thumb = art.and_then(|art| next_thumb(&base, art));
        let (current, wanted_lang, pending, ask) = {
            let hub = lock_cast(&self.state);
            let view = hub.device_view();
            let same = view.subtitle_job.as_deref() == Some(job.id.as_str());
            (
                view.subtitle.filter(|_| same),
                view.subtitle_lang,
                view.subtitle_pending && same,
                hub.latest_subtitle_ask(),
            )
        };
        // The pick for this job when it is ready; else, unless one is already
        // on its way, the video's own track in the wanted language when it is
        // read in a moment (the next episode), so it needs no reload later.
        let mut shown = None;
        if let Some(choice) = &current {
            if subtitle_ready(&self.state, job, choice).await {
                shown = tv_subtitle_address(&self.state.0.player, &base, job, choice)
                    .map(|address| (choice.clone(), address));
            }
        } else if let Some(lang) = wanted_lang.as_deref().filter(|_| !pending) {
            shown = quick_track(&self.state, job, lang)
                .await
                .and_then(|(choice, now)| {
                    tv_subtitle_address(&self.state.0.player, &base, &now, &choice)
                        .map(|address| (choice, address))
                });
        }
        let args = play_args(
            &device.id,
            device.endpoint.as_ref(),
            &url,
            Some(title.as_str()).filter(|text| !text.is_empty()),
            thumb.as_deref(),
            start_within(&url, start),
            shown
                .as_ref()
                .map(|(_, (address, lang))| (address.as_str(), lang.as_str())),
        );
        run_agent(args, CONTROL_LIMIT)
            .await
            .map_err(|err| api_detail(&agent_error(err)))?;
        let shown = shown.map(|(choice, _)| choice);
        // A pick made while the video loaded (the next episode starting) is
        // the one wanted now.
        let (pick, wanted_lang, changed) = {
            let hub = lock_cast(&self.state);
            if hub.latest_subtitle_ask() == ask {
                (current, wanted_lang, false)
            } else {
                let view = hub.device_view();
                let same = view.subtitle_job.as_deref() == Some(job.id.as_str());
                (view.subtitle.filter(|_| same), view.subtitle_lang, true)
            }
        };
        match shown {
            Some(choice) if !changed || pick.as_ref().is_none_or(|pick| *pick == choice) => {
                lock_cast(&self.state).set_device_subtitle(
                    &device.id,
                    Some(choice),
                    Some(job.id.clone()),
                    None,
                )
            }
            // Nothing shown is not the user turning subtitles off: the pick,
            // or one in the wanted language (the next episode's), follows if
            // it can be found.
            _ => {
                let want = pick
                    .map(Want::Choice)
                    .or_else(|| wanted_lang.clone().map(Want::Language));
                let mut hub = lock_cast(&self.state);
                match want {
                    // One already on its way for this job keeps coming.
                    Some(_) if pending && !changed => {
                        hub.pend_device_subtitle(&device.id, &job.id, wanted_lang)
                    }
                    Some(want) => {
                        hub.pend_device_subtitle(&device.id, &job.id, wanted_lang);
                        drop(hub);
                        send_subtitle_later(
                            self.state.clone(),
                            device.clone(),
                            job.clone(),
                            base,
                            want,
                        );
                    }
                    None => hub.set_device_subtitle(&device.id, None, Some(job.id.clone()), None),
                }
            }
        }
        Ok(url)
    }
}

impl watch::Backend for AppBackend {
    type Source = AgentWatch;

    fn watch(&self, device: &DeviceRef) -> AgentWatch {
        AgentWatch::new(device)
    }

    fn autoplay(&self) -> bool {
        crate::prefs::current(&self.state).autoplay
    }

    async fn job(&self, id: &str) -> watch::JobStep {
        use portal_api::{JobState, PromptKind};
        let Some(job) = self.state.0.player.job(id) else {
            return watch::JobStep::Gone;
        };
        match job.state {
            JobState::Ready if job.playlist.is_some() => watch::JobStep::Ready(Box::new(job)),
            JobState::Browse => watch::JobStep::Browse,
            JobState::Failed => watch::JobStep::Failed(
                job.error
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or_else(|| "The next episode did not play.".into()),
            ),
            _ => match self.state.0.prompts.read().await {
                Ok(Some(prompt)) if prompt.kind != PromptKind::Progress => watch::JobStep::Prompt,
                _ => watch::JobStep::Working,
            },
        }
    }

    async fn start(&self, request: portal_api::PlayRequest) -> Result<String, String> {
        crate::start_play_job(&self.state, request)
            .await
            .map_err(|err| api_detail(&err))
    }

    async fn stop(&self, id: &str) {
        let _ = self.state.0.player.stop(id).await;
    }

    async fn dismiss_prompt(&self) {
        use portal_api::{PromptAction, PromptAnswer, PromptKind};
        if let Ok(Some(prompt)) = self.state.0.prompts.read().await {
            if prompt.kind != PromptKind::Progress && prompt.cancellable {
                let answer = PromptAnswer {
                    id: prompt.id,
                    action: PromptAction::Cancel,
                    text: None,
                    index: None,
                };
                let _ = self.state.0.prompts.answer(&answer).await;
            }
        }
    }

    async fn cast(
        &self,
        device: &DeviceRef,
        job: &portal_api::PlayJob,
        title: &str,
        art: Option<&str>,
    ) -> Result<String, String> {
        self.cast_from(device, job, title, art, 0.0).await
    }

    async fn extend(
        &self,
        gen: u64,
        watched: &Watched,
        played: Option<f64>,
    ) -> watch::CastContinue {
        continue_cast_file(self, gen, watched, played).await
    }

    fn ahead(&self, watched: &Watched, at: f64) {
        stage_next_file(&self.state.0.player, watched, at);
    }

    fn near_end(&self, watched: &Watched) {
        // The same next item and request the autoplay chain will ask for.
        let fresh = self
            .state
            .0
            .player
            .job(&watched.job_id)
            .filter(|job| job.state == portal_api::JobState::Ready && job.playlist.is_some())
            .and_then(|job| job.next);
        let Some(next) = fresh
            .or_else(|| watched.next.clone())
            .filter(|next| !next.file.trim().is_empty())
        else {
            return;
        };
        crate::early::want(
            &self.state,
            &watched.job_id,
            watch::next_request(&next, watched.quality),
        );
    }

    fn progress(&self, report: &portal_api::ProgressReport) {
        // Saving rewrites the library file, so it runs off the async threads.
        let state = self.state.clone();
        let report = report.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = state
                .0
                .store
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            if let Err(err) = store.write_progress_from(&report, false, "cast") {
                tracing::warn!("cast progress was not saved: {err}");
            }
        });
    }

    fn publish(&self, gen: u64, update: Update) {
        let playing = matches!(
            &update,
            Update::Status {
                state: DeviceState::Playing,
                ..
            }
        );
        let job_id = {
            let mut hub = lock_cast(&self.state);
            hub.apply_device_update(gen, update);
            hub.device_view().job_id.filter(|_| playing)
        };
        // One long file can play for hours on a single request, so the TV
        // saying it plays is what keeps the stream from being cleared away.
        if let Some(job_id) = job_id {
            self.state.0.player.keep_alive(&job_id);
        }
    }
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/cast.rs"]
mod tests;
