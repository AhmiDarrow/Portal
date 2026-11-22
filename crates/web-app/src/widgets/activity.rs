//! The header's activity panel and the notice line under it (web/src/activity.ts, word for
//! word): what Portal is busy with, its progress, and a Cancel when the work allows it.
//!
//! ```ignore
//! let ticket = ctx.activity.begin(Owner::Page, describe_library(Library::Home), 350);
//! // ... when the page is drawn:
//! ctx.activity.end(ticket);
//! ctx.activity.notice("Enter a name.");
//! ```

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use remedy_ui::a11y::{Live, Role, Semantics};
use remedy_ui::core::{CornerRadii, Rect};
use remedy_ui::layout::{Align, Justify, Length};
use remedy_ui::paint::Brush;
use remedy_ui::reactive::Signal;
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{show, El, Ui};

use crate::theme::Look;

/// What the panel says: a title, a detail, and the share done when it is known.
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityInput {
    pub title: String,
    pub detail: String,
    /// 0..1 from Kodi; `None` when the amount of work is unknown.
    pub progress: Option<f64>,
}

impl ActivityInput {
    pub fn new(title: impl Into<String>, detail: impl Into<String>) -> Self {
        ActivityInput { title: title.into(), detail: detail.into(), progress: None }
    }
}

/// What the panel shows for an input (activity.ts `activityView`).
#[derive(Clone, Debug, PartialEq)]
pub struct ActivityView {
    pub title: String,
    pub detail: String,
    /// The first non-blank line of the detail.
    pub status: String,
    pub percent: Option<u32>,
}

impl ActivityView {
    pub fn of(input: &ActivityInput) -> ActivityView {
        let title = non_empty(input.title.trim(), "Working");
        let detail = non_empty(input.detail.trim(), "Working.");
        let status = detail.split('\n').map(str::trim).find(|p| !p.is_empty()).unwrap_or(&detail).to_owned();
        let percent = input.progress.filter(|p| p.is_finite()).map(|p| (p * 100.0).round().clamp(0.0, 100.0) as u32);
        ActivityView { title, detail, status, percent }
    }
}

fn non_empty(s: &str, or: &str) -> String {
    if s.is_empty() { or.to_owned() } else { s.to_owned() }
}

/// Who started the work: playback keeps the panel until it ends, so a page refresh does
/// not cover the add-on's work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Page,
    Play,
}

/// The Cancel button: shown when there is a cancel (or one that is not allowed yet).
#[derive(Clone, Default)]
pub struct CancelSpec {
    pub on_cancel: Option<Rc<dyn Fn()>>,
    pub disabled: bool,
}

#[derive(Clone)]
struct Shown {
    view: ActivityView,
    cancel: CancelSpec,
}

/// The shared activity panel and notice line (in [`crate::Ctx`]).
#[derive(Clone)]
pub struct Activity {
    ui: Ui,
    shown: Signal<Option<Rc<Shown>>>,
    notice: Signal<String>,
    seq: Rc<Cell<u64>>,
    shown_ticket: Rc<Cell<u64>>,
    shown_owner: Rc<Cell<Option<Owner>>>,
    /// The ticket waiting for its delay, and its timer.
    pending: Rc<Cell<Option<(u64, u64)>>>,
}

impl Activity {
    pub fn new(ui: &Ui) -> Activity {
        Activity {
            ui: ui.clone(),
            shown: Signal::new(None),
            notice: Signal::new(String::new()),
            seq: Rc::default(),
            shown_ticket: Rc::default(),
            shown_owner: Rc::default(),
            pending: Rc::default(),
        }
    }

    /// Shows `input` after `delay_ms` (0: now), so a fast load never flashes the panel.
    /// Returns the ticket that [`end`](Self::end) takes.
    pub fn begin(&self, owner: Owner, input: ActivityInput, delay_ms: u64) -> u64 {
        self.begin_with(owner, input, delay_ms, CancelSpec::default())
    }

    /// [`begin`](Self::begin) with a Cancel button.
    pub fn begin_with(&self, owner: Owner, input: ActivityInput, delay_ms: u64, cancel: CancelSpec) -> u64 {
        let ticket = self.seq.get() + 1;
        self.seq.set(ticket);
        if let Some((_, timer)) = self.pending.take() {
            self.ui.cancel_timer(timer);
        }
        let me = self.clone();
        let start = move || {
            me.pending.set(None);
            if me.shown_owner.get() == Some(Owner::Play) && owner != Owner::Play {
                return;
            }
            me.shown_ticket.set(ticket);
            me.shown_owner.set(Some(owner));
            me.shown.set(Some(Rc::new(Shown { view: ActivityView::of(&input), cancel })));
        };
        if delay_ms > 0 {
            let timer = self.ui.after(Duration::from_millis(delay_ms), start);
            self.pending.set(Some((ticket, timer)));
        } else {
            start();
        }
        ticket
    }

    /// Ends the work `ticket` stands for: hides the panel if it shows it.
    pub fn end(&self, ticket: u64) {
        if let Some((t, timer)) = self.pending.get() {
            if t == ticket {
                self.ui.cancel_timer(timer);
                self.pending.set(None);
            }
        }
        if self.shown_ticket.get() != ticket {
            return;
        }
        self.shown_ticket.set(0);
        self.shown_owner.set(None);
        self.shown.set(None);
    }

    /// Ends whatever page work is shown or waiting (a new page replaces the old one's),
    /// leaving playback's panel up (main.ts `hidePage` on every render).
    pub fn end_page(&self) {
        if let Some((t, _)) = self.pending.get() {
            self.end(t);
        }
        if self.shown_owner.get() == Some(Owner::Page) {
            self.end(self.shown_ticket.get());
        }
    }

    /// The window's UI, for timers.
    pub fn ui(&self) -> Ui {
        self.ui.clone()
    }

    /// Sets the notice line under the header ("" hides it).
    pub fn notice(&self, text: impl Into<String>) {
        self.notice.set(text.into());
    }
}

/// The panel, a full row of the header (index.html `#activity`).
pub fn panel(ui: &Ui, activity: &Activity) -> El {
    let shown = activity.shown;
    let u = ui.clone();
    show(ui, move || shown.with(Option::is_some), move || {
        let Some(s) = shown.get() else { return w::column(&u) };
        body(&u, &s)
    })
    .el()
    .fill_width()
}

fn body(ui: &Ui, s: &Shown) -> El {
    let v = &s.view;
    let mut head = ui
        .el("p_activity_head")
        .row()
        .align(Align::Baseline)
        .justify(Justify::SpaceBetween)
        .gap(16.0)
        .child(
            ui.el("p_activity_title")
                .text(v.title.clone())
                .semantics(Semantics::new(Role::Heading(2)).label(v.title.clone()))
                .layout(|l| l.min_width = Length::Px(0.0)),
        );
    if let Some(p) = v.percent {
        head = head.child(ui.el("p_activity_percent").text(format!("{p}%")).nowrap().layout(|l| l.shrink = 0.0));
    }
    let percent = v.percent;
    let label = v.title.clone();
    let meter = ui
        .el("p_activity_meter")
        .fill_width()
        .height(12.0)
        .paint(move |cx| {
            let (w, h) = (cx.size.width, cx.size.height);
            let r = CornerRadii::all(h / 2.0);
            let track = cx.style.background;
            cx.list.fill_rounded(Rect::new(0.0, 0.0, w, h), r, Brush::Solid(track));
            let (x0, x1) = match percent {
                Some(p) => (0.0, w * p as f32 / 100.0),
                // `activity-slide`: a third of the track from -130% to 340% of its own width,
                // 1.15 s ease-in-out, forever; still and full under reduced motion.
                None if cx.reduced_motion => (0.0, w),
                None => {
                    cx.request_next_frame();
                    let t = (cx.time.as_secs_f32() / 1.15).fract();
                    let e = remedy_ui::motion::Easing::EASE_IN_OUT.apply(t);
                    let bw = w * 0.34;
                    let x = bw * (-1.3 + 4.7 * e);
                    (x.max(0.0), (x + bw).min(w))
                }
            };
            if x1 > x0 {
                cx.list.push_clip(Rect::new(0.0, 0.0, w, h), r);
                cx.list.fill_rounded(Rect::new(x0, 0.0, x1, h), CornerRadii::all(h / 2.0), Brush::Solid(cx.style.foreground));
                cx.list.pop_clip();
            }
        })
        .semantics(move || {
            let s = Semantics::new(Role::Progress).label(label.clone());
            match percent {
                Some(p) => s.numeric(f64::from(p), 0.0, 100.0),
                None => s,
            }
        });
    let mut col = ui
        .el("p_activity")
        .column()
        .gap(8.8)
        .fill_width()
        .child(head)
        .child(
            ui.el("p_activity_detail")
                .text(v.detail.clone())
                .semantics(Semantics::new(Role::Label).label(v.detail.clone()).live(Live::Polite)),
        )
        .child(meter);
    if s.cancel.on_cancel.is_some() || s.cancel.disabled {
        let on = s.cancel.on_cancel.clone();
        let b = crate::widgets::button(ui, "Cancel", move || {
            if let Some(f) = &on {
                f();
            }
        })
        .disabled(on_is_off(&s.cancel));
        col = col.child(w::row(ui).child(b));
    }
    col
}

fn on_is_off(c: &CancelSpec) -> bool {
    c.on_cancel.is_none() || c.disabled
}

/// The notice line (index.html `#notice`): a polite status, hidden while empty.
pub fn notice(ui: &Ui, activity: &Activity) -> El {
    let text = activity.notice;
    let u = ui.clone();
    show(ui, move || text.with(|t| !t.is_empty()), move || {
        u.el("p_notice")
            .text(move || text.get())
            .semantics(move || Semantics::new(Role::Status).label(text.get()).live(Live::Polite))
    })
    .el()
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    use remedy_ui::core::Insets;
    let pad = |y: f32, x: f32| Insets { top: y, bottom: y, start: x, end: x };
    s.rule(
        "p_activity",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .padding(pad(14.4, 16.0))
            .shadow(k.shadow),
    );
    s.rule("p_activity_title", None, States::NONE, StylePatch::default().heading(true).text_size(16.0).foreground(k.text));
    s.rule(
        "p_activity_percent",
        None,
        States::NONE,
        StylePatch::default().heading(true).text_size(24.0).foreground(k.gold),
    );
    s.rule("p_activity_detail", None, States::NONE, StylePatch::default().foreground(k.text));
    s.rule("p_activity_meter", None, States::NONE, StylePatch::default().background(k.track).foreground(k.gold));
    s.rule(
        "p_notice",
        None,
        States::NONE,
        StylePatch::default()
            .foreground(k.gold)
            .background(crate::theme::mix(k.gold, 0.10, k.raised))
            .border(crate::theme::mix(k.gold, 0.38, k.line))
            .border_width(1.0)
            .radius(12.0)
            .padding(pad(10.4, 21.6)),
    );
    s.rule("p_notice", Some("narrow"), States::NONE, StylePatch::default().radius(10.0).padding(pad(10.4, 13.6)));
}

// ---- The words, as activity.ts has them ----

/// The pages that load the library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Library {
    Home,
    Title,
    Watchlist,
    Bookmarks,
    History,
}

pub fn describe_library(kind: Library) -> ActivityInput {
    let (t, d) = match kind {
        Library::Home => ("Home", "Loading your library."),
        Library::Title => ("Title", "Loading this title."),
        Library::Watchlist => ("Watchlist", "Loading the watchlist."),
        Library::Bookmarks => ("Bookmarks", "Loading bookmarks."),
        Library::History => ("History", "Loading history."),
    };
    ActivityInput::new(t, d)
}

/// How a folder is being opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowseMode {
    Open,
    Refresh,
    Starting,
}

pub fn describe_browse(name: &str, mode: BrowseMode) -> ActivityInput {
    let label = non_empty(&visible_text(name), "This folder");
    let search = has_word(&label, "search");
    match mode {
        BrowseMode::Refresh => ActivityInput::new(
            "Refreshing",
            if search { "Checking this search folder for changes." } else { "Checking this folder for changes." },
        ),
        BrowseMode::Starting => ActivityInput::new(
            format!("Opening {label}"),
            if search {
                "Starting Kodi, then opening this search folder."
            } else {
                "Starting Kodi, then opening this folder."
            },
        ),
        BrowseMode::Open => ActivityInput::new(
            format!("Opening {label}"),
            if search { "Opening this search folder from the add-on." } else { "Opening this folder from the add-on." },
        ),
    }
}

pub fn describe_search(query: &str, starting: bool) -> ActivityInput {
    let q = clip(&visible_text(query), 80);
    ActivityInput::new(
        if q.is_empty() { "Searching".to_owned() } else { format!("Searching for {q}") },
        if starting { "Starting Kodi, then searching the add-on." } else { "Searching the add-on." },
    )
}

pub fn describe_source(starting: bool) -> ActivityInput {
    ActivityInput::new(
        "Loading add-ons",
        if starting { "Starting Kodi, then reading installed video add-ons." } else { "Reading installed video add-ons." },
    )
}

pub fn describe_settings(starting: bool) -> ActivityInput {
    ActivityInput::new(
        "Loading settings",
        if starting { "Starting Kodi, then reading this add-on's settings." } else { "Reading this add-on's settings." },
    )
}

pub fn describe_repository_install() -> ActivityInput {
    ActivityInput::new("Installing the repository", "Downloading it from the link and writing the add-on files.")
}

pub fn describe_addon_install(name: &str) -> ActivityInput {
    let label = non_empty(&visible_text(name), "this add-on");
    ActivityInput::new(format!("Installing {label}"), "Downloading it from the repository and writing the files.")
}

pub fn describe_addon_update(name: &str) -> ActivityInput {
    let label = non_empty(&visible_text(name), "this add-on");
    ActivityInput::new(format!("Updating {label}"), "Downloading the new version and swapping it in.")
}

/// What is being removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removed {
    Repository,
    Video,
}

pub fn describe_remove(kind: Removed, name: &str) -> ActivityInput {
    let fallback = if kind == Removed::Repository { "the repository" } else { "the add-on" };
    let label = non_empty(&visible_text(name), fallback);
    ActivityInput::new(
        format!("Removing {label}"),
        match kind {
            Removed::Repository => "Deleting the repository from this PC.",
            Removed::Video => "Deleting the add-on and its settings from this PC.",
        },
    )
}

pub fn describe_repository_open(name: &str) -> ActivityInput {
    let label = non_empty(&visible_text(name), "this repository");
    ActivityInput::new(format!("Opening {label}"), "Reading its video add-ons.")
}

pub fn describe_action(name: Option<&str>) -> ActivityInput {
    let label = visible_text(name.unwrap_or(""));
    ActivityInput::new(
        if label.is_empty() { "Running that action".to_owned() } else { format!("Running {label}") },
        "Asking the add-on to run this action.",
    )
}

/// What the Kodi engine is asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServerAction {
    Restart,
    Quit,
}

pub fn describe_server(action: ServerAction) -> ActivityInput {
    match action {
        ServerAction::Quit => ActivityInput::new("Stopping Kodi", "Asking Kodi to quit."),
        ServerAction::Restart => ActivityInput::new("Restarting Kodi", "Stopping Kodi and starting it again."),
    }
}

/// Where playback is (activity.ts `PlayPhase`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayPhase {
    Starting,
    Opening,
    Waiting,
    Preparing,
    Working,
    Answering,
    Skipping,
}

/// The inputs of `describePlay`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayDescription<'a> {
    pub item: &'a str,
    pub phase: PlayPhase,
    pub heading: Option<&'a str>,
    pub message: Option<&'a str>,
    pub progress: Option<f64>,
    pub at: Option<&'a str>,
    pub canceling: bool,
    /// The source-list (Links) flow: never claims a stream is resolving.
    pub links: bool,
}

impl<'a> PlayDescription<'a> {
    pub fn new(item: &'a str, phase: PlayPhase) -> Self {
        PlayDescription { item, phase, heading: None, message: None, progress: None, at: None, canceling: false, links: false }
    }
}

pub fn describe_play(input: &PlayDescription<'_>) -> ActivityInput {
    let item = non_empty(&visible_text(input.item), "Playback");
    let links = input.links;
    let plain = |d: &str| ActivityInput::new(item.clone(), d);
    match input.phase {
        PlayPhase::Starting => {
            return plain(if links { "Starting Kodi for the source list." } else { "Starting Kodi." });
        }
        PlayPhase::Opening => {
            return plain(if links { "Opening the add-on source list." } else { "Asking the add-on to play this." });
        }
        PlayPhase::Waiting => {
            return plain(if links { "Waiting for the source list." } else { "Waiting for a playable stream." });
        }
        PlayPhase::Preparing => return plain(if links { "Opening the source list." } else { "Preparing the video." }),
        PlayPhase::Skipping => {
            let at = visible_text(input.at.unwrap_or(""));
            return plain(&if at.is_empty() { "Skipping to the place you chose.".to_owned() } else { format!("Skipping to {at}.") });
        }
        PlayPhase::Answering => {
            let heading = non_empty(&visible_text(input.heading.unwrap_or("")), &item);
            return ActivityInput::new(heading, if links { "Waiting for a source-list answer." } else { "Waiting for an answer." });
        }
        PlayPhase::Working => {}
    }
    let head = visible_text(input.heading.unwrap_or(""));
    let message = visible_detail(input.message.unwrap_or(""));
    let mut lines = Vec::new();
    if !head.is_empty() && head.to_lowercase() != item.to_lowercase() && head.to_lowercase() != "working" {
        lines.push(head);
    }
    if !message.is_empty() {
        lines.push(message);
    }
    let idle = if links { "The add-on is opening the source list." } else { "The add-on is finding sources." };
    let detail = if input.canceling {
        "Canceling.".to_owned()
    } else if lines.is_empty() {
        idle.to_owned()
    } else {
        lines.join("\n")
    };
    ActivityInput { title: item, detail, progress: input.progress }
}

fn has_word(text: &str, word: &str) -> bool {
    text.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| w == word)
}

/// Spaces folded, then [`safe_copy`].
fn visible_text(text: &str) -> String {
    safe_copy(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Lines kept, blank ones dropped, spaces and tabs folded in each, then [`safe_copy`].
fn visible_detail(text: &str) -> String {
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.split([' ', '\t']).filter(|p| !p.is_empty()).collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .collect();
    safe_copy(&lines.join("\n"))
}

/// Nothing that names a plugin address, a secret or a URL with credentials reaches the panel.
fn safe_copy(value: &str) -> String {
    let flat = value.to_lowercase();
    if value.is_empty()
        || flat.contains("plugin://")
        || ["token", "password", "api key", "apikey"].iter().any(|w| flat.contains(w))
        || has_credentials_url(value)
    {
        return String::new();
    }
    value.to_owned()
}

/// `[a-z][a-z0-9+.-]*://[^\s]*@`, case-insensitively.
fn has_credentials_url(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while let Some(at) = s[i..].find("://").map(|p| p + i) {
        let mut start = at;
        while start > 0 && (b[start - 1].is_ascii_alphanumeric() || b"+.-".contains(&b[start - 1])) {
            start -= 1;
        }
        let scheme_ok = start < at && b[start..at].iter().position(u8::is_ascii_alphabetic).is_some();
        if scheme_ok && s[at + 3..].split(char::is_whitespace).next().is_some_and(|rest| rest.contains('@')) {
            return true;
        }
        i = at + 3;
    }
    false
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_and_words_match_activity_ts() {
        let v = ActivityView::of(&ActivityInput { title: " ".into(), detail: "\n  First \nSecond".into(), progress: Some(0.456) });
        assert_eq!((v.title.as_str(), v.status.as_str(), v.percent), ("Working", "First", Some(46)));
        assert_eq!(ActivityView::of(&ActivityInput::new("a", "")).detail, "Working.");
        assert_eq!(describe_library(Library::Home), ActivityInput::new("Home", "Loading your library."));
        assert_eq!(describe_browse("My Search", BrowseMode::Open).detail, "Opening this search folder from the add-on.");
        assert_eq!(describe_browse("Researchers", BrowseMode::Open).detail, "Opening this folder from the add-on.");
        assert_eq!(describe_browse("", BrowseMode::Starting).title, "Opening This folder");
        assert_eq!(describe_search(&"x".repeat(90), false).title, format!("Searching for {}…", "x".repeat(79)));
        assert_eq!(describe_remove(Removed::Video, "  ").title, "Removing the add-on");
        assert_eq!(describe_action(Some("token reset")).title, "Running that action");
        assert_eq!(describe_addon_install("https://u:p@x.org/a").title, "Installing this add-on");
        let mut p = PlayDescription::new("Show  S01E01", PlayPhase::Working);
        p.heading = Some("Working");
        p.message = Some("  Resolving \n\n  link 2 ");
        assert_eq!(describe_play(&p).detail, "Resolving\nlink 2");
        p.canceling = true;
        assert_eq!(describe_play(&p).detail, "Canceling.");
        let mut p = PlayDescription::new("", PlayPhase::Skipping);
        p.at = Some("12:00");
        assert_eq!(describe_play(&p), ActivityInput::new("Playback", "Skipping to 12:00."));
    }
}
