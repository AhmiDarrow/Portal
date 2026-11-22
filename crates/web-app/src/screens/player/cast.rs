//! Casting to a Cast TV (player.ts `onCastClick`, `castToDevice`, `followDevice`,
//! `applyDeviceView`, `stopCasting`, `castEnded`, `resumeDeviceCast`; plan P10): the dock's
//! Cast button and its menu of devices, the cast itself through Portal's server
//! (`/api/cast/devices/{id}/*`, which drives the TV), and the player in remote mode while the
//! TV plays: play, pause, seeks, the TV's volume and mute, and Next go to the TV; the dock
//! follows `/api/cast/device` every 3 s, takes on the episode the TV moves on to, and hands
//! back to this browser where the TV stopped.
//!
//! The TV-browser receiver (`#/cast`) is not cast to from here (port plan, Q1).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::Duration;

use portal_api::{
    CastDeviceState, CastDeviceView, CastPlayBody, CastReplay, CastSubtitle, CastVolumeBody, JobState, NextItem,
    PlayJob, PlayKind, ReplayFields,
};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::time::{Instant, SystemTime, UNIX_EPOCH};
use remedy_ui::core::Insets;
use remedy_ui::input::{Key, PointerPhase};
use remedy_ui::layout::{Length, Position, AUTO};
use remedy_ui::playback::{PlaybackState, Remote, RemoteControl, RemoteStatus, Timeline};
use remedy_ui::reactive::{untrack, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::El;

use super::{dur, secs, target_from_next, Dock, Inner};
use crate::api;
use crate::theme::{mix, Look};
use crate::widgets::art::art_url;
use crate::widgets::play::PlayTarget;

/// How often the dock asks Portal what the TV does (player.ts `DEVICE_POLL`).
pub const DEVICE_POLL: Duration = Duration::from_secs(3);
/// A report older than this leaves the status line waiting for the TV (`DEVICE_STALE_MS`).
pub const DEVICE_STALE: Duration = Duration::from_secs(15);
/// How long a Play or Pause pressed here outlasts the TV's older reports (`DEVICE_WANT_MS`).
pub const DEVICE_WANT: Duration = Duration::from_secs(8);
/// How long a volume set here outlasts the TV's older reports (player.ts `tvVolume`).
pub const VOLUME_HOLD: Duration = Duration::from_secs(6);

/// The TV being cast to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
}

/// The dock's casting state.
pub(crate) struct Cast {
    /// The TV playing what the dock shows (`castReceiverId`, `castReceiverName`).
    pub(crate) device: Signal<Option<Device>>,
    /// A cast is being started or stopped (`castBusy`).
    busy: Signal<bool>,
    /// The device search runs (`castScanning`).
    scanning: Signal<bool>,
    menu: RefCell<Option<El>>,
    /// The Cast button, for focus when its menu closes.
    anchor: RefCell<Option<El>>,
    menu_open: Signal<bool>,
    /// Bumped to drop a device search whose menu is no longer wanted (`castMenuGen`).
    menu_gen: Cell<u64>,
    st: RefCell<Watch>,
}

#[derive(Default)]
struct Watch {
    /// Bumped to stop following (`deviceWatch`).
    generation: u64,
    /// Where the next ready job goes: Next, a quality change or a link while casting.
    pending: Option<Device>,
    last_ms: u64,
    seen: Option<Instant>,
    fresh_at: Option<Instant>,
    line: String,
    want: Option<(bool, Instant)>,
    volume_at: Option<Instant>,
    loading: Option<String>,
}

impl Cast {
    pub(crate) fn new() -> Cast {
        Cast {
            device: Signal::new(None),
            busy: Signal::new(false),
            scanning: Signal::new(false),
            menu: RefCell::default(),
            anchor: RefCell::default(),
            menu_open: Signal::new(false),
            menu_gen: Cell::new(0),
            st: RefCell::default(),
        }
    }

    /// The TV playing now, if any.
    pub(crate) fn current(&self) -> Option<Device> {
        self.device.get_untracked().ok().flatten()
    }

    /// Whether a TV plays now.
    pub(crate) fn active(&self) -> bool {
        self.device.with_untracked(Option::is_some).unwrap_or(false)
    }

    /// The TV the next ready job goes to.
    pub(crate) fn pending(&self) -> Option<Device> {
        self.st.borrow().pending.clone()
    }

    pub(crate) fn set_pending(&self, d: Option<Device>) {
        self.st.borrow_mut().pending = d;
    }

    pub(crate) fn take_pending(&self) -> Option<Device> {
        self.st.borrow_mut().pending.take()
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// player.ts `castFail`.
pub(super) fn cast_fail(e: &api::ApiError) -> String {
    if e.detail.trim().is_empty() { "Cast failed.".into() } else { e.detail.clone() }
}

/// player.ts `castMediaUrl`: a Portal address the TV can reach (a loopback host becomes this
/// PC's LAN address), or "" when there is none.
pub fn cast_media_url(playlist: &str, lan_base: &str) -> String {
    let path = playlist.trim();
    let base = lan_base.trim().trim_end_matches('/');
    if path.is_empty() {
        return String::new();
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let after = path.find("://").map_or(0, |i| i + 3);
        let end = path[after..].find(['/', '?', '#']).map_or(path.len(), |i| after + i);
        let authority = &path[after..end];
        let host = authority.rsplit('@').next().unwrap_or("");
        let host = if host.starts_with('[') { host.split(']').next().unwrap_or("").trim_start_matches('[') } else { host.split(':').next().unwrap_or("") };
        let local = matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1");
        return if local && !base.is_empty() { format!("{base}{}", &path[end..]) } else { path.to_owned() };
    }
    if base.is_empty() {
        return String::new();
    }
    if path.starts_with('/') { format!("{base}{path}") } else { format!("{base}/{path}") }
}

/// player.ts `castThumb`: the title's art at an address the TV can load.
fn cast_thumb(art: Option<&str>, base: &str) -> Option<String> {
    let resolved = art_url(art)?;
    let lower = resolved.to_ascii_lowercase();
    let usable = lower.starts_with("http://") || lower.starts_with("https://") || resolved.starts_with('/');
    usable.then(|| cast_media_url(&resolved, base)).filter(|u| !u.is_empty())
}

/// player.ts `deviceLabel`: the name, and the model when the name does not say it.
pub fn device_label(name: &str, model: &str) -> String {
    let name = if name.trim().is_empty() { "Cast device" } else { name.trim() };
    let model = model.trim();
    if model.is_empty() || name.to_lowercase().contains(&model.to_lowercase()) {
        return name.to_owned();
    }
    format!("{name} · {model}")
}

/// player.ts `castReplayFromTarget`: what the TV plays, so Portal can play it again.
fn replay(t: Option<&PlayTarget>) -> ReplayFields {
    let Some(t) = t.filter(|t| !t.file.trim().is_empty()) else { return ReplayFields::default() };
    ReplayFields {
        kind: serde_json::to_value(t.kind).ok().and_then(|v| v.as_str().map(str::to_owned)),
        id: Some(t.id.clone()),
        file: Some(t.file.clone()),
        season: t.season,
        episode: t.episode,
        art: t.art.clone(),
        show_title: t.show_title.clone(),
        show_id: t.show_id.clone(),
    }
}

/// player.ts `targetFromCast`: what the TV moved on to, as the dock's item.
fn target_from_cast(item: Option<&CastReplay>) -> Option<PlayTarget> {
    let item = item?;
    let kind: PlayKind = serde_json::from_value(serde_json::Value::String(item.kind.clone())).ok()?;
    if item.file.trim().is_empty() || item.id.trim().is_empty() {
        return None;
    }
    Some(target_from_next(&NextItem {
        kind,
        id: item.id.trim().to_owned(),
        file: item.file.trim().to_owned(),
        title: item.title.trim().to_owned(),
        season: item.season,
        episode: item.episode,
        art: item.art.clone(),
        show_title: item.show_title.clone(),
        show_id: item.show_id.clone(),
    }))
}

/// player.ts `deviceStatusLine`.
fn status_line(state: Option<CastDeviceState>, name: &str, quiet: bool) -> String {
    match state {
        Some(CastDeviceState::Next) => format!("Starting the next episode on {name}…"),
        _ if quiet => format!("Waiting for {name} to answer…"),
        Some(CastDeviceState::Starting) => format!("Starting on {name}…"),
        Some(CastDeviceState::Buffering) => format!("Loading on {name}…"),
        _ => format!("Casting to {name}."),
    }
}

/// The subtitle the TV should show with a new cast (player.ts `castSubtitle` when CC is on).
fn cast_subtitle(d: &Dock) -> Option<CastSubtitle> {
    d.cast_subtitle()
}

// ---- The player's controls, on the TV ----

/// What the player's controls do in remote mode.
struct Control(Weak<Inner>);

impl Control {
    fn with(&self, f: impl FnOnce(&Dock, Device)) {
        if let Some(i) = self.0.upgrade() {
            let d = Dock(i);
            if let Some(dev) = d.0.cast.current() {
                f(&d, dev);
            }
        }
    }
}

impl RemoteControl for Control {
    fn play(&self) {
        self.with(|d, dev| toggle(d, dev, false));
    }
    fn pause(&self) {
        self.with(|d, dev| toggle(d, dev, true));
    }
    fn seek(&self, to: Duration) {
        // The server places the seek in the TV's stream; the player holds the target until
        // the TV reports it is there.
        self.with(|d, dev| {
            let id = dev.id.clone();
            d.run(api::cast_device_seek(&dev.id, secs(to).floor()), move |d, out| {
                if let Err(e) = out {
                    if d.0.cast.current().is_some_and(|c| c.id == id) {
                        d.set_status(&cast_fail(&e));
                    }
                }
            });
        });
    }
    fn set_volume(&self, volume: f32) {
        self.with(|d, dev| send_volume(d, dev, CastVolumeBody { level: Some(volume as f64), muted: None }));
    }
    fn set_muted(&self, muted: bool) {
        self.with(|d, dev| send_volume(d, dev, CastVolumeBody { level: None, muted: Some(muted) }));
    }
    fn set_captions(&self, _track: Option<usize>) {
        // CC while casting goes to the TV from the dock's pick (`subs.rs` `sync_device_subs`).
    }
    fn next(&self) {
        // Next while casting: the next item is made and goes to the same TV (player.ts `start`).
        if let Some(i) = self.0.upgrade() {
            let d = Dock(i);
            if let Some(n) = d.0.next.get_untracked().ok().flatten() {
                d.play(target_from_next(&n));
            }
        }
    }
}

/// player.ts `toggleDevicePlay`: shown at once, held over older reports until the TV says so.
fn toggle(d: &Dock, dev: Device, pause: bool) {
    d.0.cast.st.borrow_mut().want = Some((pause, Instant::now()));
    let id = dev.id.clone();
    let call = if pause { api::cast_device_pause(&dev.id) } else { api::cast_device_resume(&dev.id) };
    d.run(call, move |d, out| {
        if let Err(e) = out {
            if d.0.cast.current().is_some_and(|c| c.id == id) {
                d.0.cast.st.borrow_mut().want = None;
                d.set_status(&cast_fail(&e));
            }
        }
    });
}

/// player.ts `setTvVolume`: the player already waited for the slider to rest.
fn send_volume(d: &Dock, dev: Device, change: CastVolumeBody) {
    d.0.cast.st.borrow_mut().volume_at = Some(Instant::now());
    let id = dev.id.clone();
    d.run(api::cast_device_volume(&dev.id, &change), move |d, out| {
        if let Err(e) = out {
            if d.0.cast.current().is_some_and(|c| c.id == id) {
                d.set_status(&cast_fail(&e));
            }
        }
    });
}

/// The title's art over the casting stage (player.ts `paintCastStage`'s `--cast-art`).
fn poster(d: &Dock) -> Option<El> {
    let t = d.0.target.get_untracked().ok().flatten()?;
    let url = art_url(t.art.as_deref())?;
    let img = remedy_ui::image::image_ext(
        &d.0.ui,
        remedy_ui::image::Source::Url(url),
        "",
        remedy_ui::image::Fit::Cover,
        remedy_ui::image::ImageOptions::new(),
    );
    Some(img.layout(|l| {
        l.width = Length::Percent(1.0);
        l.height = Length::Percent(1.0);
    }))
}

// ---- Starting, following and ending a cast ----

/// player.ts `castStart`: seconds into `job`'s stream where the TV starts (where things are
/// now: this browser, another TV, or where a cast stopped).
fn cast_start(d: &Dock, job: &PlayJob) -> f64 {
    let (attached, left, origin) = {
        let st = d.0.st.borrow();
        (st.attached, st.left_at, st.origin)
    };
    let base = job.position.filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(origin);
    let here = if d.0.cast.active() || attached { secs(untrack(|| d.0.player.position())) } else { left };
    if here.is_finite() { (here - base).max(0.0) } else { 0.0 }
}

/// player.ts `castToDevice`: the current job's stream to `dev`, from where it is now.
pub(crate) fn cast_to(d: &Dock, dev: Device) {
    close_menu(d, false);
    let Some(job_id) = d.0.st.borrow().job.clone() else { return };
    let c = &d.0.cast;
    // Never two TVs: the one playing stops before another starts.
    let previous = c.current().filter(|p| p.id != dev.id);
    c.busy.set(true);
    let j2 = job_id.clone();
    d.run(api::job(&job_id), move |d, job| {
        d.run(api::cast_info(), move |d, info| {
            let still = d.0.st.borrow().job.as_deref() == Some(j2.as_str());
            let fail = |d: &Dock, text: String| {
                d.0.cast.busy.set(false);
                if still {
                    d.set_status(&text);
                }
            };
            let (job, info) = match (job, info) {
                (Ok(j), Ok(i)) => (j, i),
                (Err(e), _) | (_, Err(e)) => return fail(d, cast_fail(&e)),
            };
            if !still {
                return d.0.cast.busy.set(false);
            }
            let base = info.url.as_deref().unwrap_or("").trim().to_owned();
            let url = cast_media_url(job.playlist.as_deref().unwrap_or(""), &base);
            if url.is_empty() {
                let text = if !base.is_empty() {
                    "The stream is not ready to cast."
                } else if !info.lan {
                    "Turn on Allow TVs on this network, then cast again."
                } else {
                    "Portal could not find this PC's network address. If you just turned on Allow TVs, restart Portal; otherwise check this PC is on the TV's network."
                };
                return fail(d, text.into());
            }
            let target = d.0.target.get_untracked().ok().flatten();
            let title = target
                .as_ref()
                .map(|t| t.title.trim().to_owned())
                .filter(|t| !t.is_empty())
                .or_else(|| job.title.clone().filter(|t| !t.trim().is_empty()))
                .unwrap_or_else(|| d.0.title.get_untracked().unwrap_or_default())
                .trim()
                .to_owned();
            let body = CastPlayBody {
                url: Some(url),
                title: (!title.is_empty()).then_some(title),
                thumb: cast_thumb(target.as_ref().and_then(|t| t.art.as_deref()), &base),
                job_id: Some(j2.clone()),
                start: Some(cast_start(d, &job)),
                subtitle: cast_subtitle(d),
                replay: replay(target.as_ref()),
            };
            let (dev2, j3) = (dev.clone(), j2.clone());
            let play = move |d: &Dock| {
                d.run(api::cast_device_play(&dev2.id, &body), move |d, out| {
                    d.0.cast.busy.set(false);
                    if d.0.st.borrow().job.as_deref() != Some(j3.as_str()) {
                        return;
                    }
                    match out {
                        Ok(_) => begin(d, dev2),
                        Err(e) => d.set_status(&cast_fail(&e)),
                    }
                });
            };
            match previous {
                Some(p) => d.run(api::cast_device_stop(&p.id), move |d, _| play(d)),
                None => play(d),
            }
        });
    });
}

/// The TV has the stream: the player becomes its remote and the dock follows it.
fn begin(d: &Dock, dev: Device) {
    let name = dev.name.clone();
    d.0.cast.device.set(Some(dev));
    d.0.cast.st.borrow_mut().want = None;
    let mut remote = Remote::new(name.clone(), Control(d.weak()));
    if let Some(p) = poster(d) {
        remote = remote.poster(p);
    }
    d.0.player.set_remote(Some(remote));
    {
        // The TV has the stream now; Play after the cast picks up from where the TV left it.
        let mut st = d.0.st.borrow_mut();
        st.attached = false;
        st.left_at = 0.0;
    }
    d.set_status(&format!("Casting to {name}."));
    d.0.ctx.activity.notice("");
    follow(d);
}

/// Stops following: the TV's state is no longer the player's.
fn leave_state(d: &Dock) {
    let c = &d.0.cast;
    {
        let mut st = c.st.borrow_mut();
        st.generation += 1;
        st.last_ms = 0;
        st.seen = None;
        st.fresh_at = None;
        st.line.clear();
        st.want = None;
        st.volume_at = None;
    }
    c.device.set_if_changed(None);
    d.0.player.set_remote(None);
}

/// player.ts `releaseCurrent`'s share: the TV stops with the item. True when one played.
pub(crate) fn release(d: &Dock) -> bool {
    close_menu(d, false);
    d.0.cast.set_pending(None);
    let Some(dev) = d.0.cast.current() else { return false };
    d.run(api::cast_device_stop(&dev.id), |_, _| {});
    leave_state(d);
    true
}

/// player.ts `castEnded`: the dock stays on the item, ready to play here from where the TV
/// was. Play asks for the stream again from there ([`Dock::retry`]).
fn ended(d: &Dock, text: String) {
    let left = secs(untrack(|| d.0.player.position()));
    leave_state(d);
    if left > 0.0 {
        d.0.st.borrow_mut().left_at = left;
    }
    d.0.player.fail(text.clone());
    d.set_status(&text);
}

/// player.ts `stopCasting`: back to this browser.
fn stop_casting(d: &Dock) {
    close_menu(d, false);
    let Some(dev) = d.0.cast.current() else { return };
    d.0.cast.busy.set(true);
    // A look answered meanwhile would see the cast gone and say "Casting ended.".
    d.0.cast.st.borrow_mut().generation += 1;
    d.run(api::cast_device_stop(&dev.id), move |d, out| {
        d.0.cast.busy.set(false);
        if !d.0.cast.current().is_some_and(|c| c.id == dev.id) {
            return;
        }
        match out {
            Ok(_) => ended(d, "Stopped casting.".into()),
            Err(e) => {
                d.set_status(&cast_fail(&e));
                follow(d);
            }
        }
    });
}

/// player.ts `followDevice`: asks Portal what the TV does every 3 s while it plays.
fn follow(d: &Dock) {
    let token = {
        let mut st = d.0.cast.st.borrow_mut();
        st.generation += 1;
        st.last_ms = 0;
        st.seen = None;
        st.fresh_at = None;
        st.generation
    };
    look(d, token);
}

fn look(d: &Dock, token: u64) {
    let id = d.0.cast.current().map(|c| c.id);
    d.run(api::cast_device(), move |d, out| {
        if d.0.cast.st.borrow().generation != token || d.0.cast.current().map(|c| c.id) != id {
            return;
        }
        // Portal did not answer this time: the next look tries again.
        if let Ok(view) = out {
            apply(d, &view);
        }
        if d.0.cast.st.borrow().generation == token {
            d.after(DEVICE_POLL, move |d| look(d, token));
        }
    });
}

/// player.ts `applyDeviceView`: what Portal last heard from the TV, into the player.
fn apply(d: &Dock, view: &CastDeviceView) {
    let Some(dev) = d.0.cast.current() else { return };
    let name = dev.name.clone();
    match view.device_id.as_deref() {
        // Portal no longer watches this cast (it restarted, or the job had gone).
        None => {
            return ended(d, format!("Portal stopped following {name}. It may still be playing there; cast again to control it here."))
        }
        Some(id) if id != dev.id => return ended(d, format!("Stopped casting to {name}.")),
        _ => {}
    }
    if let Some(job) = view.job_id.as_deref().filter(|j| !j.trim().is_empty()) {
        let (current, loaded) = {
            let st = d.0.st.borrow();
            (st.job.clone(), st.last_ready.is_some())
        };
        if current.as_deref() != Some(job) {
            adopt(d, job, view.item.as_ref(), view.title.as_deref());
        } else if !loaded && view.state != Some(CastDeviceState::Next) {
            load_job(d, job);
        }
    }
    if !view.active {
        let failed = view.state == Some(CastDeviceState::Failed);
        let note = view.note.clone().filter(|n| !n.trim().is_empty());
        return ended(d, note.unwrap_or_else(|| if failed { "Casting failed.".into() } else { "Casting ended.".into() }));
    }
    let p = &d.0.player;
    let now = Instant::now();
    let mut state = match view.state {
        Some(CastDeviceState::Starting) => PlaybackState::Opening,
        Some(CastDeviceState::Paused | CastDeviceState::Stopped) => PlaybackState::Paused,
        Some(CastDeviceState::Buffering | CastDeviceState::Next | CastDeviceState::Idle) | None => PlaybackState::Buffering,
        Some(CastDeviceState::Ended) => PlaybackState::Ended,
        Some(CastDeviceState::Failed) => PlaybackState::Failed,
        Some(CastDeviceState::Playing) => PlaybackState::Playing,
    };
    let mut position = dur(view.position.filter(|p| p.is_finite()).unwrap_or(0.0));
    let mut reported = now;
    let mut st = d.0.cast.st.borrow_mut();
    let waiting = match st.want {
        Some((paused, since)) => (view.state == Some(CastDeviceState::Paused)) != paused && since.elapsed() < DEVICE_WANT,
        None => false,
    };
    if waiting {
        // The TV has not shown the last Play or Pause yet; this older report would undo it.
        let paused = st.want.is_some_and(|w| w.0);
        state = if paused { PlaybackState::Paused } else { PlaybackState::Playing };
        position = untrack(|| p.position());
    } else {
        st.want = None;
        // A report seen before moves nothing on: the clock runs on from the first sight of it.
        if st.last_ms != view.updated_ms || st.seen.is_none() {
            let age = now_ms().saturating_sub(view.updated_ms);
            st.last_ms = view.updated_ms;
            st.fresh_at = Some(now);
            st.seen = Some(if age < 10_000 { now.checked_sub(Duration::from_millis(age)).unwrap_or(now) } else { now });
        }
        reported = st.seen.unwrap_or(now);
    }
    let length = view.duration.filter(|l| l.is_finite() && *l > 0.0).or_else(|| {
        let j = d.0.st.borrow().job_duration;
        (j > 0.0).then_some(j)
    });
    let mut status = RemoteStatus::new(state, position).reported_at(reported);
    if let Some(l) = length {
        status = status.length(dur(l));
    }
    // The TV's own volume, once a change from here has had time to reach it.
    let held = st.volume_at.is_some_and(|t| t.elapsed() < VOLUME_HOLD);
    let (volume, muted) = if held {
        (Some(untrack(|| p.volume()) as f64), Some(untrack(|| p.is_muted())))
    } else {
        (view.volume, view.muted)
    };
    if let Some(v) = volume.filter(|v| v.is_finite()) {
        status = status.volume(v as f32);
    }
    status = status.muted(muted.unwrap_or(false));
    let quiet = st.fresh_at.is_some_and(|t| t.elapsed() > DEVICE_STALE);
    let line = status_line(view.state, &name, quiet);
    let changed = line != st.line;
    if changed {
        st.line = line.clone();
    }
    drop(st);
    if let Some(l) = length {
        // The seek bar spans what the TV plays: a timeline left from the item that played
        // here (or the episode before) would keep its length.
        p.set_timeline(Some(Timeline::new(Duration::ZERO).length(dur(l))));
    }
    p.set_remote_status(status);
    // Written when it changes, so an error stays until there is news.
    if changed {
        d.set_status(&line);
    }
}

/// player.ts `adoptCastJob`: the TV moved on to `job`; the dock shows it.
fn adopt(d: &Dock, job: &str, item: Option<&CastReplay>, title: Option<&str>) {
    {
        let mut st = d.0.st.borrow_mut();
        st.job = Some(job.to_owned());
        st.last_ready = None;
        st.left_at = 0.0;
        st.origin = 0.0;
        st.job_duration = 0.0;
        st.attached = false;
    }
    d.0.job.set(Some(job.to_owned()));
    if let Some(t) = target_from_cast(item) {
        d.0.target.set(Some(t));
    }
    d.set_next(None);
    d.clear_play_activity();
    let target = d.0.target.get_untracked().ok().flatten();
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| target.as_ref().map(|t| t.title.clone()))
        .unwrap_or_default();
    d.0.title.set(title.clone());
    if let Some(t) = &target {
        d.0.player.set_title(Some(super::view::stage_title(t, &title)));
    }
    d.0.player.set_remote_poster(poster(d));
    d.request_episodes();
    load_job(d, job);
}

/// player.ts `loadCastJob`: the next episode and length of a job the TV started.
fn load_job(d: &Dock, job: &str) {
    {
        let mut st = d.0.cast.st.borrow_mut();
        if st.loading.as_deref() == Some(job) {
            return;
        }
        st.loading = Some(job.to_owned());
    }
    let id = job.to_owned();
    d.run(api::job(job), move |d, out| {
        {
            let mut st = d.0.cast.st.borrow_mut();
            if st.loading.as_deref() == Some(id.as_str()) {
                st.loading = None;
            }
        }
        // The next look at the cast tries again.
        let Ok(job) = out else { return };
        if d.0.st.borrow().job.as_deref() != Some(id.as_str()) {
            return;
        }
        if job.state == JobState::Ready && job.playlist.is_some() {
            d.0.st.borrow_mut().last_ready = Some(job.clone());
            d.paint_audio(Some(&job));
        }
        if let Some(l) = job.duration.filter(|l| l.is_finite() && *l > 0.0) {
            d.0.st.borrow_mut().job_duration = l;
        }
        d.set_next(job.next.clone());
    });
}

/// player.ts `resumeDeviceCast`: a dashboard opened while Portal casts picks that cast up.
pub(crate) fn resume(d: &Dock) {
    let token = d.generation();
    d.run(api::cast_device(), move |d, out| {
        let Ok(view) = out else { return };
        let job = view.job_id.clone().filter(|j| !j.trim().is_empty());
        let id = view.device_id.clone().filter(|j| !j.trim().is_empty());
        let (Some(job), Some(id)) = (job, id) else { return };
        if d.generation() != token || d.0.st.borrow().job.is_some() || !view.active {
            return;
        }
        let name = view.device_name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "Cast device".into());
        d.0.target.set(None);
        d.0.open.set_if_changed(true);
        let dev = Device { id, name: name.clone() };
        d.0.cast.device.set(Some(dev));
        d.0.player.set_remote(Some(Remote::new(name, Control(d.weak()))));
        adopt(d, &job, view.item.as_ref(), view.title.as_deref());
        follow(d);
        apply(d, &view);
    });
}

/// The audio track changed while casting (player.ts `applyAudio`): Portal remakes the stream
/// on the new track and the TV is sent it again.
pub(crate) fn switch_audio(d: &Dock, dev: Device, job: String, index: u32, resume: f64) {
    d.set_status("Switching audio…");
    let id = job.clone();
    d.run(api::set_audio(&job, index, resume), move |d, out| {
        if d.0.st.borrow().job.as_deref() != Some(id.as_str()) {
            return;
        }
        let last = d.0.st.borrow().last_ready.clone();
        match out {
            Ok(j) if j.playlist.is_some() => {
                {
                    let mut st = d.0.st.borrow_mut();
                    st.origin = j.position.filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(st.origin);
                    st.last_ready = Some(j.clone());
                }
                d.paint_audio(Some(&j));
                cast_to(d, dev);
            }
            Ok(j) => {
                d.set_status(&j.error.unwrap_or_else(|| "The audio track did not start.".into()));
                d.paint_audio(last.as_ref());
            }
            Err(e) => {
                d.set_status(&e.detail);
                d.paint_audio(last.as_ref());
            }
        }
    });
}

// ---- The Cast button and its menu ----

/// `#dock-cast`: "Casting" in gold while a TV plays; its menu lists the devices.
pub(crate) fn button(d: &Dock) -> El {
    let i = d.inner();
    let c = &i.cast;
    let (device, busy, scanning, job, open) = (c.device, c.busy, c.scanning, i.job, c.menu_open);
    let casting = move || device.with(Option::is_some) && job.with(Option::is_some);
    let c2 = casting.clone();
    let label = move || if c2() { "Casting".to_owned() } else { "Cast".to_owned() };
    let l2 = label.clone();
    let c3 = casting.clone();
    let b = crate::widgets::button(&i.ui, label, || {})
        .variant(move || if c3() { "casting" } else { "" })
        .disabled(move || busy.get() || scanning.get() || job.with(Option::is_none));
    let (me, anchor) = (d.clone(), b.clone());
    let b = b.on_click(move || on_click(&me, &anchor)).semantics(move || {
        Semantics::new(Role::Button).label(l2()).expanded(open.get()).action(Action::Click)
    });
    *c.anchor.borrow_mut() = Some(b.clone());
    let c4 = casting.clone();
    w::tooltip(&b, move || {
        if c4() {
            "Casting"
        } else if job.with(Option::is_some) {
            "Cast this video"
        } else {
            "Cast"
        }
        .to_owned()
    });
    // Minimizing closes the menu (player.ts `setMini`).
    let (me, pl) = (d.clone(), i.player.clone());
    b.bind(move || {
        if pl.is_compact() {
            untrack(|| close_menu(&me, false));
        }
    })
}

/// player.ts `onCastClick`: looks for devices; one device and no cast casts at once.
fn on_click(d: &Dock, anchor: &El) {
    let c = &d.0.cast;
    let Some(job) = d.0.st.borrow().job.clone() else { return };
    if c.menu.borrow().is_some() {
        c.menu_gen.set(c.menu_gen.get() + 1);
        close_menu(d, true);
        return;
    }
    if c.busy.get_untracked().unwrap_or(false) || c.scanning.get_untracked().unwrap_or(false) {
        return;
    }
    let token = c.menu_gen.get() + 1;
    c.menu_gen.set(token);
    // Its own flag: Pause, ±10 and the volume still reach the TV while the search runs.
    c.scanning.set(true);
    let prior = d.0.status.get_untracked().unwrap_or_default();
    const LOOKING: &str = "Looking for devices…";
    d.set_status(LOOKING);
    let anchor = anchor.clone();
    d.run(api::cast_devices(), move |d, out| {
        let c = &d.0.cast;
        c.scanning.set(false);
        let clear_looking = |d: &Dock| {
            if d.0.status.get_untracked().unwrap_or_default() == LOOKING {
                d.set_status(&prior);
            }
        };
        if c.menu_gen.get() != token || d.0.st.borrow().job.as_deref() != Some(job.as_str()) {
            return clear_looking(d);
        }
        let devices: Vec<Device> = match out {
            Ok(list) => list
                .items
                .into_iter()
                .filter(|x| !x.id.trim().is_empty())
                .map(|x| Device { id: x.id.trim().to_owned(), name: device_label(&x.name, &x.model) })
                .collect(),
            Err(e) => return d.set_status(&cast_fail(&e)),
        };
        let casting = c.active();
        if devices.is_empty() && !casting {
            return d.set_status("No Cast device answered.");
        }
        if !casting && devices.len() == 1 {
            return cast_to(d, devices[0].clone());
        }
        clear_looking(d);
        show_menu(d, &anchor, devices, casting);
    });
}

/// `#dock-cast-menu`: Stop casting, the devices (the one playing says so), Cancel; above the
/// button, as Portal's.
fn show_menu(d: &Dock, anchor: &El, devices: Vec<Device>, casting: bool) {
    let ui = d.0.ui.clone();
    let current = d.0.cast.current();
    let mut items: Vec<(String, Rc<dyn Fn()>)> = Vec::new();
    if casting {
        let me = d.clone();
        items.push(("Stop casting".into(), Rc::new(move || stop_casting(&me))));
    }
    for dev in devices {
        let me = d.clone();
        if current.as_ref().is_some_and(|c| c.id == dev.id) {
            items.push((format!("{} · playing", dev.name), Rc::new(move || close_menu(&me, true))));
        } else {
            items.push((dev.name.clone(), Rc::new(move || cast_to(&me, dev.clone()))));
        }
    }
    let me = d.clone();
    items.push(("Cancel".into(), Rc::new(move || close_menu(&me, true))));
    let focus = Signal::new(0usize);
    let mut list = ui.el("p_cast_menu").column().scroll().semantics(Semantics::new(Role::Menu).label("Cast"));
    let mut buttons = Vec::new();
    for (n, (label, act)) in items.into_iter().enumerate() {
        let l2 = label.clone();
        let b = crate::widgets::button(&ui, label, move || act())
            .variant("menu")
            .fill_width()
            .on_hover(move |on| {
                if on {
                    focus.set(n);
                }
            })
            .semantics(Semantics::new(Role::MenuItem).label(l2).action(Action::Click));
        let _ = b.clone().max_lines(3);
        buttons.push(b.clone());
        list = list.child(b);
    }
    let n = buttons.len();
    let bs = buttons.clone();
    let move_to = move |i: usize| {
        focus.set(i);
        bs[i].focus();
    };
    let me = d.clone();
    let list = list.on_key(move |e| {
        if !e.pressed {
            return false;
        }
        let at = focus.get_untracked().unwrap_or(0);
        match e.key {
            Key::Escape => close_menu(&me, true),
            Key::ArrowDown => move_to((at + 1) % n),
            Key::ArrowUp => move_to((at + n - 1) % n),
            Key::Home => move_to(0),
            Key::End => move_to(n - 1),
            _ => return false,
        }
        true
    });
    let me = d.clone();
    let list = list.on_focus_within(move |inside| {
        if !inside {
            close_menu(&me, false);
        }
    });
    // Above the button, at its start: `bottom: calc(100% + 0.35rem); left: 0`.
    let at = anchor.bounds().unwrap_or_default();
    let window = ui.window_size();
    let max_w = (window.width - 24.0).min(288.0);
    let min_w = 192.0f32.min(max_w);
    let x = at.min.x.clamp(4.0, (window.width - min_w - 4.0).max(4.0));
    let bottom = (window.height - at.min.y + 5.6).max(4.0);
    let list = list.layout(move |l| {
        l.position = Position::Absolute;
        l.inset = Insets { top: AUTO, start: x, end: AUTO, bottom };
        l.inset_physical = true;
        l.min_width = Length::Px(min_w);
        l.max_width = Length::Px(max_w);
        l.max_height = Length::Px(256.0f32.min(at.min.y - 10.0).max(64.0));
        l.gap = (3.2, 3.2);
    });
    let me = d.clone();
    let catcher = ui
        .el("p_catch")
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
        })
        .pass_through()
        .on_pointer(move |e| {
            // A press on the Cast button itself is its own: it closes the menu (`#dock-cast-wrap`).
            if e.phase == PointerPhase::Down && !at.contains(e.position) {
                close_menu(&me, false);
            }
            false
        });
    let layer = ui.overlay(
        ui.el("p_cast_layer")
            .layout(|l| {
                l.position = Position::Absolute;
                l.inset = Insets::ZERO;
            })
            .pass_through()
            .child(catcher)
            .child(list),
    );
    *d.0.cast.menu.borrow_mut() = Some(layer);
    d.0.cast.menu_open.set(true);
    if let Some(b) = buttons.first() {
        b.focus();
    }
}

/// player.ts `hideCastMenu`; `refocus` puts focus back on the Cast button.
fn close_menu(d: &Dock, refocus: bool) {
    let Some(layer) = d.0.cast.menu.borrow_mut().take() else { return };
    layer.remove();
    d.0.cast.menu_open.set(false);
    if refocus {
        if let Some(b) = d.0.cast.anchor.borrow().clone() {
            b.focus();
        }
    }
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    // #dock-cast.is-casting { color: var(--gold); border-color: var(--gold) }
    s.rule("p_button", Some("casting"), States::NONE, StylePatch::default().foreground(k.gold).border(k.gold));
    // #dock-cast-menu
    s.rule(
        "p_cast_menu",
        None,
        States::NONE,
        StylePatch::default()
            .background(mix(k.raised, 0.94, k.bg))
            .border(k.line)
            .border_width(1.0)
            .radius(12.0)
            .padding(Insets::all(5.6))
            .shadow(k.shadow_lift)
            .backdrop_blur(12.0),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_urls_reach_the_tv() {
        let base = "http://192.168.0.2:8490";
        assert_eq!(cast_media_url("/api/hls/a/index.m3u8", base), "http://192.168.0.2:8490/api/hls/a/index.m3u8");
        assert_eq!(cast_media_url("api/x", base), "http://192.168.0.2:8490/api/x");
        assert_eq!(cast_media_url("http://127.0.0.1:8490/api/x?y=1", base), "http://192.168.0.2:8490/api/x?y=1");
        assert_eq!(cast_media_url("http://[::1]:8490/a", base), "http://192.168.0.2:8490/a");
        assert_eq!(cast_media_url("https://cdn.example/p.jpg", base), "https://cdn.example/p.jpg");
        assert_eq!(cast_media_url("/api/x", ""), "");
        assert_eq!(cast_media_url("", base), "");
        assert_eq!(cast_thumb(Some("image://x/"), base).as_deref(), Some("http://192.168.0.2:8490/api/image?path=image%3A%2F%2Fx%2F"));
        assert_eq!(cast_thumb(Some("weird"), base), None);
    }

    #[test]
    fn labels_and_lines_as_player_ts() {
        assert_eq!(device_label("The  Ris", "Chromecast"), "The  Ris · Chromecast");
        assert_eq!(device_label("Living room Chromecast", "chromecast"), "Living room Chromecast");
        assert_eq!(device_label(" ", ""), "Cast device");
        assert_eq!(status_line(Some(CastDeviceState::Next), "TV", false), "Starting the next episode on TV…");
        assert_eq!(status_line(Some(CastDeviceState::Playing), "TV", true), "Waiting for TV to answer…");
        assert_eq!(status_line(Some(CastDeviceState::Paused), "TV", false), "Casting to TV.");
        let item = CastReplay {
            kind: "episode".into(),
            id: "show".into(),
            file: "f".into(),
            title: "Pilot".into(),
            season: Some(1),
            episode: Some(1),
            art: None,
            show_title: Some("Show".into()),
            show_id: Some("show".into()),
        };
        let t = target_from_cast(Some(&item)).expect("a target");
        assert_eq!((t.kind, t.id.as_str()), (PlayKind::Episode, "show"));
        assert!(target_from_cast(Some(&CastReplay { kind: "weird".into(), ..item })).is_none());
    }
}
