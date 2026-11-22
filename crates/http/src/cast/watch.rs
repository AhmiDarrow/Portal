//! Watching a Chromecast-type device while Portal casts to it.
//!
//! The cast helper's `watch` command keeps one connection to the device and
//! prints its media status every few seconds, so there is one Python process
//! per cast rather than one per look. `Tracker` reads those lines and decides
//! when the item has finished. When it has, Autoplay next is on, and the job
//! has a next episode, `drive` starts that episode's play job here in the
//! service and casts it to the same device, so it works with the dashboard
//! closed. A Stop from Portal cancels the watch; a stop on the TV (idle
//! reason CANCELLED), an error, or something else cast over it ends it
//! without playing anything next.

use std::future::Future;
use std::time::Duration;

use portal_api::{NextItem, PlayJob, PlayKind, PlayRequest, ProgressReport, Quality, TitleKind};
use tokio::sync::watch;
use tokio::time::Instant;

use super::{is_cast_file, same_media_path, CastReplay, DeviceEndpoint};

/// Seconds between status lines from the helper.
pub(crate) const WATCH_INTERVAL: Duration = Duration::from_secs(4);
/// Idle within this many seconds of the end, with no reason given, counts as finished.
const NEAR_END: f64 = 15.0;
/// Status lines to wait for the item to start (about two minutes).
const START_POLLS: u32 = 30;
/// Idle lines, with no reason and not near the end, before the cast counts as closed.
const IDLE_POLLS: u32 = 3;
/// Failed lines in a row before the device counts as gone (about half a minute).
const LOST_POLLS: u32 = 8;
const PROGRESS_EVERY: Duration = Duration::from_secs(30);
/// How long the next episode may take to resolve and transcode its first
/// segment, room enough for the engine's one retry after a timeout.
const RESOLVE_LIMIT: Duration = Duration::from_secs(10 * 60);
/// How long an add-on question for the next episode may wait for an answer in Portal.
const PROMPT_LIMIT: Duration = Duration::from_secs(2 * 60);
const JOB_POLL: Duration = Duration::from_secs(1);

pub(crate) const PROMPT_NOTE: &str =
    "The next episode needs an answer in Portal. Open Portal to start it.";

/// One status line from the helper. Times are the device's, from the start of what it was given.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct DeviceStatus {
    /// PLAYING, PAUSED, BUFFERING, IDLE, or empty when nothing is loaded.
    pub state: String,
    pub idle_reason: Option<String>,
    pub current_time: Option<f64>,
    pub duration: Option<f64>,
    pub content_id: Option<String>,
    /// The TV's own volume, 0 to 1, and mute.
    pub volume: Option<f64>,
    pub muted: Option<bool>,
}

fn finite(value: Option<&serde_json::Value>) -> Option<f64> {
    value
        .and_then(|item| item.as_f64())
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn upper(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(|item| item.as_str())
        .map(|text| text.trim().to_ascii_uppercase())
        .filter(|text| !text.is_empty())
}

/// A helper line as a status, or the helper's error text.
pub(crate) fn status_from_value(value: &serde_json::Value) -> Result<DeviceStatus, String> {
    if value.get("ok").and_then(|item| item.as_bool()) != Some(true) {
        let text = value
            .get("error")
            .and_then(|item| item.as_str())
            .map(super::clean_label)
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| "The Cast device did not answer.".into());
        return Err(text);
    }
    let state = upper(value.get("state"))
        .filter(|state| state != "UNKNOWN")
        .unwrap_or_default();
    Ok(DeviceStatus {
        state,
        idle_reason: upper(value.get("idle_reason")),
        current_time: finite(value.get("current_time")),
        duration: finite(value.get("duration")),
        content_id: value
            .get("content_id")
            .and_then(|item| item.as_str())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string),
        volume: finite(value.get("volume")).map(|level| level.min(1.0)),
        muted: value.get("muted").and_then(|item| item.as_bool()),
    })
}

/// Why a watch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    /// Played to the end.
    Finished,
    /// Stopped on the TV or by another app on the network.
    Stopped,
    /// The TV reported an error.
    Failed,
    /// Something else was cast over it.
    Replaced,
    /// The player closed before the end without saying why.
    Closed,
    /// The device stopped answering.
    Lost,
    /// It never started playing.
    NeverStarted,
}

impl Ending {
    fn state(self) -> DeviceState {
        match self {
            Self::Finished => DeviceState::Ended,
            Self::Failed | Self::Lost | Self::NeverStarted => DeviceState::Failed,
            Self::Stopped | Self::Replaced | Self::Closed => DeviceState::Stopped,
        }
    }

    fn note(self) -> &'static str {
        match self {
            Self::Finished => "Finished.",
            Self::Stopped => "Stopped on the TV.",
            Self::Failed => "The TV could not play the video.",
            Self::Replaced => "Something else is playing on the TV.",
            Self::Closed => "The TV closed the video.",
            Self::Lost => "The TV stopped answering.",
            Self::NeverStarted => "The TV did not start the video.",
        }
    }
}

pub(crate) use portal_api::CastDeviceState as DeviceState;

pub(crate) fn state_from_status(status: &DeviceStatus) -> DeviceState {
    match status.state.as_str() {
        "PLAYING" => DeviceState::Playing,
        "PAUSED" => DeviceState::Paused,
        "BUFFERING" => DeviceState::Buffering,
        "IDLE" => DeviceState::Idle,
        _ => DeviceState::Starting,
    }
}

/// Reads status lines and decides when the cast is over.
#[derive(Debug)]
pub(crate) struct Tracker {
    media: String,
    seen_playing: bool,
    position: f64,
    duration: f64,
    waiting: u32,
    idle: u32,
    failures: u32,
    /// When the TV began buffering after it had played, and where.
    stalled: Option<(Instant, f64)>,
    /// Stalls so far, and their time.
    stalls: u32,
    stalled_for: Duration,
}

impl Tracker {
    pub(crate) fn new(media_url: &str) -> Self {
        Self {
            media: media_url.to_string(),
            seen_playing: false,
            position: 0.0,
            duration: 0.0,
            waiting: 0,
            idle: 0,
            failures: 0,
            stalled: None,
            stalls: 0,
            stalled_for: Duration::ZERO,
        }
    }

    /// The item's length when the TV reports none (a growing playlist plays as live).
    pub(crate) fn with_length(mut self, length: Option<f64>) -> Self {
        if let Some(length) = length.filter(|value| value.is_finite() && *value > 0.0) {
            self.duration = length;
        }
        self
    }

    fn near_end(&self) -> bool {
        self.seen_playing && self.duration > 0.0 && self.duration - self.position <= NEAR_END
    }

    /// Before it has played, a few odd lines are only the TV loading.
    fn still_loading(&mut self) -> Option<Ending> {
        self.waiting += 1;
        (self.waiting >= START_POLLS).then_some(Ending::NeverStarted)
    }

    /// Idle or closed with no reason: finished when near the end, else closed after a few looks.
    fn quiet(&mut self, limit: u32) -> Option<Ending> {
        if !self.seen_playing {
            return self.still_loading();
        }
        if self.near_end() {
            return Some(Ending::Finished);
        }
        self.idle += 1;
        (self.idle >= limit).then_some(Ending::Closed)
    }

    pub(crate) fn observe(&mut self, status: &DeviceStatus) -> Option<Ending> {
        self.failures = 0;
        let ours = status
            .content_id
            .as_deref()
            .is_none_or(|content| same_media_path(content, &self.media));
        if !ours {
            // A status from before the load can still name what played earlier.
            if self.seen_playing {
                return Some(Ending::Replaced);
            }
            return self.still_loading();
        }
        // A cast file's address names its length, so a line with exactly this
        // address is about this load and not the shorter file before it.
        let this_load = self.media.contains("?v=")
            && status
                .content_id
                .as_deref()
                .is_some_and(|content| content.trim() == self.media);
        match status.state.as_str() {
            "PLAYING" | "PAUSED" | "BUFFERING" => {
                self.note_stall(status);
                self.seen_playing = true;
                self.idle = 0;
                if let Some(position) = status.current_time {
                    self.position = position;
                }
                if let Some(duration) = status.duration.filter(|value| *value > 0.0) {
                    self.duration = duration;
                }
                None
            }
            "IDLE" => match status.idle_reason.as_deref() {
                // A short file can play through between two looks.
                Some("FINISHED") if self.seen_playing || this_load => Some(Ending::Finished),
                Some("FINISHED") => self.still_loading(),
                Some("CANCELLED") => Some(Ending::Stopped),
                // Portal seeks a growing stream by loading it again, which
                // briefly reports INTERRUPTED for the same address.
                Some("INTERRUPTED") if self.seen_playing => {
                    self.idle += 1;
                    (self.idle >= IDLE_POLLS).then_some(Ending::Replaced)
                }
                // Loading a longer file of the same title interrupts the short one.
                Some("INTERRUPTED") => self.still_loading(),
                Some("ERROR") => Some(Ending::Failed),
                _ => self.quiet(IDLE_POLLS),
            },
            // Nothing loaded: the receiver app closed, which some TVs do at the end.
            _ => self.quiet(2),
        }
    }

    /// Logs each time the TV stops to buffer once it has played, with
    /// where and for how long, so a slow source or network shows up.
    fn note_stall(&mut self, status: &DeviceStatus) {
        let buffering = status.state == "BUFFERING";
        match (self.stalled, buffering) {
            (None, true) if self.seen_playing => {
                let at = status.current_time.unwrap_or(self.position);
                self.stalled = Some((Instant::now(), at));
            }
            (Some((since, at)), false) => {
                self.stalled = None;
                let lasted = since.elapsed();
                self.stalls += 1;
                self.stalled_for += lasted;
                tracing::info!(
                    position = format!("{at:.0}"),
                    // Status lines come every few seconds, so this is good to about that.
                    seconds = format!("{:.0}", lasted.as_secs_f64()),
                    stalls = self.stalls,
                    total = format!("{:.0}", self.stalled_for.as_secs_f64()),
                    "The TV stopped to buffer"
                );
            }
            _ => {}
        }
    }

    /// A line that could not be read, or the helper stopping.
    pub(crate) fn failed(&mut self) -> Option<Ending> {
        self.failures += 1;
        (self.failures >= LOST_POLLS).then_some(Ending::Lost)
    }
}

/// The device a cast went to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceRef {
    pub id: String,
    pub name: String,
    pub endpoint: Option<DeviceEndpoint>,
}

/// What is being watched: the play job on the device and what comes after it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Watched {
    pub device: DeviceRef,
    pub job_id: String,
    /// The address the device was given.
    pub media_url: String,
    /// Where in the source the device's stream starts.
    pub origin: f64,
    /// The source's length, when known.
    pub job_duration: Option<f64>,
    pub item: Option<CastReplay>,
    pub next: Option<NextItem>,
    pub quality: Quality,
}

impl Watched {
    fn source_position(&self, status: &DeviceStatus) -> Option<f64> {
        status.current_time.map(|at| self.origin + at)
    }

    fn source_duration(&self, status: &DeviceStatus) -> Option<f64> {
        // A cast file's own length is only what was ready when it was made.
        let reported = status.duration.filter(|_| !is_cast_file(&self.media_url));
        self.job_duration
            .filter(|value| *value > 0.0)
            .or_else(|| reported.map(|length| self.origin + length))
    }
}

/// A play job, as the watch needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum JobStep {
    Working,
    /// An add-on is asking something that only a person can answer.
    Prompt,
    Ready(Box<PlayJob>),
    /// The add-on answered with a source list.
    Browse,
    Failed(String),
    Gone,
}

/// Changes to the device cast the dashboard sees. Applied only while `gen` is current.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Update {
    Status {
        state: DeviceState,
        position: Option<f64>,
        duration: Option<f64>,
        volume: Option<f64>,
        muted: Option<bool>,
    },
    /// The next episode's job started; the device still shows the last one.
    Next {
        job_id: String,
        title: String,
        item: Option<CastReplay>,
    },
    /// The next episode is on the device.
    Casting(Box<Watched>),
    Ended {
        state: DeviceState,
        note: String,
    },
}

pub(crate) trait StatusSource: Send {
    /// The next status line; waits for it.
    fn next(&mut self) -> impl Future<Output = Result<DeviceStatus, String>> + Send;
}

/// What the watch does to the rest of Portal. Tests use a fake.
pub(crate) trait Backend: Send + Sync {
    type Source: StatusSource;
    fn watch(&self, device: &DeviceRef) -> Self::Source;
    /// Autoplay next, as the user last chose it.
    fn autoplay(&self) -> bool;
    fn job(&self, id: &str) -> impl Future<Output = JobStep> + Send;
    fn start(&self, request: PlayRequest) -> impl Future<Output = Result<String, String>> + Send;
    fn stop(&self, id: &str) -> impl Future<Output = ()> + Send;
    /// Closes an add-on question nobody answered.
    fn dismiss_prompt(&self) -> impl Future<Output = ()> + Send;
    /// Casts a ready job to the device; answers the address the device was given.
    fn cast(
        &self,
        device: &DeviceRef,
        job: &PlayJob,
        title: &str,
        art: Option<&str>,
    ) -> impl Future<Output = Result<String, String>> + Send;
    /// A cast file ended. Continue the same title when more of it exists.
    fn extend(
        &self,
        _gen: u64,
        _watched: &Watched,
        _played: Option<f64>,
    ) -> impl Future<Output = CastContinue> + Send {
        std::future::ready(CastContinue::Done)
    }
    /// The device is `at` seconds into the file it was given. What follows
    /// that file can be made ready here, before the device runs out.
    fn ahead(&self, _watched: &Watched, _at: f64) {}
    /// The device is near the end of the whole title (source seconds): the
    /// next episode's job can start resolving now.
    fn near_end(&self, _watched: &Watched) {}
    fn progress(&self, report: &ProgressReport);
    fn publish(&self, gen: u64, update: Update);
}

/// A short file ended. The same title may continue, or it really finished.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CastContinue {
    /// The device was given a longer file of the same title.
    Same(Box<Watched>),
    /// The title itself ended.
    Done,
    /// A longer file could not be prepared or started.
    Failed(String),
}

/// How a watch ended, for the log and the tests.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Outcome {
    /// Portal stopped it (Stop, another cast, or shutdown).
    Cancelled,
    /// The device's cast ended; `chained` next episodes played before that.
    Ended { ending: Ending, chained: u32 },
    /// Finished, and there was nothing to play next (or Autoplay next was off).
    Done { chained: u32, autoplay: bool },
    /// Finished, but the next episode could not be cast.
    NextFailed { note: String, chained: u32 },
}

enum ChainError {
    Cancelled,
    Failed(String),
}

fn cancelled(cancel: &watch::Receiver<bool>) -> bool {
    *cancel.borrow()
}

/// Runs `future` unless Portal cancels the watch first.
async fn unless_cancelled<F: Future>(
    cancel: &mut watch::Receiver<bool>,
    future: F,
) -> Option<F::Output> {
    if cancelled(cancel) {
        return None;
    }
    tokio::select! {
        biased;
        _ = cancel.changed() => None,
        output = future => Some(output),
    }
}

/// Watches `watched` on its device and plays what comes next until the cast ends.
pub(crate) async fn drive<B: Backend>(
    backend: &B,
    gen: u64,
    mut watched: Watched,
    mut cancel: watch::Receiver<bool>,
) -> Outcome {
    let mut chained = 0;
    loop {
        let Some((ending, last)) = observe(backend, gen, &watched, &mut cancel).await else {
            return Outcome::Cancelled;
        };
        if ending != Ending::Finished {
            backend.publish(
                gen,
                Update::Ended {
                    state: ending.state(),
                    note: ending.note().into(),
                },
            );
            return Outcome::Ended { ending, chained };
        }
        let played = last.map(|(at, _)| at);
        let continued = unless_cancelled(&mut cancel, backend.extend(gen, &watched, played)).await;
        let Some(continued) = continued else {
            return Outcome::Cancelled;
        };
        match continued {
            CastContinue::Same(next) => {
                watched = *next;
                continue;
            }
            CastContinue::Failed(note) => {
                backend.publish(
                    gen,
                    Update::Ended {
                        state: DeviceState::Failed,
                        note,
                    },
                );
                return Outcome::Ended {
                    ending: Ending::Failed,
                    chained,
                };
            }
            CastContinue::Done => {}
        }
        let length = last.and_then(|(_, length)| length);
        record(backend, &watched, length, length);
        let autoplay = backend.autoplay();
        let next = if autoplay {
            latest_next(backend, &watched).await
        } else {
            None
        };
        let Some(next) = next else {
            let note = if autoplay {
                "Finished."
            } else {
                "Finished. Autoplay next is off."
            };
            backend.publish(
                gen,
                Update::Ended {
                    state: DeviceState::Ended,
                    note: note.into(),
                },
            );
            return Outcome::Done { chained, autoplay };
        };
        match chain(backend, gen, &watched, &next, &mut cancel).await {
            Ok(following) => {
                let finished = std::mem::replace(&mut watched, following);
                backend.stop(&finished.job_id).await;
                chained += 1;
                backend.publish(gen, Update::Casting(Box::new(watched.clone())));
            }
            Err(ChainError::Cancelled) => return Outcome::Cancelled,
            Err(ChainError::Failed(note)) => {
                backend.publish(
                    gen,
                    Update::Ended {
                        state: DeviceState::Failed,
                        note: note.clone(),
                    },
                );
                return Outcome::NextFailed { note, chained };
            }
        }
    }
}

/// The job's own next item if it still has one, else the one known at cast time.
async fn latest_next<B: Backend>(backend: &B, watched: &Watched) -> Option<NextItem> {
    let fresh = match backend.job(&watched.job_id).await {
        JobStep::Ready(job) => job.next,
        _ => None,
    };
    fresh
        .or_else(|| watched.next.clone())
        .filter(|next| !next.file.trim().is_empty())
}

/// Status lines until the cast ends; None when Portal cancels. Also answers
/// the last position seen, and the length when known, in source seconds.
async fn observe<B: Backend>(
    backend: &B,
    gen: u64,
    watched: &Watched,
    cancel: &mut watch::Receiver<bool>,
) -> Option<(Ending, Option<(f64, Option<f64>)>)> {
    let mut tracker = Tracker::new(&watched.media_url)
        .with_length(watched.job_duration.map(|length| length - watched.origin));
    let mut source = backend.watch(&watched.device);
    let mut last: Option<(f64, Option<f64>)> = None;
    let mut reported = Instant::now();
    loop {
        let line = unless_cancelled(cancel, source.next()).await?;
        match line {
            Ok(status) => {
                let ending = tracker.observe(&status);
                let position = watched.source_position(&status);
                let duration = watched.source_duration(&status);
                // An idle line reports time zero, which is not where it stopped.
                let active = matches!(status.state.as_str(), "PLAYING" | "PAUSED" | "BUFFERING");
                if let (true, Some(at)) = (active, position) {
                    last = Some((at, duration));
                }
                if let (true, Some(at)) = (active, status.current_time) {
                    backend.ahead(watched, at);
                }
                if let (true, Some(at), Some(length)) = (active, position, duration) {
                    if length - at <= crate::early::LEAD && length > at {
                        backend.near_end(watched);
                    }
                }
                if ending.is_none() {
                    backend.publish(
                        gen,
                        Update::Status {
                            state: state_from_status(&status),
                            position,
                            duration,
                            volume: status.volume,
                            muted: status.muted,
                        },
                    );
                }
                let playing = matches!(status.state.as_str(), "PLAYING" | "PAUSED");
                if playing && reported.elapsed() >= PROGRESS_EVERY {
                    reported = Instant::now();
                    record(backend, watched, position, duration);
                }
                if let Some(ending) = ending {
                    return Some((ending, last));
                }
            }
            Err(_) => {
                if let Some(ending) = tracker.failed() {
                    return Some((ending, last));
                }
            }
        }
    }
}

/// Saves the place in the library, as the dashboard's player does.
fn record<B: Backend>(
    backend: &B,
    watched: &Watched,
    position: Option<f64>,
    duration: Option<f64>,
) {
    let (Some(position), Some(duration)) = (position, duration) else {
        return;
    };
    let Some(item) = watched.item.as_ref() else {
        return;
    };
    if let Some(report) = progress_report(item, position, duration) {
        backend.progress(&report);
    }
}

/// Artwork JSON must not be reported as a name.
fn human(text: Option<&str>) -> Option<String> {
    let text = text?.trim();
    let lower = text.to_ascii_lowercase();
    if text.is_empty()
        || text.starts_with('{')
        || lower.starts_with("%7b")
        || text.contains("\"poster\"")
    {
        return None;
    }
    Some(text.to_string())
}

pub(crate) fn progress_report(
    item: &CastReplay,
    position: f64,
    duration: f64,
) -> Option<ProgressReport> {
    if !position.is_finite() || !duration.is_finite() || duration <= 0.0 || position < 0.0 {
        return None;
    }
    let (kind, id, name, episode_name) = match item.kind.as_str() {
        "episode" => (
            TitleKind::Tv,
            item.show_id
                .clone()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or_else(|| item.id.clone()),
            human(item.show_title.as_deref())?,
            Some(item.title.clone()),
        ),
        "movie" => (
            TitleKind::Movie,
            item.id.clone(),
            human(Some(&item.title))?,
            None,
        ),
        _ => return None,
    };
    if id.trim().is_empty() {
        return None;
    }
    Some(ProgressReport {
        id,
        kind,
        name,
        art: item.art.clone(),
        season: item.season,
        episode: item.episode,
        episode_name,
        file: Some(item.file.clone()),
        position: position.min(duration),
        duration,
    })
}

/// The play request for the next item, as the dashboard would send it: from the start, same quality.
pub(crate) fn next_request(next: &NextItem, quality: Quality) -> PlayRequest {
    let episode = next.kind == PlayKind::Episode;
    let show_id = next
        .show_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| next.id.clone());
    PlayRequest {
        kind: next.kind,
        id: if episode {
            show_id.clone()
        } else {
            next.id.clone()
        },
        file: next.file.clone(),
        title: next.title.clone(),
        quality,
        season: next.season,
        episode: next.episode,
        art: next.art.clone(),
        show_title: next.show_title.clone(),
        show_id: episode.then_some(show_id),
        resume_seconds: None,
        manual_links: false,
        audio_language: None,
    }
}

pub(crate) fn replay_from_next(next: &NextItem) -> CastReplay {
    let request = next_request(next, Quality::Auto);
    CastReplay {
        kind: if next.kind == PlayKind::Episode {
            "episode"
        } else {
            "movie"
        }
        .into(),
        id: request.id,
        file: request.file,
        title: request.title,
        season: request.season,
        episode: request.episode,
        art: request.art,
        show_title: request.show_title,
        show_id: request.show_id,
    }
}

/// Starts the next item's job, waits for its stream, and casts it to the same device.
async fn chain<B: Backend>(
    backend: &B,
    gen: u64,
    current: &Watched,
    next: &NextItem,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Watched, ChainError> {
    let item = replay_from_next(next);
    let job_id =
        match unless_cancelled(cancel, backend.start(next_request(next, current.quality))).await {
            None => return Err(ChainError::Cancelled),
            Some(Err(detail)) => {
                return Err(ChainError::Failed(format!(
                    "The next episode did not start: {detail}"
                )))
            }
            Some(Ok(id)) => id,
        };
    backend.publish(
        gen,
        Update::Next {
            job_id: job_id.clone(),
            title: next.title.clone(),
            item: Some(item.clone()),
        },
    );
    let begun = Instant::now();
    let mut asked: Option<Instant> = None;
    let job = loop {
        if cancelled(cancel) {
            backend.stop(&job_id).await;
            return Err(ChainError::Cancelled);
        }
        let failure = match backend.job(&job_id).await {
            JobStep::Ready(job) => break *job,
            JobStep::Working => {
                asked = None;
                None
            }
            JobStep::Prompt => {
                let since = *asked.get_or_insert_with(Instant::now);
                if since.elapsed() >= PROMPT_LIMIT {
                    backend.dismiss_prompt().await;
                    Some(PROMPT_NOTE.to_string())
                } else {
                    None
                }
            }
            JobStep::Browse => {
                Some("The next episode needs a source picked in Portal.".to_string())
            }
            JobStep::Failed(detail) => Some(detail),
            JobStep::Gone => {
                return Err(ChainError::Failed("The next episode was stopped.".into()))
            }
        };
        let failure = failure.or_else(|| {
            (begun.elapsed() >= RESOLVE_LIMIT)
                .then(|| "The next episode took too long to start.".to_string())
        });
        if let Some(note) = failure {
            tracing::warn!(
                job = %job_id,
                seconds = begun.elapsed().as_secs(),
                error = %note,
                "The next episode did not start on the TV"
            );
            backend.stop(&job_id).await;
            return Err(ChainError::Failed(note));
        }
        if unless_cancelled(cancel, tokio::time::sleep(JOB_POLL))
            .await
            .is_none()
        {
            backend.stop(&job_id).await;
            return Err(ChainError::Cancelled);
        }
    };
    let title = job
        .title
        .clone()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or_else(|| next.title.clone());
    let cast = unless_cancelled(
        cancel,
        backend.cast(&current.device, &job, &title, item.art.as_deref()),
    )
    .await;
    let media_url = match cast {
        Some(Ok(url)) => url,
        Some(Err(detail)) => {
            backend.stop(&job_id).await;
            return Err(ChainError::Failed(format!(
                "The next episode could not be cast: {detail}"
            )));
        }
        None => {
            backend.stop(&job_id).await;
            return Err(ChainError::Cancelled);
        }
    };
    Ok(Watched {
        device: current.device.clone(),
        job_id,
        origin: job
            .position
            .filter(|at| at.is_finite() && *at >= 0.0 && !super::is_cast_playlist(&media_url))
            .unwrap_or(0.0),
        media_url,
        job_duration: job.duration,
        item: Some(item),
        next: job.next.clone(),
        quality: current.quality,
    })
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/cast/watch.rs"]
mod tests;
