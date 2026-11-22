//! The player dock (player.ts, stage.ts, player-time.ts, index.html `#dock`; plan P7 and P8):
//! one media-playback [`Player`] for the page, built once under every page by the shell, and
//! the dock around it: the side panel (titles, Watchlist, Bookmark, Info, Minimize, Close,
//! the status line, Next episode, Cast, Choose source, Quality, Audio, Autoplay next), the
//! theater layout with its episodes panel, and the mini player.
//!
//! What plays comes from a page through [`crate::widgets::play::play`]: the add-on is asked
//! for a stream (`POST /api/play`, then `GET /api/play/{id}` until it is ready, showing each
//! resolve phase on the picture, add-on questions in a modal, Cancel when the add-on allows),
//! and the HLS stream plays on the work's [`Timeline`]: a seek outside what is converted asks
//! Portal for a new stream ("Skipping to …"), a quality or audio change holds the picture
//! while Portal restarts the stream ([`Player::hold_for_source`]). Progress is saved every
//! 5 s while playing (an 8 s ticker that also keeps the stream alive and looks for a next
//! season near the end), and at once on pause, close and when the page is hidden.
//!
//! Casting (P10) is in `cast.rs`: the Cast button and its menu, and the player as the TV's
//! remote while a TV plays. A seam is left for subtitles (P9, `captions_seam` in `view.rs`: a
//! disabled CC button where the captions control goes; `cast_subtitle` in `cast.rs`).

mod cast;
mod episodes;
mod view;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use portal_api::{
    AudioTrack, JobState, NextItem, PlayJob, PlayKind, PlayRequest, ProgressBody, ProgressReport, Prompt,
    PromptAction, PromptKind, Quality, TitleKind,
};
use remedy_ui::core::time::Instant;
use remedy_ui::playback::{
    KeyScheme, MediaError, MediaSource, NextUp, PlaybackState, Player, PlayerOptions, StageStatus, Timeline,
    UnseenPolicy,
};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::{El, Ui};

use crate::screens::prompts;
use crate::widgets::activity::{describe_play, ActivityView, CancelSpec, Owner, PlayDescription, PlayPhase};
use crate::widgets::play::PlayTarget;
use crate::widgets::titles::{clean_episode_title, episode_code, format_clock};
use crate::{api, prefs, Ctx};

pub use view::rules;

/// How often the ticker saves progress, keeps the stream alive and looks for a next season.
pub const TICK: Duration = Duration::from_secs(8);
/// Progress saved no oftener than this while playing.
pub const PROGRESS_EVERY: f64 = 5000.0;
/// How long the Quality and Audio menus must sit still before the stream restarts.
pub const SETTLE: Duration = Duration::from_millis(600);
/// How long before the end the Up next card shows (player.ts `paintUpNext`).
pub const UP_NEXT_LEAD: Duration = Duration::from_secs(20);
/// Seconds before the end when the server starts resolving the next episode (`early::LEAD`).
pub const EARLY_LEAD: f64 = 60.0;
/// How near the end the job is asked for a next season found after it started.
pub const NEXT_LOOKUP: f64 = 90.0;
/// How long the browser may take to fill the screen before the request counts as refused.
pub const FULLSCREEN_WAIT: Duration = Duration::from_millis(1500);
/// A wait for video this long asks the job whether its stream died.
pub const STALL: Duration = Duration::from_secs(8);

/// The dock, once the shell built it.
#[derive(Clone)]
pub struct Dock(Rc<Inner>);

pub(crate) struct Inner {
    pub(crate) ui: Ui,
    pub(crate) ctx: Ctx,
    pub(crate) player: Player,
    clock: Instant,
    st: RefCell<State>,
    /// The dock is open (`.dock.open`).
    pub(crate) open: Signal<bool>,
    /// What plays (`currentTarget`).
    pub(crate) target: Signal<Option<PlayTarget>>,
    /// The title as the job last named it (setDockTitle's input).
    pub(crate) title: Signal<String>,
    /// The status line (`#dock-status`), the same words the stage says before video.
    pub(crate) status: Signal<String>,
    /// What plays next, when the job knows (`currentNext`).
    pub(crate) next: Signal<Option<NextItem>>,
    /// The job playing (`currentJob`).
    pub(crate) job: Signal<Option<String>>,
    /// The file's audio tracks and the one playing.
    pub(crate) audio: Signal<(Vec<AudioTrack>, Option<u32>)>,
    /// Theater was asked for, this dock session (`theaterWanted`).
    pub(crate) theater: Signal<bool>,
    pub(crate) episodes: episodes::Episodes,
    pub(crate) cast: cast::Cast,
}

#[derive(Default)]
struct State {
    generation: u64,
    skip_generation: u64,
    job: Option<String>,
    last_ready: Option<PlayJob>,
    origin: f64,
    job_duration: f64,
    /// A stream of the current job is in the player.
    attached: bool,
    /// The current item found no stream, or Portal did not answer: Play asks again.
    start_failed: bool,
    /// Where the stream was when it died, so a retry or a save carries on from there.
    left_at: f64,
    last_post: Option<f64>,
    play_ticket: u64,
    stage_cancel: Option<Rc<dyn Fn()>>,
    saw_progress: bool,
    canceling: bool,
    cancel_prompt: String,
    next_asked: String,
    early_at: Option<f64>,
    /// What had focus when playing started, for when the dock closes.
    opener: Option<El>,
    /// This start makes the player's next ([`Player::on_next_needed`]).
    as_next: bool,
    ticker: Option<u64>,
    quality_timer: Option<u64>,
    audio_timer: Option<u64>,
    audio_pending: bool,
    stall_timer: Option<u64>,
    last_state: Option<PlaybackState>,
}

thread_local! {
    static DOCK: RefCell<Option<Dock>> = const { RefCell::new(None) };
}

/// The dock, once built.
pub fn dock() -> Option<Dock> {
    DOCK.with(|d| d.borrow().clone())
}

/// The dock under every page; built once.
pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let d = Dock::new(ui, ctx);
    DOCK.with(|s| *s.borrow_mut() = Some(d.clone()));
    let el = view::dock(&d);
    cast::resume(&d);
    el
}

/// A page's Play (player.ts `playFromLibrary`): opens the full player, out of the mini one.
pub fn play_from_library(target: PlayTarget) {
    if let Some(d) = dock() {
        d.remember_opener();
        d.0.player.set_compact(false);
        d.play(target);
    }
}

/// Portal's words for the player's own controls where the engine words them differently
/// (index.html's names and titles).
const WORDS: &str = "playback-seek = Time
playback-full-screen = Fullscreen
playback-exit-full-screen = Exit fullscreen
playback-full-screen-tip = Fullscreen (f)
playback-back-tip = Back 10 seconds (j)
playback-forward-tip = Forward 10 seconds (l)
playback-mute-tip = Mute (m)
playback-theater-tip = Theater mode (t)
playback-skipping = Skipping to { $position }.
playback-cast-connecting = Playing on { $device }
playback-cast-playing = Playing on { $device }
playback-cast-paused = Paused on { $device }
playback-cast-buffering = Playing on { $device }
playback-cast-ended = Playing on { $device }
playback-cast-failed = Playing on { $device }
playback-cast-next = Playing on { $device }
";

fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

fn dur(s: f64) -> Duration {
    Duration::try_from_secs_f64(s.max(0.0)).unwrap_or_default()
}

/// player.ts `appliedSeek`: a resume point past the first minute, whole seconds.
fn applied_seek(resume: Option<f64>) -> f64 {
    match resume {
        Some(r) if r.is_finite() && r > 60.0 => r.floor(),
        _ => 0.0,
    }
}

/// player.ts `humanTitle`: a name, never artwork JSON an add-on put there.
pub fn human_title(name: Option<&str>) -> String {
    let text = name.unwrap_or("").trim();
    let lower = text.to_lowercase();
    if text.is_empty()
        || text.starts_with('{')
        || lower.starts_with("%7b")
        || text.contains("\"poster\"")
        || lower.contains("%22poster%22")
        || (lower.contains("%7b") && lower.contains("poster"))
    {
        return String::new();
    }
    text.to_owned()
}

/// titles.ts `displayMediaTitle`.
pub fn display_media_title(raw: &str, season: Option<u32>, episode: Option<u32>, show: Option<&str>) -> String {
    let cleaned = clean_episode_title(raw, season, episode, show);
    let code = episode_code(season, episode);
    match (code.is_empty(), cleaned.is_empty()) {
        (false, false) => format!("{code} · {cleaned}"),
        (false, true) => code,
        _ if !cleaned.is_empty() => cleaned,
        _ => raw.trim().to_owned(),
    }
}

/// player.ts `nextDetail`: `S02E01 · Title`.
pub fn next_detail(n: &NextItem) -> String {
    let code = episode_code(n.season, n.episode);
    let title = clean_episode_title(&n.title, n.season, n.episode, n.show_title.as_deref());
    [code, title].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// player.ts `nextIsEpisode`.
pub fn next_is_episode(n: &NextItem) -> bool {
    n.kind == PlayKind::Episode || (n.season.is_some() && n.episode.is_some_and(|e| e > 0))
}

/// What the Up next card calls `n`.
fn next_card_title(n: &NextItem) -> String {
    let d = next_detail(n);
    if !d.is_empty() {
        return d;
    }
    let h = human_title(Some(&n.title));
    if h.is_empty() { "Next episode".into() } else { h }
}

/// player.ts `targetFromNext`.
pub fn target_from_next(n: &NextItem) -> PlayTarget {
    let episode = n.kind == PlayKind::Episode;
    let show_id = n.show_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| n.id.clone());
    let show = human_title(n.show_title.as_deref());
    PlayTarget {
        kind: n.kind,
        id: if episode { show_id.clone() } else { n.id.clone() },
        file: n.file.clone(),
        title: n.title.clone(),
        season: n.season,
        episode: n.episode,
        art: n.art.clone(),
        show_title: (!show.is_empty()).then(|| show.clone()),
        show_id: episode.then(|| show_id.clone()),
        info: None,
        resume_seconds: None,
        manual_links: false,
        progress_kind: if episode { TitleKind::Tv } else { TitleKind::Movie },
        progress_id: if episode { show_id } else { n.id.clone() },
        progress_name: if episode { show } else { human_title(Some(&n.title)) },
        track_progress: matches!(n.kind, PlayKind::Movie | PlayKind::Episode),
    }
}

/// player.ts `playBody`.
pub fn play_body(t: &PlayTarget, p: &prefs::Prefs) -> PlayRequest {
    PlayRequest {
        kind: t.kind,
        id: t.id.clone(),
        file: t.file.clone(),
        title: t.title.clone(),
        quality: p.quality,
        season: t.season,
        episode: t.episode.filter(|e| *e > 0),
        art: t.art.clone().filter(|a| !a.is_empty()),
        show_title: t.show_title.clone().filter(|s| !s.is_empty()),
        show_id: t.show_id.clone().filter(|s| !s.is_empty()),
        resume_seconds: t.resume_seconds.filter(|r| *r > 60.0),
        manual_links: t.manual_links,
        // subtitles.ts `startupAudioLanguage`: the language last picked.
        audio_language: p.audio_language.clone(),
    }
}

/// The name a progress save reports (player.ts `progressReportName`).
fn progress_name(t: &PlayTarget) -> String {
    let named = human_title(Some(&t.progress_name));
    if !named.is_empty() {
        return named;
    }
    let show = human_title(t.show_title.as_deref());
    if t.progress_kind == TitleKind::Movie {
        let h = human_title(Some(&t.title));
        return if h.is_empty() { show } else { h };
    }
    show
}

/// The tallest picture a quality asks for (player.ts `qualityHeight`): Auto is 1080p.
fn quality_height(q: Quality) -> f64 {
    q.cap_height().map_or(f64::INFINITY, |h| h as f64)
}

/// Why a stream stopped, in a sentence (player.ts `streamFailure`).
fn stream_failure(e: Option<&MediaError>) -> &'static str {
    match e {
        Some(MediaError::NotFound(_)) => "The stream stopped loading.",
        Some(MediaError::Unsupported(r) | MediaError::Format(r) | MediaError::Decode(r))
            if r.to_lowercase().contains("load") || r.to_lowercase().contains("timeout") =>
        {
            "The stream stopped loading."
        }
        _ => "This video could not be played here.",
    }
}

/// `text`, saying Play tries again (player.ts `failStart`).
fn with_retry(text: &str) -> String {
    if text.to_lowercase().contains("try again") {
        return text.to_owned();
    }
    format!("{} Press Play to try again.", text.trim_end())
}

impl Dock {
    fn new(ui: &Ui, ctx: &Ctx) -> Dock {
        let _ = remedy_ui::view::strings::add_messages("media-playback", "en", WORDS);
        // The skip glyphs' "10" is SVG text in the page's UI face (`.icon-btn svg text`).
        for f in remedy_ui_fonts::all().into_iter().filter(|f| f.family == "Remedy Sans UI") {
            remedy_ui::svg::add_font(f.data.to_vec());
        }
        let p = prefs::now();
        let options = PlayerOptions::default()
            .autoplay(true)
            .two_rows(true)
            .skip_buttons(true)
            .speed_button(false)
            .volume_on_hover(true)
            .keys(KeyScheme::Arrows)
            .find_subtitles(false)
            // A page's video plays on in a hidden tab.
            .unseen(UnseenPolicy::KeepPlaying)
            .volume(p.volume)
            .next_up_lead(Some(UP_NEXT_LEAD))
            .next_up_delay(p.autoplay.then_some(Duration::ZERO))
            .compact_pauses(true)
            .keep_awake(true)
            .web_backend(remedy_ui::playback::WebBackend::MediaSource);
        let player = Player::empty_platform(ui, options);
        let d = Dock(Rc::new(Inner {
            ui: ui.clone(),
            ctx: ctx.clone(),
            player,
            clock: Instant::now(),
            st: RefCell::default(),
            open: Signal::new(false),
            target: Signal::new(None),
            title: Signal::new(String::new()),
            status: Signal::new(String::new()),
            next: Signal::new(None),
            job: Signal::new(None),
            audio: Signal::new((Vec::new(), None)),
            theater: Signal::new(false),
            episodes: episodes::Episodes::new(),
            cast: cast::Cast::new(),
        }));
        d.wire();
        d
    }

    pub(crate) fn inner(&self) -> &Inner {
        &self.0
    }

    fn now_ms(&self) -> f64 {
        self.0.clock.elapsed().as_secs_f64() * 1000.0
    }

    fn generation(&self) -> u64 {
        self.0.st.borrow().generation
    }

    fn weak(&self) -> std::rc::Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    /// Runs `f` with the dock after `delay`, if it is still there.
    fn after(&self, delay: Duration, f: impl FnOnce(&Dock) + 'static) -> u64 {
        let w = self.weak();
        self.0.ui.after(delay, move || {
            if let Some(i) = w.upgrade() {
                f(&Dock(i));
            }
        })
    }

    /// Sends `call`, then `done` with the dock and the answer.
    fn run<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        call: api::Call<T>,
        done: impl FnOnce(&Dock, Result<T, api::ApiError>) + 'static,
    ) {
        let w = self.weak();
        self.0.ctx.api.run(call, &Cancel::new(), move |out| {
            if let Some(i) = w.upgrade() {
                done(&Dock(i), out);
            }
        });
    }

    /// The player's own events: retries, seeks outside the stream, Cancel, the next, the
    /// layout requests, and the state changes that save progress.
    fn wire(&self) {
        let p = &self.0.player;
        p.keys_from(&self.0.ui.root());
        let w = self.weak();
        let with = move |f: &dyn Fn(&Dock)| {
            if let Some(i) = w.upgrade() {
                f(&Dock(i));
            }
        };
        let w2 = with.clone();
        p.on_retry(move |at| w2(&|d| d.retry(secs(at))));
        let w2 = with.clone();
        p.on_seek_outside(move |to| w2(&|d| d.skip_to(secs(to))));
        let w2 = with.clone();
        p.on_cancel(move || {
            w2(&|d| {
                let cancel = d.0.st.borrow().stage_cancel.clone();
                match cancel {
                    Some(c) => c(),
                    // Nothing of the add-on's to stop: what the player waits for ends here.
                    None if d.0.player.starting_next().is_some() => d.0.player.cancel_next(),
                    None => d.0.player.cancel_skip(),
                }
            })
        });
        let w2 = with.clone();
        p.on_next_needed(move |_| {
            w2(&|d| {
                let next = d.0.next.get_untracked().ok().flatten();
                match next {
                    Some(n) => {
                        d.0.st.borrow_mut().as_next = true;
                        d.play(target_from_next(&n));
                    }
                    None => d.0.player.cancel_next(),
                }
            })
        });
        let w2 = with.clone();
        p.on_theater(move |on| w2(&|d| d.set_theater(on)));
        let win = self.0.ctx.window.clone();
        let (w2, pf) = (with.clone(), p.clone());
        p.on_fullscreen(move |on| {
            win.set_fullscreen(on);
            if !on {
                return;
            }
            // A browser that refuses (no press behind the request) leaves the page as it was.
            let (win2, pf2) = (win.clone(), pf.clone());
            w2(&move |d| {
                let (win3, pf3) = (win2.clone(), pf2.clone());
                d.after(FULLSCREEN_WAIT, move |_| {
                    if !untrack(|| win3.is_fullscreen()) && untrack(|| pf3.is_fullscreen()) {
                        pf3.request_fullscreen(false);
                    }
                });
            });
        });
        // The browser's own Escape leaves full screen without the player asking.
        let (w2, win) = (with.clone(), self.0.ctx.window.clone());
        let root = self.0.ui.root();
        let was = Cell::new(false);
        let _ = root.bind(move || {
            let full = win.is_fullscreen();
            if was.replace(full) && !full {
                w2(&|d| {
                    if untrack(|| d.0.player.is_fullscreen()) {
                        d.0.player.request_fullscreen(false);
                    }
                });
            }
        });
        // Volume per device; autoplay follows the shared choice. A TV's volume is its own,
        // never this browser's.
        let (w2, pl) = (with.clone(), p.clone());
        let _ = self.0.ui.root().bind(move || {
            let (v, muted) = (pl.volume(), pl.is_muted());
            let _ = muted;
            if pl.is_remote() {
                return;
            }
            w2(&|d| {
                if (prefs::now().volume - v).abs() > 1e-4 {
                    prefs::set(&d.0.ctx, prefs::Patch { volume: Some(v), ..Default::default() });
                }
            });
        });
        let (w2, ps) = (with.clone(), prefs::signal());
        let _ = self.0.ui.root().bind(move || {
            let auto = ps.with(|p| p.autoplay);
            w2(&|d| untrack(|| d.0.player.set_next_up_delay(auto.then_some(Duration::ZERO))));
        });
        let (w2, pl) = (with.clone(), p.clone());
        let _ = self.0.ui.root().bind(move || {
            let s = pl.state();
            w2(&|d| untrack(|| d.state_changed(s)));
        });
        let (w2, pl) = (with.clone(), p.clone());
        let _ = self.0.ui.root().bind(move || {
            let (at, len) = (pl.position(), pl.duration());
            let _ = len;
            w2(&|d| untrack(|| d.near_end(secs(at))));
        });
        // Choices made on another screen or device show here (prefs.ts on mount and when
        // the page is seen again).
        prefs::sync(&self.0.ctx, || {});
        // Seen again: choices made elsewhere meanwhile.
        let w2 = with.clone();
        let _ = self.0.ui.root().on_visibility(move |seen| {
            if seen {
                w2(&|d| prefs::sync(&d.0.ctx, || {}));
            }
        });
        // The page hidden or closed: save the place now, with keepalive so the post outlives
        // the tab (player.ts on visibilitychange and pagehide; the ticker may not run again).
        // ENGINE GAP: `on_leaving` does not say whether the page is closing or only hidden,
        // so the job is not stopped here as pagehide does (that would end it on a tab switch).
        let w2 = with.clone();
        self.0.ctx.app.on_leaving(move || w2(&|d| d.post_progress(true, true, None)));
    }

    fn remember_opener(&self) {
        let ui = &self.0.ui;
        let focused = ui.tree().focused().map(|f| ui.element(f));
        let inside = |el: &El| {
            let mut at = Some(el.clone());
            while let Some(e) = at {
                if e.info().is_some_and(|i| i.widget == "p_dock") {
                    return true;
                }
                at = e.parent();
            }
            false
        };
        let mut st = self.0.st.borrow_mut();
        match focused {
            Some(f) if !inside(&f) => st.opener = Some(f),
            // A fresh start with nothing focused must not hand focus to a control from an
            // earlier play.
            _ if !self.0.open.get_untracked().unwrap_or(false) => st.opener = None,
            _ => {}
        }
    }

    /// player.ts `play`: starts `target` in this dock.
    pub fn play(&self, target: PlayTarget) {
        let token = {
            let mut st = self.0.st.borrow_mut();
            st.generation += 1;
            st.generation
        };
        self.start(target, token);
    }

    // ---- Status ----

    /// player.ts `setStatus`: the status line, and the stage's words before video.
    pub(crate) fn set_status(&self, text: &str) {
        self.0.status.set_if_changed(text.to_owned());
        self.paint_stage();
    }

    /// The words over the picture: busy while a play activity runs, a message before the
    /// video started (player.ts `paintStage`).
    fn paint_stage(&self) {
        let (ticket, attached, as_next) = {
            let st = self.0.st.borrow();
            (st.play_ticket, st.attached, st.as_next)
        };
        let text = self.0.status.get_untracked().unwrap_or_default();
        let p = &self.0.player;
        if as_next && untrack(|| p.starting_next().is_some()) {
            p.next_status((!text.is_empty()).then_some(text));
            return;
        }
        let failed = untrack(|| p.state() == PlaybackState::Failed);
        let status = if ticket != 0 && self.0.target.get_untracked().ok().flatten().is_some() {
            Some(StageStatus::new(text).busy(true))
        } else if !attached && !text.is_empty() && !failed && !self.0.cast.active() {
            Some(StageStatus::new(text))
        } else {
            None
        };
        p.set_status(status);
    }

    /// player.ts `showPlay`: the activity panel and the stage say where the play is.
    fn show_play(&self, input: PlayDescription<'_>, cancel: Option<Rc<dyn Fn()>>, cancel_disabled: bool) {
        let Some(target) = self.0.target.get_untracked().ok().flatten() else {
            self.clear_play_activity();
            return;
        };
        let mut input = input;
        input.links = input.links || target.manual_links;
        let described = describe_play(&input);
        let view = ActivityView::of(&described);
        let spec = CancelSpec { on_cancel: cancel.clone(), disabled: cancel_disabled };
        let ticket = self.0.ctx.activity.begin_with(Owner::Play, described, 0, spec);
        {
            let mut st = self.0.st.borrow_mut();
            st.play_ticket = ticket;
            st.stage_cancel = cancel.clone();
        }
        self.0.player.offer_cancel(cancel.is_some() && !cancel_disabled);
        self.set_status(&view.status);
    }

    fn clear_play_activity(&self) {
        let ticket = std::mem::take(&mut self.0.st.borrow_mut().play_ticket);
        if ticket == 0 {
            return;
        }
        self.0.ctx.activity.end(ticket);
        self.0.st.borrow_mut().stage_cancel = None;
        self.0.player.offer_cancel(false);
        self.paint_stage();
        self.episodes_after_resolve();
    }

    /// player.ts `failStart`: says why, and Play asks again.
    fn fail_start(&self, text: &str) {
        self.0.st.borrow_mut().start_failed = true;
        self.0.st.borrow_mut().as_next = false;
        // A start meant for a TV says where Play tries again (player.ts `failStart`).
        let shown = match self.0.cast.pending() {
            Some(dev) if !text.to_lowercase().contains("try again") => {
                format!("{} Press Play to try again on {}.", text.trim_end(), dev.name)
            }
            _ => with_retry(text),
        };
        self.0.status.set_if_changed(shown);
        let p = &self.0.player;
        p.cancel_next();
        let base = text.trim_end().trim_end_matches(" Press Play to try again.").to_owned();
        p.fail(base);
    }

    // ---- Start ----

    /// player.ts `start`.
    fn start(&self, target: PlayTarget, token: u64) {
        let report = self.0.target.get_untracked().ok().flatten().is_some_and(|t| t.track_progress);
        // Files.GetDirectory during a resolve steals Kodi from the source search: the list
        // waits until this item is on screen.
        self.0.episodes.hold(true);
        let as_next = self.0.st.borrow().as_next;
        // Next, a quality change or another link while casting goes to the same TV, and so
        // does Play after one of those could not start.
        let recast = self.0.cast.current().or_else(|| self.0.cast.pending());
        self.release_current(report, as_next);
        self.0.cast.set_pending(recast);
        {
            let mut st = self.0.st.borrow_mut();
            st.start_failed = false;
            st.origin = applied_seek(target.resume_seconds);
            st.saw_progress = false;
            st.canceling = false;
            st.cancel_prompt.clear();
            st.as_next = as_next;
        }
        let links = target.manual_links;
        let title = target.title.clone();
        self.0.target.set(Some(target.clone()));
        self.0.title.set(title.clone());
        self.0.open.set_if_changed(true);
        self.0.player.set_title(Some(view::stage_title(&target, &title)));
        let mut d = PlayDescription::new(&title, PlayPhase::Opening);
        d.links = links;
        self.show_play(d, None, false);
        self.0.episodes.hold(false);
        self.request_episodes();
        self.0.ctx.activity.notice("");
        self.run(api::status(), move |me, out| {
            if me.generation() != token {
                return;
            }
            if out.map_or(true, |s| s.state != portal_api::EngineState::Running) {
                let t = me.0.title.get_untracked().unwrap_or_default();
                let mut d = PlayDescription::new(&t, PlayPhase::Starting);
                d.links = links;
                me.show_play(d, None, false);
            }
            // Quality and subtitles as last chosen on any screen, not only this one.
            let w = me.weak();
            prefs::sync(&me.0.ctx, move || {
                let Some(i) = w.upgrade() else { return };
                let me = Dock(i);
                if me.generation() != token {
                    return;
                }
                let body = play_body(&target, &prefs::now());
                me.run(api::play(&body), move |me, out| {
                    let accepted = match out {
                        Ok(a) => a,
                        Err(e) => {
                            if me.generation() == token {
                                me.clear_play_activity();
                                me.fail_start(&e.detail);
                            }
                            return;
                        }
                    };
                    if me.generation() != token {
                        me.run(api::stop(&accepted.id), |_, _| {});
                        return;
                    }
                    me.0.st.borrow_mut().job = Some(accepted.id.clone());
                    me.0.job.set(Some(accepted.id.clone()));
                    me.poll(token, accepted.id, 0);
                });
            });
        });
    }

    /// player.ts `releaseCurrent`: saves the place, stops the job, and clears what the
    /// last item left. The player itself stops fetching (unless it is making its next).
    fn release_current(&self, report: bool, as_next: bool) {
        self.0.episodes.release();
        let id = self.0.st.borrow().job.clone();
        let then = id.clone().map(|id| -> Box<dyn FnOnce(&Dock)> { Box::new(move |me: &Dock| me.run(api::stop(&id), |_, _| {})) });
        // Built now, sent before the job stops; a play that starts meanwhile is never reset by it.
        if report {
            self.post_progress(true, false, then);
        } else if let Some(f) = then {
            f(self);
        }
        // The TV stops with the item.
        let casting = cast::release(self);
        let held = {
            let mut st = self.0.st.borrow_mut();
            st.skip_generation += 1;
            st.job = None;
            st.left_at = 0.0;
            st.saw_progress = false;
            st.canceling = false;
            st.cancel_prompt.clear();
            st.last_ready = None;
            st.origin = 0.0;
            st.job_duration = 0.0;
            st.next_asked.clear();
            st.early_at = None;
            st.audio_pending = false;
            for t in [st.audio_timer.take(), st.stall_timer.take()].into_iter().flatten() {
                self.0.ui.cancel_timer(t);
            }
            std::mem::take(&mut st.attached) || casting
        };
        self.clear_play_activity();
        self.stop_ticker();
        self.0.job.set_if_changed(None);
        self.0.audio.set((Vec::new(), None));
        self.0.target.set_if_changed(None);
        prompts::hide();
        if !as_next {
            self.0.next.set_if_changed(None);
            self.0.player.set_next_up(None);
            if held {
                // Nothing more of the old stream is fetched while the next one is made.
                let at = untrack(|| self.0.player.position());
                self.0.player.hold_for_source(at);
            }
        }
    }

    /// player.ts `poll`: follows the job until it plays, fails, or hands back a folder.
    fn poll(&self, token: u64, id: String, misses: u32) {
        let id2 = id.clone();
        self.run(api::job(&id), move |me, out| {
            if me.generation() != token {
                return;
            }
            let job = match out {
                Ok(j) => j,
                // Portal can be slow while Kodi is busy; one miss is not the end of the job.
                Err(_) if misses + 1 < 4 => {
                    me.after(Duration::from_secs(1), move |me| me.poll(token, id2, misses + 1));
                    return;
                }
                Err(e) => {
                    prompts::hide();
                    me.clear_play_activity();
                    me.fail_start(&e.detail);
                    return;
                }
            };
            match job.state {
                JobState::Working | JobState::Prompt => {
                    let again = move |me: &Dock| me.after(Duration::from_millis(250), move |me| me.poll(token, id2, 0));
                    if job.prompt_known {
                        let prompt = job.prompt.clone();
                        me.paint_play_step(token, &job, prompt.as_ref());
                        again(me);
                    } else {
                        me.run(api::prompt(), move |me, read| {
                            if me.generation() != token {
                                return;
                            }
                            // A prompt failure is not the end of the job: the next poll tries again.
                            if let Ok(read) = read {
                                me.paint_play_step(token, &job, read.prompt.as_ref());
                            }
                            again(me);
                        });
                    }
                }
                JobState::Browse if job.browse_path.is_some() => {
                    prompts::hide();
                    let path = job.browse_path.clone().unwrap_or_default();
                    // A manual link choice the add-on answered with a folder: its sources.
                    let picking = me.0.target.get_untracked().ok().flatten().filter(|t| t.manual_links);
                    me.clear_play_activity();
                    me.set_status("");
                    me.0.st.borrow_mut().generation += 1;
                    me.0.episodes.release();
                    me.release_current(false, false);
                    me.hide_dock();
                    crate::widgets::play::set_source_pick(picking.map(|t| (path.clone(), t)));
                    me.0.ctx.go(&crate::pages::Page::Browse(path));
                }
                JobState::Failed => {
                    prompts::hide();
                    me.clear_play_activity();
                    me.fail_start(job.error.as_deref().unwrap_or("Playback failed."));
                }
                JobState::Ready if job.playlist.is_some() => {
                    prompts::hide();
                    me.set_next(job.next.clone());
                    if let Some(t) = job.title.clone() {
                        me.0.title.set_if_changed(t);
                    }
                    me.attach(&job);
                }
                _ => {
                    prompts::hide();
                    me.after(Duration::from_millis(500), move |me| me.poll(token, id2, 0));
                }
            }
        });
    }

    /// player.ts `paintPlayStep`: a progress dialog on the stage, a question in the modal.
    fn paint_play_step(&self, token: u64, job: &PlayJob, prompt: Option<&Prompt>) {
        let Some(target) = self.0.target.get_untracked().ok().flatten() else {
            self.clear_play_activity();
            return;
        };
        let item = job.title.clone().filter(|t| !t.is_empty()).unwrap_or(target.title.clone());
        if let Some(p) = prompt.filter(|p| p.kind == PromptKind::Progress) {
            prompts::hide();
            let canceling = {
                let mut st = self.0.st.borrow_mut();
                st.saw_progress = true;
                if p.id != st.cancel_prompt {
                    st.canceling = false;
                    st.cancel_prompt.clear();
                }
                st.canceling
            };
            let mut d = PlayDescription::new(&item, PlayPhase::Working);
            d.heading = Some(&p.title);
            d.message = Some(&p.message);
            d.progress = p.progress;
            d.canceling = canceling;
            let cancel: Option<Rc<dyn Fn()>> = (p.cancellable && !canceling).then(|| {
                let (w, pid, item, heading, message, progress) =
                    (self.weak(), p.id.clone(), item.clone(), p.title.clone(), p.message.clone(), p.progress);
                Rc::new(move || {
                    let Some(i) = w.upgrade() else { return };
                    let me = Dock(i);
                    if me.generation() != token {
                        return;
                    }
                    {
                        let mut st = me.0.st.borrow_mut();
                        st.canceling = true;
                        st.cancel_prompt = pid.clone();
                    }
                    let mut d = PlayDescription::new(&item, PlayPhase::Working);
                    d.heading = Some(&heading);
                    d.message = Some(&message);
                    d.progress = progress;
                    d.canceling = true;
                    me.show_play(d, None, true);
                    let body = portal_api::PromptAnswer { id: pid.clone(), action: PromptAction::Cancel, text: None, index: None };
                    me.run(api::answer_prompt(&body), move |me, out| {
                        if let (Err(e), true) = (out, me.generation() == token) {
                            me.set_status(&e.detail);
                        }
                    });
                }) as Rc<dyn Fn()>
            });
            self.show_play(d, cancel, canceling);
            return;
        }
        if let Some(p) = prompt.filter(|p| prompts::has_words(p)) {
            self.render_prompt(p);
            let mut d = PlayDescription::new(&item, PlayPhase::Answering);
            d.heading = Some(&p.title);
            self.show_play(d, None, false);
            return;
        }
        prompts::hide();
        let phase = if self.0.st.borrow().saw_progress { PlayPhase::Preparing } else { PlayPhase::Waiting };
        self.show_play(PlayDescription::new(&item, phase), None, false);
    }

    /// The add-on's question, answered through the modal; a cancelled link choice goes back
    /// to the item as it was, from where it was.
    fn render_prompt(&self, p: &Prompt) {
        // In full screen it would be answered unseen.
        if self.0.player.is_fullscreen() {
            self.0.player.request_fullscreen(false);
        }
        let (w1, w2) = (self.weak(), self.weak());
        let back = self.0.target.get_untracked().ok().flatten().filter(|t| t.manual_links);
        let hooks = prompts::Hooks {
            on_error: Rc::new(move |text| {
                if let Some(i) = w1.upgrade() {
                    Dock(i).set_status(&text);
                }
            }),
            after: Rc::new(move |p, action| {
                let Some(i) = w2.upgrade() else { return };
                let me = Dock(i);
                let still = me.0.target.get_untracked().ok().flatten();
                if let (PromptAction::Cancel, PromptKind::Select, Some(b)) = (action, p.kind, back.clone()) {
                    if still.as_ref() == Some(&b) {
                        me.play(PlayTarget { manual_links: false, ..b });
                    }
                }
            }),
        };
        prompts::show(&self.0.ui, &self.0.ctx, p, hooks);
    }

    /// player.ts `attachPlayback`: the job's stream into the player, on the work's time.
    fn attach(&self, job: &PlayJob) {
        let Some(playlist) = job.playlist.clone() else {
            self.clear_play_activity();
            self.set_status("The stream did not arrive.");
            return;
        };
        // Made for a TV: it goes there, not into this page's player.
        if let Some(dev) = self.0.cast.take_pending() {
            {
                let mut st = self.0.st.borrow_mut();
                st.last_ready = Some(job.clone());
                st.origin = job.position.filter(|p| p.is_finite() && *p >= 0.0).unwrap_or(st.origin);
                st.job_duration = job.duration.filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(0.0);
                st.as_next = false;
            }
            self.paint_audio(Some(job));
            self.clear_play_activity();
            cast::cast_to(self, dev);
            return;
        }
        let resume = self.0.target.get_untracked().ok().flatten().and_then(|t| t.resume_seconds);
        let origin = job.position.filter(|p| p.is_finite() && *p >= 0.0).unwrap_or_else(|| applied_seek(resume));
        let duration = job.duration.filter(|d| d.is_finite() && *d >= 0.0).unwrap_or(0.0);
        let as_next = {
            let mut st = self.0.st.borrow_mut();
            st.origin = origin;
            st.job_duration = duration;
            st.attached = true;
            st.start_failed = false;
            st.left_at = 0.0;
            st.last_ready = Some(job.clone());
            std::mem::take(&mut st.as_next)
        };
        self.paint_audio(Some(job));
        // P9: the job's subtitles (`job.subtitles`, `subtitles_pending`) and the online search
        // start here, as player.ts `fillSubtitles`, `awaitSubtitles` and `searchSubtitles`.
        let mut timeline = Timeline::new(dur(origin));
        if duration > 0.0 {
            timeline = timeline.length(dur(duration));
        }
        let source = MediaSource::url(playlist);
        if as_next && self.0.player.starting_next().is_some() {
            self.0.player.start_next(source, Some(timeline));
        } else {
            self.0.player.replace_source(source, timeline);
        }
        self.clear_play_activity();
        self.0.ctx.activity.notice("");
        self.set_status("");
        self.start_ticker();
    }

    // ---- Seeking, quality, audio, retry ----

    /// player.ts `skipTo`, for a place outside what the stream holds: Portal makes a new
    /// stream from there. Presses of ±10 meanwhile add up in the player.
    fn skip_to(&self, at: f64) {
        let Some(id) = self.0.st.borrow().job.clone() else {
            self.0.player.cancel_skip();
            return;
        };
        let length = self.progress_duration();
        let at = if length > 0.0 { at.floor().min(length.floor()) } else { at.floor() }.max(0.0);
        let (token, mine) = {
            let mut st = self.0.st.borrow_mut();
            st.skip_generation += 1;
            (st.generation, st.skip_generation)
        };
        let title = self.0.target.get_untracked().ok().flatten().map(|t| t.title).unwrap_or_default();
        let clock = format_clock(at);
        let mut d = PlayDescription::new(if title.is_empty() { "Playback" } else { &title }, PlayPhase::Skipping);
        d.at = Some(&clock);
        self.show_play(d, None, false);
        let id2 = id.clone();
        self.run(api::seek(&id, at), move |me, out| {
            let fresh = |me: &Dock| {
                let st = me.0.st.borrow();
                st.generation == token && st.skip_generation == mine
            };
            if !fresh(me) {
                return;
            }
            match out {
                Ok(job) if job.playlist.is_some() => {
                    me.set_next(job.next.clone());
                    if let Some(t) = job.title.clone() {
                        me.0.title.set_if_changed(t);
                    }
                    me.attach(&job);
                }
                Ok(_) => {
                    me.clear_play_activity();
                    me.0.player.cancel_skip();
                    me.set_status("The stream did not arrive.");
                }
                Err(e) => me.recover_skip(&id2, at, e.detail, token, mine),
            }
        });
    }

    /// player.ts `recoverSkip`: the server may have stopped the old stream before the seek
    /// failed. Carry on with the job if it still plays; otherwise ask for the item again from
    /// the place that was asked for.
    fn recover_skip(&self, id: &str, at: f64, failure: String, token: u64, mine: u64) {
        self.run(api::job(id), move |me, live| {
            {
                let st = me.0.st.borrow();
                if st.generation != token || st.skip_generation != mine {
                    return;
                }
            }
            if let Some(live) = live.ok().filter(|j| j.state == JobState::Ready && j.playlist.is_some()) {
                // The same stream means the seek was refused before it was touched.
                let (attached, untouched) = {
                    let st = me.0.st.borrow();
                    (st.attached, st.last_ready.as_ref().and_then(|j| j.playlist.clone()) == live.playlist)
                };
                if attached && untouched {
                    me.0.player.cancel_skip();
                    me.0.player.play();
                } else {
                    me.attach(&live);
                }
                me.clear_play_activity();
                if untouched {
                    me.set_status(&failure);
                }
                return;
            }
            match me.0.target.get_untracked().ok().flatten() {
                Some(t) => me.play(PlayTarget { resume_seconds: (at > 60.0).then_some(at), ..t }),
                None => {
                    me.clear_play_activity();
                    me.0.player.cancel_skip();
                    me.set_status(&failure);
                }
            }
        });
    }

    /// Play after a failure (player.ts `togglePlay`): the item again when it never started,
    /// else the stream again from where it stopped.
    fn retry(&self, at: f64) {
        let (failed, job, last, attached, left) = {
            let st = self.0.st.borrow();
            (st.start_failed, st.job.clone(), st.last_ready.clone(), st.attached, st.left_at)
        };
        let target = self.0.target.get_untracked().ok().flatten();
        if failed {
            if let Some(t) = target {
                self.play(t);
            }
            return;
        }
        let (Some(_), Some(last), false) = (job, last, attached) else { return };
        let at = if left > 0.0 { left } else { at };
        let end = self.progress_duration();
        if at > 60.0 && (end <= 0.0 || end - at > 30.0) {
            self.skip_to(at);
        } else {
            self.attach(&last);
        }
    }

    /// Quality, once the menu sat still (player.ts `applyQuality`): a lower or equal height
    /// restarts only the transcode, here; a taller one is a different copy, which the add-on
    /// has to find again.
    pub(crate) fn choose_quality(&self, chosen: Quality) {
        if let Some(t) = self.0.st.borrow_mut().quality_timer.take() {
            self.0.ui.cancel_timer(t);
        }
        let t = self.after(SETTLE, move |me| {
            me.0.st.borrow_mut().quality_timer = None;
            me.apply_quality(chosen);
        });
        self.0.st.borrow_mut().quality_timer = Some(t);
    }

    fn apply_quality(&self, chosen: Quality) {
        let before = prefs::now().quality;
        prefs::set(&self.0.ctx, prefs::Patch { quality: Some(chosen), ..Default::default() });
        let Some(target) = self.0.target.get_untracked().ok().flatten() else { return };
        let resume = self.position();
        let replay = {
            let t = target.clone();
            move |me: &Dock| me.play(PlayTarget { resume_seconds: (resume > 60.0).then_some(resume), ..t.clone() })
        };
        let (id, attached, ready) = {
            let st = self.0.st.borrow();
            (st.job.clone(), st.attached, st.last_ready.is_some())
        };
        let skipping = self.0.player.skipping().is_some();
        let taller = quality_height(chosen) > quality_height(before);
        let Some(id) = id.filter(|_| attached && ready && !skipping && !taller) else {
            replay(self);
            return;
        };
        let (token, mine) = self.begin_hold(&target.title, resume);
        self.run(api::set_quality(&id, chosen, resume), move |me, out| {
            if !me.holds(token, mine) {
                return;
            }
            match out {
                Ok(job) if job.playlist.is_some() => me.attach(&job),
                _ => replay(me),
            }
        });
    }

    /// The picture waits for a new stream of the same place (a quality or audio switch).
    fn begin_hold(&self, title: &str, at: f64) -> (u64, u64) {
        let ids = {
            let mut st = self.0.st.borrow_mut();
            st.skip_generation += 1;
            (st.generation, st.skip_generation)
        };
        self.0.player.hold_for_source(dur(at));
        self.show_play(PlayDescription::new(if title.is_empty() { "Playback" } else { title }, PlayPhase::Preparing), None, false);
        ids
    }

    fn holds(&self, token: u64, mine: u64) -> bool {
        let st = self.0.st.borrow();
        st.generation == token && st.skip_generation == mine
    }

    /// An audio pick, once the menu sat still.
    pub(crate) fn choose_audio(&self, index: u32) {
        {
            let mut st = self.0.st.borrow_mut();
            st.audio_pending = true;
            if let Some(t) = st.audio_timer.take() {
                self.0.ui.cancel_timer(t);
            }
        }
        let t = self.after(SETTLE, move |me| {
            {
                let mut st = me.0.st.borrow_mut();
                st.audio_timer = None;
                st.audio_pending = false;
            }
            me.apply_audio(index);
        });
        self.0.st.borrow_mut().audio_timer = Some(t);
    }

    /// player.ts `applyAudio`: the playing file onto another track; the add-on is not asked
    /// again. A tagged pick is remembered; one with no language does not wipe the last.
    fn apply_audio(&self, index: u32) {
        let (tracks, current) = self.0.audio.get_untracked().unwrap_or_default();
        if Some(index) == current {
            return;
        }
        let last = self.0.st.borrow().last_ready.clone();
        let Some(chosen) = tracks.iter().find(|t| t.index == index).cloned() else {
            self.paint_audio(last.as_ref());
            return;
        };
        if let Some(lang) = chosen.lang.clone() {
            prefs::set(&self.0.ctx, prefs::Patch { audio_language: Some(Some(lang)), ..Default::default() });
        }
        let resume = self.position();
        let (id, attached) = {
            let st = self.0.st.borrow();
            (st.job.clone(), st.attached)
        };
        let Some(id) = id else { return };
        if let Some(dev) = self.0.cast.current() {
            cast::switch_audio(self, dev, id, index, resume);
            return;
        }
        if !attached || last.is_none() || self.0.player.skipping().is_some() {
            self.paint_audio(last.as_ref());
            return;
        }
        let title = self.0.target.get_untracked().ok().flatten().map(|t| t.title).unwrap_or_default();
        let (token, mine) = self.begin_hold(&title, resume);
        self.run(api::set_audio(&id, index, resume), move |me, out| {
            if !me.holds(token, mine) {
                return;
            }
            let restore = |me: &Dock, why: String| {
                me.clear_play_activity();
                me.set_status(&why);
                let last = me.0.st.borrow().last_ready.clone();
                me.paint_audio(last.as_ref());
                me.0.player.cancel_skip();
            };
            match out {
                Ok(job) if job.playlist.is_some() => me.attach(&job),
                Ok(job) => restore(me, job.error.unwrap_or_else(|| "The audio track did not start.".into())),
                Err(e) => restore(me, e.detail),
            }
        });
    }

    /// player.ts `paintAudio`: the file's tracks; one track shows and cannot be changed.
    fn paint_audio(&self, job: Option<&PlayJob>) {
        let tracks = job.map(|j| j.audio.clone()).unwrap_or_default();
        let same = self.0.audio.with_untracked(|(t, _)| *t == tracks).unwrap_or(false);
        if self.0.st.borrow().audio_pending && same {
            return;
        }
        let chosen = job
            .and_then(|j| j.audio_index)
            .filter(|i| tracks.iter().any(|t| t.index == *i))
            .or_else(|| tracks.first().map(|t| t.index));
        self.0.audio.set((tracks, chosen));
    }

    /// "Choose a stream link" (player.ts `playWithLinks`): the add-on's source list, from
    /// where it is.
    pub(crate) fn play_with_links(&self) {
        let Some(t) = self.0.target.get_untracked().ok().flatten().filter(|t| !t.file.is_empty()) else { return };
        let resume = self.position();
        self.play(PlayTarget { manual_links: true, resume_seconds: (resume > 60.0).then_some(resume), ..t });
    }

    // ---- Next ----

    fn set_next(&self, next: Option<NextItem>) {
        let next = next.filter(|n| !n.file.is_empty());
        self.0.player.set_next_up(next.as_ref().map(|n| NextUp::later(next_card_title(n))));
        self.0.next.set(next);
    }

    /// Next episode, N and Play now: the player asks for it ([`Player::on_next_needed`]).
    pub(crate) fn play_next(&self) {
        if self.0.next.with_untracked(Option::is_some).unwrap_or(false) {
            self.0.player.play_next();
        }
    }

    /// Near the end: ask for a next season found after the job started (player.ts
    /// `refreshNext` runs on the ticker), and with autoplay on, ask Portal to resolve the
    /// next now (`askEarly`), again every few seconds while near the end.
    fn near_end(&self, at: f64) {
        let Some(job) = self.0.st.borrow().job.clone() else { return };
        let duration = self.progress_duration();
        let left = duration - at;
        let next = self.0.next.get_untracked().ok().flatten();
        let started = self.0.st.borrow().attached && untrack(|| self.0.player.state() == PlaybackState::Playing);
        if let Some(n) = next.filter(|_| started && prefs::now().autoplay) {
            if duration > 60.0 && left > 0.0 && left <= EARLY_LEAD {
                let now = self.now_ms();
                let due = self.0.st.borrow().early_at.is_none_or(|t| now - t >= 4000.0);
                if due {
                    self.0.st.borrow_mut().early_at = Some(now);
                    let body = play_body(&target_from_next(&n), &prefs::now());
                    self.run(api::early(&job, &body), |_, _| {});
                }
            }
        }
    }

    /// The ticker's look for a next season (player.ts `refreshNext`), once per job.
    fn refresh_next(&self, token: u64) {
        let Some(id) = self.0.st.borrow().job.clone() else { return };
        if self.0.next.with_untracked(Option::is_some).unwrap_or(true) || self.0.st.borrow().next_asked == id {
            return;
        }
        let left = self.progress_duration() - self.position();
        if !(left > 0.0 && left <= NEXT_LOOKUP) {
            return;
        }
        self.0.st.borrow_mut().next_asked = id.clone();
        self.run(api::job(&id), move |me, out| {
            if me.generation() != token || me.0.st.borrow().job.as_deref() != Some(&id) {
                return;
            }
            if let Some(n) = out.ok().and_then(|j| j.next).filter(|n| !n.file.is_empty()) {
                me.set_next(Some(n));
            }
        });
    }

    // ---- Progress ----

    fn position(&self) -> f64 {
        let (attached, left) = {
            let st = self.0.st.borrow();
            (st.attached, st.left_at)
        };
        // Nothing plays here (the stream stopped): the place is where it left off.
        if !attached && left > 0.0 {
            return left;
        }
        secs(untrack(|| self.0.player.position()))
    }

    fn progress_duration(&self) -> f64 {
        let d = self.0.st.borrow().job_duration;
        if d > 0.0 && d.is_finite() {
            return d;
        }
        untrack(|| self.0.player.duration()).map_or(0.0, secs)
    }

    /// player.ts `postProgress`, then `then` once it went (or was not needed).
    fn post_progress(&self, force: bool, keepalive: bool, then: Option<Box<dyn FnOnce(&Dock)>>) {
        let body = self.progress_body(force);
        let Some(body) = body else {
            if let Some(f) = then {
                f(self);
            }
            return;
        };
        let id = body.report.id.clone();
        let mut call = api::progress(&body);
        if keepalive {
            call = call.keepalive();
        }
        let progress = self.0.ctx.progress;
        self.run(call, move |me, _| {
            // A page showing this title's episodes refreshes what it says was watched.
            if force {
                progress.update(|p| *p = (id, p.1 + 1));
            }
            if let Some(f) = then {
                f(me);
            }
        });
    }

    fn progress_body(&self, force: bool) -> Option<ProgressBody> {
        if untrack(|| self.0.player.skipping().is_some()) {
            return None;
        }
        let t = self.0.target.get_untracked().ok().flatten().filter(|t| t.track_progress)?;
        let now = self.now_ms();
        {
            let st = self.0.st.borrow();
            if !force && st.last_post.is_some_and(|l| now - l < PROGRESS_EVERY) {
                return None;
            }
            // Nothing attached and no place known: saving now would reset it. A TV's place is.
            if !st.attached && st.left_at <= 0.0 && !self.0.cast.active() {
                return None;
            }
        }
        let duration = self.progress_duration();
        let position = self.position();
        if !(duration.is_finite() && duration > 0.0 && position.is_finite() && position >= 0.0) {
            return None;
        }
        self.0.st.borrow_mut().last_post = Some(now);
        let tv = t.progress_kind == TitleKind::Tv;
        Some(ProgressBody {
            report: ProgressReport {
                id: t.progress_id.clone(),
                kind: t.progress_kind,
                name: progress_name(&t),
                art: t.art.clone().filter(|a| !a.is_empty()),
                season: t.season.filter(|_| tv),
                episode: t.episode.filter(|e| tv && *e > 0),
                episode_name: tv.then(|| t.title.clone()),
                file: Some(t.file.clone()),
                position,
                duration,
            },
            flush: force,
        })
    }

    fn stop_ticker(&self) {
        if let Some(t) = self.0.st.borrow_mut().ticker.take() {
            self.0.ui.cancel_timer(t);
        }
    }

    /// Every 8 s: progress (at most every 5 s), a late next season, and the heartbeat.
    fn start_ticker(&self) {
        self.stop_ticker();
        let token = self.generation();
        fn tick(me: &Dock, token: u64) {
            let t = me.after(TICK, move |me| {
                if me.generation() != token {
                    return;
                }
                me.post_progress(false, false, None);
                me.refresh_next(token);
                me.heartbeat(token);
                tick(me, token);
            });
            me.0.st.borrow_mut().ticker = Some(t);
        }
        tick(self, token);
    }

    /// player.ts `heartbeat`: tells Portal this page still has its stream, paused or not;
    /// a stream Portal had to end says why.
    fn heartbeat(&self, token: u64) {
        let (id, attached) = {
            let st = self.0.st.borrow();
            (st.job.clone(), st.attached)
        };
        let Some(id) = id.filter(|_| attached && untrack(|| self.0.player.skipping().is_none())) else { return };
        let id2 = id.clone();
        self.run(api::job(&id), move |me, out| {
            if me.generation() != token || me.0.st.borrow().job.as_deref() != Some(&id2) {
                return;
            }
            if let Some(err) = out.ok().filter(|j| j.state == JobState::Failed).and_then(|j| j.error) {
                me.set_status(&err);
            }
        });
    }

    /// What the player's state means here: a pause saves the place, a long wait asks whether
    /// the stream died, a failed stream says why, the end saves and moves on.
    fn state_changed(&self, s: PlaybackState) {
        let before = self.0.st.borrow_mut().last_state.replace(s);
        if before == Some(s) {
            return;
        }
        if let Some(t) = self.0.st.borrow_mut().stall_timer.take() {
            self.0.ui.cancel_timer(t);
        }
        let attached = self.0.st.borrow().attached;
        match s {
            PlaybackState::Paused if attached => self.post_progress(true, false, None),
            PlaybackState::Buffering if attached => self.check_stall(),
            PlaybackState::Failed if attached && self.0.player.error().is_some() => {
                // Out of retries: say why instead of spinning. Play picks up from here.
                let text = stream_failure(self.0.player.error().as_ref());
                let at = self.position();
                {
                    let mut st = self.0.st.borrow_mut();
                    st.left_at = at;
                    st.attached = false;
                }
                self.0.status.set_if_changed(with_retry(text));
                self.0.player.fail(text);
            }
            PlaybackState::Ended if attached => self.ended(),
            _ => {}
        }
    }

    /// player.ts `checkStall`: a long wait for video can mean Portal's transcode stopped.
    fn check_stall(&self) {
        let (token, id) = (self.generation(), self.0.st.borrow().job.clone());
        let Some(id) = id else { return };
        let t = self.after(STALL, move |me| {
            me.0.st.borrow_mut().stall_timer = None;
            let waiting = untrack(|| me.0.player.state() == PlaybackState::Buffering);
            if me.generation() != token || me.0.st.borrow().job.as_deref() != Some(&id) || !waiting {
                return;
            }
            let id2 = id.clone();
            me.run(api::job(&id), move |me, out| {
                let Ok(job) = out else { return };
                let waiting = untrack(|| me.0.player.state() == PlaybackState::Buffering);
                if me.generation() != token || me.0.st.borrow().job.as_deref() != Some(&id2) || !waiting || job.state != JobState::Failed {
                    return;
                }
                let at = me.position();
                {
                    let mut st = me.0.st.borrow_mut();
                    st.left_at = at;
                    st.attached = false;
                }
                let why = job.error.unwrap_or_else(|| "The stream stopped.".into());
                me.0.status.set_if_changed(with_retry(&why));
                me.0.player.fail(why);
            });
        });
        self.0.st.borrow_mut().stall_timer = Some(t);
    }

    /// The end: the place is saved; a next season found late plays with autoplay on (a next
    /// known before plays through the player's own Up next).
    fn ended(&self) {
        let token = self.generation();
        let known = self.0.next.get_untracked().ok().flatten();
        let job = self.0.st.borrow().job.clone();
        self.post_progress(
            true,
            false,
            Some(Box::new(move |me: &Dock| {
                if me.generation() != token || known.is_some() {
                    return;
                }
                let Some(id) = job else { return };
                me.run(api::job(&id), move |me, out| {
                    if me.generation() != token {
                        return;
                    }
                    let Some(n) = out.ok().and_then(|j| j.next).filter(|n| !n.file.is_empty()) else { return };
                    me.set_next(Some(n));
                    if prefs::now().autoplay {
                        me.play_next();
                    }
                });
            })),
        );
    }

    // ---- Layout ----

    /// Theater mode, asked for by the player's button or T (player.ts `toggleTheater`).
    fn set_theater(&self, on: bool) {
        if on {
            // Opening wide loads the season again; a quality change does not.
            self.0.episodes.reload();
        }
        self.0.theater.set_if_changed(on);
        self.request_episodes();
    }

    /// Close (player.ts `#dock-close`): saves the place, stops the job, and puts focus back
    /// on what was played if it was in the dock.
    pub(crate) fn close(&self) {
        if crate::screens::info::is_open() {
            crate::screens::info::close(false);
        }
        let report = self.0.target.get_untracked().ok().flatten().is_some_and(|t| t.track_progress);
        self.0.st.borrow_mut().generation += 1;
        self.0.episodes.release();
        self.release_current(report, false);
        let back = self.0.st.borrow_mut().opener.take();
        let had_focus = view::dock_has_focus(self);
        self.hide_dock();
        if let (true, Some(b)) = (had_focus, back) {
            b.focus();
        }
    }

    /// player.ts `hideDock`: full screen and picture in picture would stay up, empty.
    fn hide_dock(&self) {
        let p = &self.0.player;
        if p.is_fullscreen() {
            p.request_fullscreen(false);
        }
        if p.is_pip() {
            p.exit_pip();
        }
        p.set_compact(false);
        // The wish stays, so the next item in this dock session opens wide again.
        self.0.open.set_if_changed(false);
        self.set_status("");
        self.0.title.set_if_changed(String::new());
        p.set_title(None);
        p.set_status(None);
    }

    fn request_episodes(&self) {
        episodes::request(self);
    }

    fn episodes_after_resolve(&self) {
        episodes::after_resolve(self);
    }

    /// Whether a play activity (a resolve) runs now: Kodi is busy, the list waits.
    pub(crate) fn resolving(&self) -> bool {
        self.0.st.borrow().play_ticket != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_targets_as_player_ts() {
        assert_eq!(human_title(Some(r#"{"poster": "x"}"#)), "");
        assert_eq!(human_title(Some(" Dune ")), "Dune");
        assert_eq!(display_media_title("Show - S01E02 - Pilot", Some(1), Some(2), Some("Show")), "S01E02 · Pilot");
        let n = NextItem {
            kind: PlayKind::Episode,
            id: "ep".into(),
            file: "f".into(),
            title: "S02E01 Return".into(),
            season: Some(2),
            episode: Some(1),
            show_id: Some("show".into()),
            show_title: Some("Show".into()),
            art: None,
        };
        assert_eq!(next_detail(&n), "S02E01 · Return");
        let t = target_from_next(&n);
        assert_eq!((t.id.as_str(), t.progress_id.as_str(), t.progress_kind), ("show", "show", TitleKind::Tv));
        assert!(t.track_progress);
        assert_eq!(with_retry("No stream."), "No stream. Press Play to try again.");
        assert_eq!(with_retry("Busy, try again later."), "Busy, try again later.");
        assert_eq!(applied_seek(Some(61.7)), 61.0);
        assert_eq!(applied_seek(Some(59.0)), 0.0);
        assert!(quality_height(Quality::P720) < quality_height(Quality::Auto));
        assert!(quality_height(Quality::Source).is_infinite());
    }
}
