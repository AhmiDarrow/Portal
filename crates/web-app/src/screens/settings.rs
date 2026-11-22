//! Portal's own settings ("Settings" in the nav, `#/server`; main.ts:2690-3730, plan P11):
//! a jump bar, then Appearance, Playback, Portal on this PC, Cast, Devices (PC only),
//! Subtitles, Media info, Kodi engine and Updates, each a card with the old page's controls,
//! words and server calls. A phone, tablet or TV shows Appearance and Playback only.
//!
//! Each card is built once and follows its own signals, so a switch keeps focus through a
//! save as the old page's `repaintCard` did by hand.

use std::cell::Cell;
use std::rc::Rc;

use portal_api::{
    AppView, CastBody, MetadataPut, MetadataSettings, PairedDevice, Quality, Status, SubtitleMode, SubtitleSettings,
    SubtitleSettingsPut, UpdateInfo, UpdateState,
};
use remedy_ui::a11y::{Action, Live, Role, Semantics};
use remedy_ui::core::{Color, CornerRadii, Insets, Point, Rect, Size};
use remedy_ui::layout::{Align, Justify, Length};
use remedy_ui::paint::Brush;
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{LineHeight, States, StylePatch, StyleSheet};
use remedy_ui::view::TextField;
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail, ApiError};
use crate::fonts::{FontId, Fonts};
use crate::prefs::{self, Patch};
use crate::theme::{self, mix, Look};
use crate::widgets::activity::{describe_server, Owner, ServerAction};
use crate::widgets::button;
use crate::widgets::confirm::confirm;
use crate::widgets::page::h1;
use crate::Ctx;

/// How many subtitle languages can be chosen (main.ts `MAX_SUB_LANGUAGES`);
/// [`set_language_limit`] changes it.
pub const LANGUAGE_LIMIT: usize = 4;

thread_local! {
    static LIMIT: Cell<usize> = const { Cell::new(LANGUAGE_LIMIT) };
}

pub fn set_language_limit(n: usize) {
    LIMIT.with(|l| l.set(n.max(1)));
}

fn language_limit() -> usize {
    LIMIT.with(Cell::get)
}

/// Languages offered for subtitles, by the code the services know (subtitles.ts
/// `COMMON_LANGUAGES`).
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt-BR", "Portuguese (Brazil)"),
    ("pt-PT", "Portuguese"),
    ("nl", "Dutch"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("no", "Norwegian"),
    ("fi", "Finnish"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
    ("el", "Greek"),
    ("tr", "Turkish"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("hr", "Croatian"),
    ("sr", "Serbian"),
    ("bg", "Bulgarian"),
    ("ar", "Arabic"),
    ("he", "Hebrew"),
    ("fa", "Persian"),
    ("hi", "Hindi"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("id", "Indonesian"),
    ("ms", "Malay"),
    ("zh-CN", "Chinese (Simplified)"),
    ("zh-TW", "Chinese (Traditional)"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
];

/// ISO 639-2 tags (subtitles.ts `THREE_LETTER`).
const THREE_LETTER: &[(&str, &str)] = &[
    ("eng", "en"), ("spa", "es"), ("fre", "fr"), ("fra", "fr"), ("ger", "de"), ("deu", "de"), ("ita", "it"),
    ("por", "pt"), ("dut", "nl"), ("nld", "nl"), ("swe", "sv"), ("dan", "da"), ("nor", "no"), ("nob", "no"),
    ("nno", "no"), ("fin", "fi"), ("pol", "pl"), ("cze", "cs"), ("ces", "cs"), ("hun", "hu"), ("rum", "ro"),
    ("ron", "ro"), ("gre", "el"), ("ell", "el"), ("tur", "tr"), ("rus", "ru"), ("ukr", "uk"), ("hrv", "hr"),
    ("srp", "sr"), ("bul", "bg"), ("ara", "ar"), ("heb", "he"), ("per", "fa"), ("fas", "fa"), ("hin", "hi"),
    ("tha", "th"), ("vie", "vi"), ("ind", "id"), ("may", "ms"), ("msa", "ms"), ("chi", "zh"), ("zho", "zh"),
    ("jpn", "ja"), ("kor", "ko"), ("slv", "sl"), ("slo", "sk"), ("slk", "sk"), ("est", "et"), ("lav", "lv"),
    ("lit", "lt"), ("ice", "is"), ("isl", "is"), ("cat", "ca"),
];

/// "en" from "en-US", "eng" or "EN" (subtitles.ts `primaryLanguage`).
pub fn primary_language(tag: &str) -> String {
    let text = tag.trim().to_lowercase().replace('_', "-");
    let primary = text.split('-').next().unwrap_or_default();
    THREE_LETTER.iter().find(|(k, _)| *k == primary).map_or(primary, |(_, v)| v).to_owned()
}

/// The browser's languages as subtitle codes, at most the limit, else English
/// (subtitles.ts `browserLanguages`).
pub fn browser_languages(locales: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in locales {
        let lower = raw.to_lowercase();
        let mut code = primary_language(raw);
        if code == "pt" {
            code = if lower.contains("br") { "pt-BR" } else { "pt-PT" }.into();
        }
        if code == "zh" {
            code = if ["tw", "hk", "mo", "hant"].iter().any(|t| lower.contains(t)) { "zh-TW" } else { "zh-CN" }.into();
        }
        if !code.is_empty() && !out.contains(&code) {
            out.push(code);
        }
        if out.len() == language_limit() {
            break;
        }
    }
    if out.is_empty() {
        out.push("en".into());
    }
    out
}

fn language_name(code: &str) -> String {
    LANGUAGES.iter().find(|(c, _)| *c == code).map_or(code, |(_, n)| n).to_owned()
}

/// "A and B", or "A, B and C".
fn spoken_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [a] => (*a).to_owned(),
        [rest @ .., last] if items.len() >= 3 => format!("{} and {last}", rest.join(", ")),
        _ => items.join(" and "),
    }
}

const QUALITIES: [(Quality, &str); 6] = [
    (Quality::Auto, "Auto"),
    (Quality::Source, "Source"),
    (Quality::P2160, "4K"),
    (Quality::P1080, "1080p"),
    (Quality::P720, "720p"),
    (Quality::P480, "480p"),
];

const RESTART_TEXT: &str = "Restart the Portal server? Anything playing will stop.";
const CAST_RESTART_TEXT: &str = "Portal needs to restart before this takes effect.";

/// The Cast card's facts (main.ts `ServerCast`).
#[derive(Clone, Debug, PartialEq, Default)]
struct CastView {
    lan: bool,
    url: String,
    restart: bool,
}

impl CastView {
    fn from(body: &CastBody, before: Option<&CastView>) -> CastView {
        CastView {
            lan: body.lan,
            url: body.url.clone().unwrap_or_default(),
            restart: body.restart_needed.or(before.map(|b| b.restart)).unwrap_or(false),
        }
    }
}

/// The TV page's address (main.ts `castPageUrl`): only http(s), ending in `#/cast`.
fn cast_page_url(url: &str) -> String {
    let t = url.trim();
    let lower = t.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return String::new();
    }
    let bare = t.trim_end_matches('/');
    if bare.ends_with("#/cast") {
        return t.to_owned();
    }
    format!("{}/#/cast", t.trim_end_matches('/'))
}

/// One card's data, its last error and note, and whether a save is under way.
struct Part<T: 'static> {
    data: Signal<Option<T>>,
    error: Signal<String>,
    note: Signal<String>,
    busy: Signal<bool>,
}

impl<T> Clone for Part<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Part<T> {}

impl<T: Clone + 'static> Part<T> {
    fn new() -> Self {
        Part { data: Signal::new(None), error: Signal::new(String::new()), note: Signal::new(String::new()), busy: Signal::new(false) }
    }
    fn busy_now(&self) -> bool {
        self.busy.get_untracked().unwrap_or(false)
    }
    fn say(&self, note: impl Into<String>, error: impl Into<String>) {
        self.note.set(note.into());
        self.error.set(error.into());
    }
}

#[derive(Clone, Copy)]
struct State {
    status: Part<Status>,
    cast: Part<CastView>,
    app: Part<AppView>,
    meta: Part<MetadataSettings>,
    subs: Part<SubtitleSettings>,
    devices: Part<Vec<PairedDevice>>,
    update: Part<()>,
}

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let root = w::column(ui).fill_width().child(h1(ui, "Settings"));
    if ctx.is_remote() {
        // A phone, tablet or TV keeps its own look and shares playback choices.
        return root.child(look_card(ui, ctx)).child(playback_card(ui, ctx)).child(muted(
            ui,
            "Add-ons, casting, subtitles services and the rest of Portal's settings change on the PC running Portal.",
        ));
    }
    let s = State {
        status: Part::new(),
        cast: Part::new(),
        app: Part::new(),
        meta: Part::new(),
        subs: Part::new(),
        devices: Part::new(),
        update: Part::new(),
    };
    let ready = Signal::new(false);
    let failed = Signal::new(String::new());
    load(ctx, s, ready, failed);
    let (u, c, r2) = (ui.clone(), ctx.clone(), root.clone());
    let root = root.bind(move || {
        let (ok, err) = (ready.get(), failed.get());
        untrack(|| {
            if !err.is_empty() {
                let _ = r2.clone().child(w::text(&u, err).variant("error"));
            } else if ok {
                paint(&u, &c, s, &r2);
            }
        });
    });
    root
}

/// main.ts `renderServer`: the engine's status and the update first, then every card's data
/// at once, and the page once all have answered.
fn load(ctx: &Ctx, s: State, ready: Signal<bool>, failed: Signal<String>) {
    let c = ctx.clone();
    let this_pc = ctx.me.get_untracked().ok().flatten().is_some_and(|m| m.this_pc);
    ctx.api.run(api::update(), &Cancel::new(), {
        let c = ctx.clone();
        move |out| {
            if let Ok(info) = out {
                c.update.set(Some(info));
            }
        }
    });
    ctx.api.run(api::status(), &Cancel::new(), move |out| {
        let status = match out {
            Ok(st) => st,
            Err(e) => return failed.set(shown_detail(&e)),
        };
        s.status.data.set(Some(status));
        let left = Rc::new(Cell::new(if this_pc { 5 } else { 4 }));
        let done = move || {
            left.set(left.get() - 1);
            if left.get() == 0 {
                ready.set(true);
            }
        };
        let d = Rc::new(done);
        let d1 = d.clone();
        c.api.run(api::cast_info(), &Cancel::new(), move |out| {
            match out {
                Ok(b) => s.cast.data.set(Some(CastView::from(&b, None))),
                Err(e) => s.cast.error.set(shown_detail(&e)),
            }
            d1();
        });
        let d2 = d.clone();
        c.api.run(api::metadata(), &Cancel::new(), move |out| {
            match out {
                Ok(m) => s.meta.data.set(Some(m)),
                Err(e) => s.meta.error.set(shown_detail(&e)),
            }
            d2();
        });
        let d3 = d.clone();
        c.api.run(api::subtitle_settings(), &Cancel::new(), move |out| {
            match out {
                Ok(v) => s.subs.data.set(Some(clean_subs(v))),
                Err(e) => s.subs.error.set(shown_detail(&e)),
            }
            d3();
        });
        let d4 = d.clone();
        c.api.run(api::app_info(), &Cancel::new(), move |out| {
            match out {
                Ok(a) => s.app.data.set(Some(a)),
                Err(e) => s.app.error.set(shown_detail(&e)),
            }
            d4();
        });
        if this_pc {
            refresh_devices(&c, s, Some(d.clone()));
        }
    });
}

fn clean_subs(mut v: SubtitleSettings) -> SubtitleSettings {
    v.languages.retain(|c| !c.trim().is_empty());
    v
}

fn refresh_devices(ctx: &Ctx, s: State, then: Option<Rc<dyn Fn()>>) {
    ctx.api.run(api::devices(), &Cancel::new(), move |out| {
        match out {
            Ok(list) => {
                s.devices.data.set(Some(list));
                s.devices.error.set(String::new());
            }
            Err(e) => s.devices.error.set(shown_detail(&e)),
        }
        if let Some(t) = then {
            t();
        }
    });
}

fn paint(ui: &Ui, ctx: &Ctx, s: State, root: &El) {
    let this_pc = ctx.me.get_untracked().ok().flatten().is_some_and(|m| m.this_pc);
    let look = look_card(ui, ctx);
    let playback = playback_card(ui, ctx);
    let app = app_card(ui, ctx, s);
    let cast = cast_card(ui, ctx, s);
    let devices = this_pc.then(|| devices_card(ui, ctx, s));
    let subs = subs_card(ui, ctx, s);
    let media = media_card(ui, ctx, s);
    let engine = engine_card(ui, ctx, s);
    let update = update_card(ui, ctx, s);
    let mut jumps = vec![("Appearance", look.clone()), ("Playback", playback.clone()), ("Portal", app.clone()), ("Cast", cast.clone())];
    if let Some(d) = &devices {
        jumps.push(("Devices", d.clone()));
    }
    jumps.extend([("Subtitles", subs.clone()), ("Media info", media.clone()), ("Kodi", engine.clone()), ("Updates", update.clone())]);
    let mut r = root.clone().child(jump_bar(ui, ctx, jumps)).child(look).child(playback).child(app).child(cast);
    if let Some(d) = devices {
        r = r.child(d);
    }
    let _ = r.child(subs).child(media).child(engine).child(update);
}

// ---- Pieces ----

/// `.server-card`: a raised card with its `h2`.
fn card(ui: &Ui, title: &str) -> El {
    ui.el("p_server_card")
        .column()
        .fill_width()
        .layout(|l| {
            l.margin.top = 8.8;
            l.margin.bottom = 8.8;
        })
        .semantics(Semantics::new(Role::Region).label(title.to_owned()))
        .child(w::heading(ui, 2, title.to_owned()).layout(|l| l.margin.bottom = 9.6))
}

/// A muted line (`p.muted`): under a heading or a row it is a little smaller and spaced.
fn muted(ui: &Ui, text: impl Into<String>) -> El {
    w::text(ui, text.into()).variant("muted")
}

/// A muted line right under the card's `h2` (their margins collapse in CSS).
fn intro(ui: &Ui, text: impl Into<String>) -> El {
    ui.el("p_server_note").text(text.into()).layout(|l| l.margin.bottom = 8.0)
}

fn note(ui: &Ui, text: impl Into<String>) -> El {
    ui.el("p_server_note").text(text.into()).layout(|l| {
        l.margin.top = 5.6;
        l.margin.bottom = 8.0;
    })
}

/// A line that shows while `text` says something: an error (`.error`, `role=alert`) or a
/// status note (muted, `role=status`).
fn said(ui: &Ui, text: Signal<String>, error: bool) -> El {
    let el = w::text(ui, move || text.get()).variant(if error { "error" } else { "muted" });
    let el = el.semantics(move || {
        Semantics::new(if error { Role::Alert } else { Role::Status }).label(text.get()).live(if error { Live::Assertive } else { Live::Polite })
    });
    when(el, move || !text.with(String::is_empty))
}

/// Shows `el` only while `on` says so.
fn when(el: El, on: impl Fn() -> bool + 'static) -> El {
    let e2 = el.clone();
    el.bind(move || {
        let v = on();
        let _ = e2.clone().hidden(!v);
    })
}

/// `.row` inside a card; `sep` draws the hairline over a row that follows another.
fn row(ui: &Ui, sep: bool) -> El {
    let el = ui.el(if sep { "p_server_row_sep" } else { "p_server_row" });
    let el = if sep {
        // ENGINE GAP: no per-side border (`border-top`) in StylePatch; the hairline is
        // painted at the row's top edge in the line color at 75%.
        el.layout(|l| l.margin.top = 2.4).paint(|cx| {
            let c = cx.theme.colors.line.unwrap_or(cx.theme.colors.muted);
            cx.list.fill_rect(Rect::new(0.0, 0.0, cx.size.width, 1.0), Brush::Solid(mix(c, 0.75, Color::TRANSPARENT)));
        })
    } else {
        el
    };
    el.row()
        .align(Align::Center)
        .fill_width()
        .layout(|l| {
            l.wrap = true;
            l.gap = (9.6, 9.6);
        })
}

/// An On/Off row: the label, and Portal's switch at the right.
fn switch_row(
    ui: &Ui,
    label: &str,
    sep: bool,
    on: impl Fn() -> bool + Clone + 'static,
    disabled: impl Fn() -> bool + 'static,
    press: impl Fn() + 'static,
) -> El {
    let (on1, on2) = (on.clone(), on);
    let label_s = label.to_owned();
    let track = ui
        .el("p_switch")
        .width(44.0)
        .height(24.0)
        .layout(|l| l.shrink = 0.0)
        .paint(|cx| {
            // The knob: 1.05rem, 0.2rem from the start; on, 1.2rem further.
            let d = 16.8;
            let x = if cx.states.contains(States::CHECKED) { 3.2 + 19.2 } else { 3.2 } - 1.0;
            let y = (cx.size.height - d) / 2.0;
            cx.list.fill_rounded(Rect::new(x, y, x + d, y + d), CornerRadii::all(d / 2.0), Brush::Solid(cx.style.foreground));
        })
        .disabled(disabled)
        .on_click(press);
    let t2 = track.clone();
    let track = track.bind(move || t2.state(States::CHECKED, on1())).semantics(move || {
        Semantics::new(Role::Switch).label(label_s.clone()).checked(on2()).action(Action::Click)
    });
    row(ui, sep)
        .layout(|l| l.wrap = false)
        .justify(Justify::SpaceBetween)
        .child(w::text(ui, label.to_owned()).layout(|l| l.min_width = Length::Px(0.0)).semantics(Semantics::new(Role::Label)))
        .child(track)
}

/// A labelled drop-down at the right of its label (`.select-row`). `current` is the choice
/// to show (followed), `pick` runs when the person chooses another.
fn select_row(
    ui: &Ui,
    label: &str,
    sep: bool,
    options: Vec<String>,
    current: impl Fn() -> usize + 'static,
    pick: impl Fn(usize) + 'static,
) -> El {
    let shown = Signal::new(untrack(&current));
    let sel = w::select(ui, label.to_owned(), options, shown).variant("settings").justify(Justify::SpaceBetween).layout(|l| {
        l.max_width = Length::Percent(0.6);
        l.width = Length::Px(240.0);
        l.shrink = 1.0;
        l.min_width = Length::Px(0.0);
    });
    let sel = sel.bind(move || shown.set_if_changed(current()));
    let last = Rc::new(Cell::new(shown.get_untracked().unwrap_or(0)));
    let sel = sel.bind(move || {
        let i = shown.get();
        if last.replace(i) != i {
            untrack(|| pick(i));
        }
    });
    row(ui, sep)
        .layout(|l| l.wrap = false)
        .justify(Justify::SpaceBetween)
        .child(w::text(ui, label.to_owned()).layout(|l| l.min_width = Length::Px(0.0)))
        .child(sel)
}

/// A field with its button(s) (`.search`).
fn field_row(ui: &Ui) -> El {
    ui.el("p_server_field").row().align(Align::Center).fill_width().layout(|l| {
        l.wrap = true;
        l.gap = (8.0, 8.0);
        l.margin.top = 4.8;
        l.margin.bottom = 5.6;
    })
}

fn input(field: &TextField) -> El {
    field.el().variant("portal_search").layout(|l| {
        l.grow = 1.0;
        l.shrink = 1.0;
        l.width = Length::Px(224.0);
        l.min_width = Length::Px(0.0);
        // `.search input { max-width: 28rem }`
        l.max_width = Length::Px(448.0);
    })
}

/// The "Key saved" / "No key saved" line (`.key-saved` in gold with a tick).
fn key_state(ui: &Ui, saved: impl Fn() -> bool + Clone + 'static, yes: &str, no: &str) -> El {
    let s2 = saved.clone();
    let yes_el = when(ui.el("p_key_saved").text(format!("\u{2713}\u{a0}{yes}")), saved);
    let no_el = when(ui.el("p_server_small").text(no.to_owned()), move || !s2());
    w::column(ui).child(yes_el).child(no_el)
}

/// A key form (`.media-key`): label, a password field with Save and (once saved) Remove,
/// the saved state and a hint. `save` gets the trimmed key; an empty one is not sent.
#[allow(clippy::too_many_arguments)]
fn key_form(
    ui: &Ui,
    label: &str,
    name: &str,
    saved: impl Fn() -> bool + Clone + 'static,
    busy: Signal<bool>,
    hint: &str,
    empty: impl Fn() + 'static,
    save: impl Fn(String, TextField) + 'static,
    remove: impl Fn() + 'static,
) -> El {
    let field = w::secret_input(ui, label.to_owned());
    let s1 = saved.clone();
    let name = name.to_owned();
    let f2 = field.clone();
    let submit = Rc::new(move || {
        let v = f2.value().trim().to_owned();
        if v.is_empty() {
            empty();
        } else {
            save(v, f2.clone());
        }
    });
    let s2 = submit.clone();
    let field = field.on_submit(move || s2());
    let f3 = field.clone();
    let _ = field.el().bind(move || {
        let p = if s1() { "Paste a new key".to_owned() } else { format!("Paste {name}") };
        let _ = f3.clone().placeholder(p);
    });
    let s3 = saved.clone();
    let row = field_row(ui)
        .child(input(&field))
        .child(button(ui, "Save", move || submit()).disabled(move || busy.get()))
        .child(when(button(ui, "Remove", remove).disabled(move || busy.get()), move || s3()));
    ui.el("p_media_key")
        .column()
        .fill_width()
        .layout(|l| l.margin.top = 14.4)
        .child(ui.el("p_media_label").layout(|l| l.margin.bottom = 1.6).text(label.to_owned()))
        .child(row)
        .child(key_state(ui, saved, "Key saved", "No key saved"))
        .child(ui.el("p_server_small").text(hint.to_owned()))
}

// ---- Appearance and Playback ----

fn look_card(ui: &Ui, ctx: &Ctx) -> El {
    let (t, f) = (ctx.theme, ctx.fonts);
    let themes: Vec<String> = theme::MENU.iter().map(|c| c.name.to_owned()).collect();
    let faces: Vec<String> = FontId::ALL.iter().map(|f| f.name().to_owned()).collect();
    let at = |id: FontId| FontId::ALL.iter().position(|x| *x == id).unwrap_or(0);
    let app = ctx.app.clone();
    let a2 = app.clone();
    let pick_font = move |text: bool, i: usize| {
        let id = FontId::ALL[i.min(FontId::ALL.len() - 1)];
        if id == FontId::System {
            crate::fonts::load_system(&a2);
        }
        f.update(|v| if text { v.text = id } else { v.headings = id });
    };
    let p2 = pick_font.clone();
    let sample = ui
        .el("p_font_sample")
        .column()
        .fill_width()
        .layout(|l| {
            l.margin.top = 9.6;
            l.margin.bottom = 9.6;
        })
        .child(ui.el("p_font_sample_head").text("Continue watching").layout(|l| l.margin.bottom = 4.0))
        .child(ui.el("p_font_sample_text").text("Season 2 · Episode 5 — 42 min left. Àéîõü Ωμέγα Привет 0123456789"));
    card(ui, "Appearance")
        .child(intro(ui, "Applies to this browser only, so each screen can look its own way."))
        .child(select_row(
            ui,
            "Theme",
            false,
            themes,
            move || theme::MENU.iter().position(|c| c.id == t.get()).unwrap_or(0),
            move |i| t.set(theme::MENU[i.min(theme::MENU.len() - 1)].id.to_owned()),
        ))
        .child(select_row(ui, "Text font", true, faces.clone(), move || at(f.get().text), move |i| pick_font(true, i)))
        .child(select_row(ui, "Heading font", true, faces, move || at(f.get().headings), move |i| p2(false, i)))
        .child(sample)
        .child(row(ui, false).child(button(ui, "Use Portal's fonts", move || f.set(Fonts::default()))))
}

fn playback_card(ui: &Ui, ctx: &Ctx) -> El {
    let p = prefs::signal();
    let (c1, c2, c3) = (ctx.clone(), ctx.clone(), ctx.clone());
    card(ui, "Playback")
        .child(intro(ui, "Shared by every screen that opens Portal, and by the TV. The player's own controls change the same choices."))
        .child(select_row(
            ui,
            "Quality",
            false,
            QUALITIES.iter().map(|(_, n)| (*n).to_owned()).collect(),
            move || QUALITIES.iter().position(|(q, _)| *q == p.with(|p| p.quality)).unwrap_or(0),
            move |i| prefs::set(&c1, Patch { quality: Some(QUALITIES[i.min(5)].0), ..Patch::default() }),
        ))
        .child(switch_row(
            ui,
            "Turn subtitles on when a video starts",
            true,
            move || p.with(|p| p.subtitles != SubtitleMode::Off),
            || false,
            move || {
                let off = prefs::now().subtitles == SubtitleMode::Off;
                prefs::set(&c2, Patch { subtitles: Some(if off { SubtitleMode::Auto } else { SubtitleMode::Off }), ..Patch::default() });
            },
        ))
        .child(switch_row(
            ui,
            "Play the next episode automatically",
            true,
            move || p.with(|p| p.autoplay),
            || false,
            move || prefs::set(&c3, Patch { autoplay: Some(!prefs::now().autoplay), ..Patch::default() }),
        ))
        .child(note(ui, "Quality applies from the next video you start. Subtitle languages are under Subtitles."))
}

// ---- Portal on this PC ----

fn card_name(card: &str) -> Option<&'static str> {
    match card {
        "nvidia" => Some("NVIDIA"),
        "intel" => Some("Intel"),
        "amd" => Some("AMD"),
        _ => None,
    }
}

fn app_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let a = s.app;
    let hardware = move || a.data.with(|v| v.as_ref().is_some_and(|v| card_name(&v.encoder.card).is_some() && v.encoder.hardware));
    let has_card = move || a.data.with(|v| v.as_ref().is_some_and(|v| card_name(&v.encoder.card).is_some()));
    let auto_on = move || a.data.with(|v| v.as_ref().is_some_and(|v| v.autostart == Some(true)));
    let auto_known = move || a.data.with(|v| v.as_ref().is_some_and(|v| v.autostart.is_some()));
    let tray = move || a.data.with(|v| v.as_ref().is_some_and(|v| v.tray));
    let no_tray = move || a.data.with(|v| v.as_ref().is_some_and(|v| !v.tray));
    let encoding = move || {
        a.data.with(|v| {
            let Some(v) = v else { return String::new() };
            match card_name(&v.encoder.card) {
                Some(c) if v.encoder.hardware => format!("Video is encoded by the {c} graphics card. A change applies from the next video you start."),
                Some(_) => "Video is encoded by the processor. A change applies from the next video you start.".into(),
                None => "Video is encoded by the processor: Portal found no graphics card it can use.".into(),
            }
        })
    };
    let (c1, c2) = (ctx.clone(), ctx.clone());
    let enc_text = Signal::new(String::new());
    let enc = note(ui, "");
    let e2 = enc.clone();
    let enc = enc.bind(move || {
        let t = encoding();
        enc_text.set_if_changed(t.clone());
        let _ = e2.clone().text(remedy_ui::view::RichText::plain(t));
    });
    card(ui, "Portal on this PC")
        .child(switch_row(
            ui,
            "Encode video on the graphics card",
            false,
            hardware,
            move || a.busy.get() || !has_card(),
            move || save_app(&c1, s, true, !hardware()),
        ))
        .child(when(enc, move || !enc_text.with(String::is_empty)))
        .child(switch_row(
            ui,
            "Start with Windows",
            // The encoding line sits between, so no hairline (`.row + .row`).
            false,
            auto_on,
            move || a.busy.get() || !tray() || !auto_known(),
            move || save_app(&c2, s, false, !auto_on()),
        ))
        .child(when(note(ui, "Start with Windows needs Portal's tray icon, which is not installed here."), no_tray))
        .child(said(ui, a.error, true))
}

fn save_app(ctx: &Ctx, s: State, encoder: bool, on: bool) {
    let a = s.app;
    if a.busy_now() {
        return;
    }
    a.busy.set(true);
    let call = if encoder { api::app_encoder(on) } else { api::app_autostart(on) };
    ctx.api.run(call, &Cancel::new(), move |out| {
        match out {
            Ok(v) => {
                a.data.set(Some(v));
                a.error.set(String::new());
            }
            Err(e) => a.error.set(shown_detail(&e)),
        }
        a.busy.set(false);
    });
}

// ---- Cast ----

fn cast_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let c = s.cast;
    let on = move || c.data.with(|v| v.as_ref().is_some_and(|v| v.lan));
    let href = move || c.data.with(|v| v.as_ref().map(|v| cast_page_url(&v.url)).unwrap_or_default());
    let (h1_, h2_) = (href, href);
    let app = ctx.app.clone();
    let link = ui
        .el("p_link")
        .text(move || remedy_ui::view::RichText::plain(h1_()))
        .on_click(move || {
            let u = href();
            if !u.is_empty() {
                app.open_url(u, |_| {});
            }
        })
        .semantics(move || Semantics::new(Role::Link).label(h2_()).url(h2_()).action(Action::Click));
    let address = when(
        row(ui, false)
            .layout(|l| {
                l.margin.top = 8.8;
                l.gap = (4.0, 4.0);
            })
            .child(w::text(ui, "Open this address on a TV:"))
            .child(link),
        move || !href().is_empty(),
    );
    let waiting = when(note(ui, "The cast link appears when Portal reports an address."), move || on() && href().is_empty());
    let c1 = ctx.clone();
    let restart = when(
        row(ui, true)
            .justify(Justify::SpaceBetween)
            .child(muted(ui, CAST_RESTART_TEXT))
            .child(button(ui, "Restart Portal", move || restart_for_cast(&c1, s)).disabled(move || c.busy.get())),
        move || c.data.with(|v| v.as_ref().is_some_and(|v| v.restart)),
    );
    let c2 = ctx.clone();
    card(ui, "Cast")
        .child(switch_row(ui, "Allow TVs on this network", false, on, || false, move || toggle_cast(&c2, s)))
        .child(address)
        .child(waiting)
        .child(restart)
        .child(said(ui, c.error, true))
}

fn toggle_cast(ctx: &Ctx, s: State) {
    let c = s.cast;
    if c.busy_now() {
        return;
    }
    let next = !c.data.get_untracked().ok().flatten().is_some_and(|v| v.lan);
    c.busy.set(true);
    ctx.api.run(api::set_cast_lan(next), &Cancel::new(), move |out| {
        match out {
            Ok(b) => {
                let before = c.data.get_untracked().ok().flatten();
                c.data.set(Some(CastView::from(&b, before.as_ref())));
                c.error.set(String::new());
            }
            Err(e) => c.error.set(shown_detail(&e)),
        }
        c.busy.set(false);
    });
}

fn restart_for_cast(ctx: &Ctx, s: State) {
    let c = s.cast;
    if c.busy_now() {
        return;
    }
    let ctx = ctx.clone();
    confirm(&ctx.activity.ui(), RESTART_TEXT, "Restart", move || {
        c.busy.set(true);
        ctx.activity.notice("Restarting Portal…");
        let c2 = ctx.clone();
        ctx.api.run(api::app_restart(), &Cancel::new(), move |out| match out {
            Ok(_) => reload_when_back(&c2, 0, false),
            Err(e) => {
                c2.activity.notice("");
                c.error.set(shown_detail(&e));
                c.busy.set(false);
            }
        });
    });
}

/// appmenu.ts `reloadWhenBack`: every 0.7 s for up to 90 s, waits for the server to go away
/// and answer again (or to answer after 10 s), then loads the page fresh.
fn reload_when_back(ctx: &Ctx, ticks: u32, gone: bool) {
    const TICK_MS: u64 = 700;
    let until = 90_000 / TICK_MS as u32;
    let c = ctx.clone();
    ctx.activity.ui().after(std::time::Duration::from_millis(TICK_MS), move || {
        let ticks = ticks + 1;
        if ticks >= until {
            return c.app.reload();
        }
        let c2 = c.clone();
        c.api.run(api::status(), &Cancel::new(), move |out| {
            let up = out.is_ok();
            let gone = gone || !up;
            if up && (gone || ticks * TICK_MS as u32 > 10_000) {
                c2.app.reload();
            } else {
                reload_when_back(&c2, ticks, gone);
            }
        });
    });
}

// ---- Devices ----

fn seen_at(ui: &Ui, ms: i64) -> String {
    if ms <= 0 {
        return String::new();
    }
    // shortcut: the offset now, not at that instant (a date across a daylight saving change
    // is an hour off); the engine gives no zone history.
    let local = ms.div_euclid(1000) + i64::from(ui.local_time().utc_offset);
    let date = remedy_ui::i18n::Date::from_days_since_epoch(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    let time = remedy_ui::i18n::Time::new((secs / 3600) as u8, ((secs % 3600) / 60) as u8).unwrap_or_default();
    remedy_ui::i18n::CalendarSymbols::for_locale(&crate::screens::info::page_locale(ui)).date_time(date, time)
}

fn devices_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let d = s.devices;
    let field = w::text_input(ui, "Number shown on the device").placeholder("123 456").numeric();
    let (c1, f1) = (ctx.clone(), field.clone());
    let submit = Rc::new(move || {
        let code: String = f1.value().chars().filter(char::is_ascii_digit).collect();
        if code.len() != 6 {
            d.say("", "Type the 6-digit number the device shows.");
            f1.el().focus();
            return;
        }
        pair_device(&c1, s, code, f1.clone());
    });
    let s2 = submit.clone();
    let field = field.on_submit(move || s2());
    let form = ui
        .el("p_media_key")
        .column()
        .fill_width()
        .layout(|l| l.margin.top = 14.4)
        .semantics(Semantics::new(Role::Form).label("Pair a device"))
        .child(ui.el("p_media_label").layout(|l| l.margin.bottom = 1.6).text("Number shown on the device"))
        .child(field_row(ui).child(input(&field).layout(|l| l.max_width = Length::Px(448.0))).child(button(ui, "Pair", move || submit()).disabled(move || d.busy.get())))
        .child(said(ui, d.note, false))
        .child(said(ui, d.error, true));
    let list = w::column(ui).fill_width();
    let (l2, u, c) = (list.clone(), ui.clone(), ctx.clone());
    let list = list.bind(move || {
        let rows = d.data.get().unwrap_or_default();
        untrack(|| {
            l2.clear();
            if rows.is_empty() {
                let _ = l2.clone().child(muted(&u, "No devices are paired.").layout(|l| l.margin.top = 16.0));
                return;
            }
            for (i, dev) in rows.into_iter().enumerate() {
                let seen = seen_at(&u, dev.last_seen);
                let facts = [dev.address.clone(), if seen.is_empty() { seen } else { format!("last seen {seen}") }]
                    .into_iter()
                    .filter(|f| !f.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let (c2, dv) = (c.clone(), dev.clone());
                let name = w::row(&u)
                    .gap(4.0)
                    .layout(|l| {
                        l.wrap = true;
                        l.min_width = Length::Px(0.0);
                        l.shrink = 1.0;
                    })
                    .child(w::text(&u, dev.name.clone()))
                    .child(muted(&u, facts));
                let r = row(&u, i > 0)
                    .justify(Justify::SpaceBetween)
                    .child(name)
                    .child(button(&u, "Remove", move || remove_device(&c2, s, dv.clone())).disabled(move || d.busy.get()));
                let _ = l2.clone().child(r);
            }
        });
    });
    card(ui, "Devices")
        .child(intro(ui, "A phone, tablet or TV that opens Portal shows a number. Type it here to let that device in."))
        .child(form)
        .child(list)
}

fn pair_device(ctx: &Ctx, s: State, code: String, field: TextField) {
    let d = s.devices;
    if d.busy_now() {
        return;
    }
    d.busy.set(true);
    let c = ctx.clone();
    ctx.api.run(api::pair_approve(&code), &Cancel::new(), move |out| {
        match out {
            Ok(r) => {
                d.say(format!("Paired: {}. It opens Portal in a moment.", r.name), "");
                field.set_value("");
                let f2 = field.clone();
                refresh_devices(
                    &c,
                    s,
                    Some(Rc::new(move || {
                        d.busy.set(false);
                        f2.el().focus();
                    })),
                );
                return;
            }
            Err(e) => d.say("", shown_detail(&e)),
        }
        d.busy.set(false);
        field.el().focus();
    });
}

fn remove_device(ctx: &Ctx, s: State, dev: PairedDevice) {
    let d = s.devices;
    if d.busy_now() {
        return;
    }
    d.busy.set(true);
    let c = ctx.clone();
    ctx.api.run(api::remove_device(&dev.id), &Cancel::new(), move |out| match out {
        Ok(_) => {
            d.say(format!("Removed {}. It needs a new number to come back.", dev.name), "");
            refresh_devices(&c, s, Some(Rc::new(move || d.busy.set(false))));
        }
        Err(e) => {
            d.error.set(shown_detail(&e));
            d.busy.set(false);
        }
    });
}

// ---- Subtitles ----

#[derive(Clone, Copy)]
enum KeySource {
    SubDl,
    SubSource,
}

impl KeySource {
    fn name(self) -> &'static str {
        match self {
            KeySource::SubDl => "SubDL",
            KeySource::SubSource => "SubSource",
        }
    }
    fn hint(self) -> &'static str {
        match self {
            KeySource::SubDl => "A free key comes from your account on subdl.com → API.",
            KeySource::SubSource => "A free key comes from your profile on subsource.net → API key.",
        }
    }
    fn saved(self, s: &SubtitleSettings) -> bool {
        match self {
            KeySource::SubDl => s.subdl,
            KeySource::SubSource => s.subsource,
        }
    }
    fn put(self, value: Option<String>) -> SubtitleSettingsPut {
        match self {
            KeySource::SubDl => SubtitleSettingsPut { subdl_key: Some(value), ..Default::default() },
            KeySource::SubSource => SubtitleSettingsPut { subsource_key: Some(value), ..Default::default() },
        }
    }
}

fn subs_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let p = s.subs;
    let has = move || p.data.with(Option::is_some);
    let flag = move |f: fn(&SubtitleSettings) -> bool| move || p.data.with(|v| v.as_ref().is_some_and(f));
    let unavailable = when(muted(ui, "Subtitle settings are not available."), move || !has());

    let intro_text = move || {
        p.data.with(|v| {
            let Some(v) = v else { return String::new() };
            let mut services = vec!["Gestdown", "TVsubtitles", "Subtitle Cat"];
            if v.opensubtitles {
                services.push("OpenSubtitles");
            }
            for k in [KeySource::SubDl, KeySource::SubSource] {
                if k.saved(v) {
                    services.push(k.name());
                }
            }
            format!(
                "The CC button finds subtitles for what you play. Gestdown and TVsubtitles cover TV shows, Subtitle Cat covers films and TV shows (machine translations are marked), and OpenSubtitles, SubDL and SubSource join in once their keys are saved. Searching sends the title, year, season and episode to {}.",
                spoken_list(&services)
            )
        })
    };
    let intro = self::intro(ui, "");
    let i2 = intro.clone();
    let intro = intro.bind(move || {
        let _ = i2.clone().text(remedy_ui::view::RichText::plain(intro_text()));
    });

    // Preferred languages: chips with a remove button, and a menu to add one.
    let locales = ctx.app.preferred_locales();
    let langs = w::column(ui).fill_width().layout(|l| {
        l.margin.top = 9.6;
        l.margin.bottom = 3.2;
        l.gap = (6.4, 9.6);
    });
    let (l2, u, c) = (langs.clone(), ui.clone(), ctx.clone());
    let langs = langs.bind(move || {
        let Some(v) = p.data.get() else { return };
        let busy = p.busy.get();
        let locales = locales.clone();
        untrack(|| {
            l2.clear();
            let saved = !v.languages.is_empty();
            let current = if saved { v.languages.clone() } else { browser_languages(&locales) };
            let title = format!("Preferred languages{}", if saved { "" } else { " (from your browser)" });
            let mut chips = ui_row(&u, 5.6).semantics(Semantics::new(Role::List).label(title.clone()));
            for (i, code) in current.iter().enumerate() {
                let name = language_name(code);
                let (c2, cur, n2) = (c.clone(), current.clone(), name.clone());
                let x = button(&u, "×", move || {
                    let rest: Vec<String> = cur.iter().enumerate().filter(|(at, _)| *at != i).map(|(_, c)| c.clone()).collect();
                    save_subs(&c2, s, SubtitleSettingsPut { languages: Some(rest), ..Default::default() }, format!("{n2} removed."), Vec::new());
                })
                .variant("chip_x")
                .disabled(busy)
                .semantics(Semantics::new(Role::Button).label(format!("Remove {name}")).action(Action::Click));
                chips = chips.child(
                    u.el("p_lang_chip")
                        .row()
                        .align(Align::Center)
                        .gap(3.2)
                        .semantics(Semantics::new(Role::ListItem).label(name.clone()))
                        .child(w::text(&u, name))
                        .child(x),
                );
            }
            let full = current.len() >= language_limit();
            let addable: Vec<(&str, &str)> = LANGUAGES.iter().copied().filter(|(code, _)| !current.iter().any(|c| c == code)).collect();
            let first = if full { format!("Up to {} languages", language_limit()) } else { "Add a language…".to_owned() };
            let options: Vec<String> = std::iter::once(first).chain(addable.iter().map(|(_, n)| (*n).to_owned())).collect();
            let chosen = Signal::new(0usize);
            let (c3, cur) = (c.clone(), current.clone());
            let add = w::select(&u, "Add a language", options, chosen).variant("settings").disabled(full || busy);
            let add = add.bind(move || {
                let i = chosen.get();
                if i == 0 {
                    return;
                }
                let Some((code, name)) = addable.get(i - 1).copied() else { return };
                let mut next = cur.clone();
                next.push(code.to_owned());
                next.truncate(language_limit());
                untrack(|| save_subs(&c3, s, SubtitleSettingsPut { languages: Some(next), ..Default::default() }, format!("{name} added."), Vec::new()));
            });
            let _ = l2.clone().child(muted(&u, title)).child(ui_row(&u, 9.6).child(chips).child(add));
        });
    });

    let hi = flag(|v| v.hearing_impaired);
    let c_hi = ctx.clone();
    let hearing = switch_row(ui, "Prefer subtitles for the hearing impaired", false, hi, move || p.busy.get(), move || {
        save_subs(&c_hi, s, SubtitleSettingsPut { hearing_impaired: Some(!hi()), ..Default::default() }, String::new(), Vec::new());
    });

    let os = flag(|v| v.opensubtitles);
    let (c_k1, c_k2) = (ctx.clone(), ctx.clone());
    let os_key = key_form(
        ui,
        "OpenSubtitles API key (optional, adds OpenSubtitles results)",
        "an OpenSubtitles API key",
        os,
        p.busy,
        "A free key comes from your account on opensubtitles.com → API consumers.",
        move || p.say("", "Enter an OpenSubtitles API key."),
        move |key, field| {
            save_subs(&c_k1, s, SubtitleSettingsPut { opensubtitles_key: Some(Some(key)), ..Default::default() }, "OpenSubtitles key saved.".into(), vec![field])
        },
        move || {
            let put = SubtitleSettingsPut {
                opensubtitles_key: Some(None),
                opensubtitles_username: Some(None),
                opensubtitles_password: Some(None),
                ..Default::default()
            };
            save_subs(&c_k2, s, put, "OpenSubtitles key removed.".into(), Vec::new());
        },
    );

    let signed = flag(|v| v.opensubtitles_user);
    let user = w::text_input(ui, "Username").placeholder("Username");
    let pass = w::secret_input(ui, "OpenSubtitles password").placeholder("Password");
    let (c_l, u1, p1) = (ctx.clone(), user.clone(), pass.clone());
    let login = Rc::new(move || {
        let (name, word) = (u1.value().trim().to_owned(), p1.value());
        if name.is_empty() || word.is_empty() {
            p.say("", "Enter your OpenSubtitles username and password.");
            if name.is_empty() { u1.el().focus() } else { p1.el().focus() }
            return;
        }
        let put = SubtitleSettingsPut { opensubtitles_username: Some(Some(name)), opensubtitles_password: Some(Some(word)), ..Default::default() };
        // The old page repaints the form empty once the sign-in is saved.
        save_subs(&c_l, s, put, "Signed in to OpenSubtitles.".into(), vec![u1.clone(), p1.clone()]);
    });
    let (l1, l2) = (login.clone(), login.clone());
    let user = user.on_submit(move || l1());
    let pass = pass.on_submit(move || l2());
    let c_so = ctx.clone();
    let s3 = signed;
    let login_form = ui
        .el("p_media_key")
        .column()
        .fill_width()
        .layout(|l| l.margin.top = 14.4)
        .child(ui.el("p_media_label").layout(|l| l.margin.bottom = 1.6).text("OpenSubtitles account (optional, for more downloads a day)"))
        .child(
            field_row(ui)
                .child(input(&user))
                .child(input(&pass))
                .child(button(ui, "Sign in", move || login()).disabled(move || p.busy.get()))
                .child(when(
                    button(ui, "Sign out", move || {
                        let put = SubtitleSettingsPut { opensubtitles_username: Some(None), opensubtitles_password: Some(None), ..Default::default() };
                        save_subs(&c_so, s, put, "Signed out of OpenSubtitles.".into(), Vec::new());
                    })
                    .disabled(move || p.busy.get()),
                    move || s3(),
                )),
        )
        .child(key_state(ui, signed, "Signed in", "Not signed in"));
    let login_form = when(login_form, os);

    let mut more = w::column(ui).fill_width();
    for k in [KeySource::SubDl, KeySource::SubSource] {
        let (c1, c2) = (ctx.clone(), ctx.clone());
        let saved = move || p.data.with(|v| v.as_ref().is_some_and(|v| k.saved(v)));
        more = more.child(key_form(
            ui,
            &format!("{} API key (optional, adds {} results)", k.name(), k.name()),
            &format!("a {} API key", k.name()),
            saved,
            p.busy,
            k.hint(),
            move || p.say("", format!("Enter a {} API key.", k.name())),
            move |key, field| save_subs(&c1, s, k.put(Some(key)), format!("{} key saved.", k.name()), vec![field]),
            move || save_subs(&c2, s, k.put(None), format!("{} key removed.", k.name()), Vec::new()),
        ));
    }

    let body = w::column(ui)
        .fill_width()
        .child(intro)
        .child(langs)
        .child(hearing)
        .child(os_key)
        .child(login_form)
        .child(more)
        .child(said(ui, p.note, false));
    card(ui, "Subtitles").child(unavailable).child(when(body, has)).child(said(ui, p.error, true))
}

fn ui_row(ui: &Ui, gap: f32) -> El {
    w::row(ui).align(Align::Center).layout(move |l| {
        l.wrap = true;
        l.gap = (gap, gap);
    })
}

/// main.ts `saveSubs`: one save at a time; a saved key's field is emptied.
fn save_subs(ctx: &Ctx, s: State, put: SubtitleSettingsPut, note: String, fields: Vec<TextField>) {
    let p = s.subs;
    if p.busy_now() {
        return;
    }
    p.busy.set(true);
    ctx.api.run(api::set_subtitle_settings(&put), &Cancel::new(), move |out| {
        match out {
            Ok(v) => {
                p.data.set(Some(clean_subs(v)));
                p.say(note, "");
                for f in &fields {
                    f.set_value("");
                }
            }
            Err(e) => {
                p.say("", shown_detail(&e));
                if let Some(f) = fields.first() {
                    f.el().focus();
                }
            }
        }
        p.busy.set(false);
    });
}

// ---- Media info ----

fn media_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let m = s.meta;
    let has = move || m.data.with(Option::is_some);
    let online = move || m.data.with(|v| v.as_ref().is_some_and(|v| v.online));
    let tmdb = move || m.data.with(|v| v.as_ref().is_some_and(|v| v.tmdb));
    let (c1, c2, c3) = (ctx.clone(), ctx.clone(), ctx.clone());
    let body = w::column(ui)
        .fill_width()
        .child(switch_row(ui, "Look up extra info online (TVmaze, Wikipedia)", false, online, move || m.busy.get(), move || {
            save_meta(&c1, s, MetadataPut { online: Some(!online()), tmdb_key: None }, None, None);
        }))
        .child(note(ui, "Looking up info sends the title and year of what you open to TVmaze and Wikipedia, and to TMDB when a key is saved."))
        .child(key_form(
            ui,
            "TMDB API key (optional, for richer info)",
            "a TMDB API key",
            tmdb,
            m.busy,
            "A free key comes from your account on themoviedb.org → Settings → API.",
            move || m.say("", "Enter a TMDB API key."),
            move |key, field| save_meta(&c2, s, MetadataPut { online: None, tmdb_key: Some(Some(key)) }, Some("TMDB key saved."), Some(field)),
            move || save_meta(&c3, s, MetadataPut { online: None, tmdb_key: Some(None) }, Some("TMDB key removed."), None),
        ))
        .child(said(ui, m.note, false));
    card(ui, "Media info")
        .child(when(muted(ui, "Media info settings are not available."), move || !has()))
        .child(when(body, has))
        .child(said(ui, m.error, true))
}

fn save_meta(ctx: &Ctx, s: State, put: MetadataPut, note: Option<&'static str>, field: Option<TextField>) {
    let m = s.meta;
    if m.busy_now() || m.data.get_untracked().ok().flatten().is_none() {
        return;
    }
    m.busy.set(true);
    ctx.api.run(api::set_metadata(&put), &Cancel::new(), move |out| {
        match out {
            Ok(v) => {
                m.data.set(Some(v));
                m.say(note.unwrap_or_default(), "");
                if let Some(f) = field {
                    f.set_value("");
                }
            }
            Err(e) => {
                // A failed toggle keeps the note; a failed key save clears it (main.ts).
                if note.is_some() {
                    m.note.set(String::new());
                }
                m.error.set(shown_detail(&e));
                if let Some(f) = field {
                    f.el().focus();
                }
            }
        }
        m.busy.set(false);
    });
}

// ---- Kodi engine ----

fn engine_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let st = s.status;
    let fact = move |f: fn(&Status) -> String| move || st.data.with(|v| v.as_ref().map(f).unwrap_or_default());
    let facts: [(&str, fn(&Status) -> String); 6] = [
        // "Running", "Starting", "Stopped": the state capitalized, as main.ts shows it.
        ("State", |s| format!("{:?}", s.state)),
        ("Version", |s| s.version.clone().unwrap_or_default()),
        ("Process ID", |s| s.pid.map(|p| p.to_string()).unwrap_or_default()),
        ("Started by Portal", |s| if s.started_by_us { "Yes" } else { "No" }.into()),
        ("Window", |s| if s.window_visible { "Shown" } else { "Hidden" }.into()),
        ("Web server", |s| if s.webserver { "On" } else { "Off" }.into()),
    ];
    let mut card = card(ui, "Kodi engine");
    for (i, (label, f)) in facts.into_iter().enumerate() {
        card = card.child(row(ui, i > 0).justify(Justify::SpaceBetween).child(muted(ui, label)).child(w::text(ui, fact(f))));
    }
    let problem = Signal::new(String::new());
    let p2 = problem;
    let card = card.bind(move || {
        let e = st.data.with(|v| v.as_ref().and_then(|v| v.error.clone())).unwrap_or_default();
        let msg = st.error.get();
        p2.set_if_changed(if e.is_empty() { msg } else { e });
    });
    let visible = move || st.data.with(|v| v.as_ref().is_some_and(|v| v.window_visible));
    let (c1, c2, c3) = (ctx.clone(), ctx.clone(), ctx.clone());
    let window_label = move || if visible() { "Hide Kodi window".to_owned() } else { "Show Kodi window".to_owned() };
    card.child(when(w::text(ui, move || problem.get()).variant("error"), move || !problem.with(String::is_empty)))
        .child(
            row(ui, false)
                .padding(Insets { top: 9.6, bottom: 0.0, start: 0.0, end: 0.0 })
                .child(button(ui, window_label, move || engine_call(&c1, s, None)))
                .child(button(ui, "Restart Kodi engine", move || engine_call(&c2, s, Some(ServerAction::Restart))))
                .child(button(ui, "Close Kodi engine", move || {
                    let c = c3.clone();
                    if st.busy_now() {
                        return;
                    }
                    confirm(&c3.activity.ui(), "Close the Kodi engine? Anything playing stops.", "Close", move || {
                        engine_call(&c, s, Some(ServerAction::Quit))
                    });
                })),
        )
}

/// main.ts `serverCall`: one engine action at a time; a restart or quit shows in the
/// activity panel.
fn engine_call(ctx: &Ctx, s: State, action: Option<ServerAction>) {
    let st = s.status;
    if st.busy_now() {
        return;
    }
    st.busy.set(true);
    let ticket = action.map(|a| ctx.activity.begin(Owner::Page, describe_server(a), 0));
    let visible = st.data.get_untracked().ok().flatten().is_some_and(|v| v.window_visible);
    let call = match action {
        None => api::engine_window(!visible),
        Some(ServerAction::Restart) => api::engine_restart().no_timeout(),
        Some(ServerAction::Quit) => api::engine_quit().no_timeout(),
    };
    let c = ctx.clone();
    ctx.api.run(call, &Cancel::new(), move |out: Result<Status, ApiError>| {
        match out {
            Ok(next) => {
                st.data.set(Some(next));
                st.error.set(String::new());
            }
            Err(e) => st.error.set(shown_detail(&e)),
        }
        st.busy.set(false);
        if let Some(t) = ticket {
            c.activity.end(t);
        }
    });
}

// ---- Updates ----

fn percent(info: &UpdateInfo) -> Option<u32> {
    info.progress.filter(|p| p.is_finite()).map(|p| (p.clamp(0.0, 1.0) * 100.0).round() as u32)
}

/// main.ts `updateLine`.
pub fn update_line(info: &UpdateInfo) -> String {
    let latest = info.latest.clone().unwrap_or_default();
    match info.state {
        UpdateState::Checking => "Checking for updates…".into(),
        UpdateState::Downloading => match percent(info) {
            Some(p) => format!("Downloading Portal {latest} ({p}%)"),
            None => format!("Downloading Portal {latest}…"),
        },
        UpdateState::Ready => format!("Portal {latest} is downloaded."),
        UpdateState::Installing => format!("Installing Portal {latest}. Portal will close and reopen."),
        _ if info.available => format!("Portal {latest} is available."),
        _ if info.state == UpdateState::Failed || info.error.is_some() => String::new(),
        _ if info.latest.is_some() => "Portal is up to date.".into(),
        _ => "No update found.".into(),
    }
}

fn release_link(url: Option<&str>) -> Option<String> {
    url.filter(|u| u.starts_with("https://github.com/")).map(str::to_owned)
}

fn update_card(ui: &Ui, ctx: &Ctx, s: State) -> El {
    let info = ctx.update;
    let up = s.update;
    let has = move || info.with(Option::is_some);
    let working = move || {
        up.busy.get() || info.with(|i| i.as_ref().is_some_and(|i| matches!(i.state, UpdateState::Checking | UpdateState::Downloading | UpdateState::Installing)))
    };
    let get = move |f: fn(&UpdateInfo) -> String| move || info.with(|i| i.as_ref().map(f).unwrap_or_default());
    let version = row(ui, false).justify(Justify::SpaceBetween).child(muted(ui, "Portal version")).child(w::text(ui, get(|i| i.current.clone())));
    let line_text = get(update_line);
    let lt = line_text;
    let line = when(w::text(ui, line_text).layout(|l| l.margin = Insets { top: 8.0, bottom: 8.0, start: 0.0, end: 0.0 }), move || !lt().is_empty());
    let meter = ui
        .el("p_activity_meter")
        .fill_width()
        .height(8.0)
        .layout(|l| {
            l.margin.top = 6.4;
            l.margin.bottom = 9.6;
        })
        .paint(move |cx| {
            let pc = info.with(|i| i.as_ref().and_then(percent));
            let (w, h) = (cx.size.width, cx.size.height);
            let r = CornerRadii::all(h / 2.0);
            cx.list.fill_rounded(Rect::new(0.0, 0.0, w, h), r, Brush::Solid(cx.style.background));
            let (x0, x1) = match pc {
                Some(p) => (0.0, w * p as f32 / 100.0),
                None if cx.reduced_motion => (0.0, w * 0.34),
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
                cx.list.fill_rounded(Rect::new(x0, 0.0, x1, h), r, Brush::Solid(cx.style.foreground));
                cx.list.pop_clip();
            }
        })
        .semantics(move || {
            let s = Semantics::new(Role::Progress).label("Download");
            match info.with(|i| i.as_ref().and_then(percent)) {
                Some(p) => s.numeric(f64::from(p), 0.0, 100.0),
                None => s,
            }
        });
    let meter = when(meter, move || info.with(|i| i.as_ref().is_some_and(|i| i.state == UpdateState::Downloading)));
    let problem = move || {
        let mine = up.error.get();
        if !mine.is_empty() {
            return (mine, true);
        }
        info.with(|i| i.as_ref().map(|i| (i.error.clone().unwrap_or_default(), i.state == UpdateState::Failed)).unwrap_or_default())
    };
    let (pr1, pr2) = (problem, problem);
    let err_el = when(w::text(ui, move || pr1().0).variant("error"), move || pr2().1 && !pr2().0.is_empty());
    let (pr3, pr4) = (problem, problem);
    let note_el = when(w::text(ui, move || pr3().0).variant("muted"), move || !pr4().1 && !pr4().0.is_empty());
    let (c1, c2) = (ctx.clone(), ctx.clone());
    let install_label = get(|i| format!("Install {}", i.latest.clone().unwrap_or_default()));
    let can_install = move || info.with(|i| i.as_ref().is_some_and(|i| i.available && i.latest.is_some()));
    let install = when(button(ui, install_label, move || install_update(&c2, s)).variant("gold").disabled(working), can_install);
    let link = move || info.with(|i| i.as_ref().and_then(|i| release_link(i.release_url.as_deref())));
    let app = ctx.app.clone();
    let notes = when(
        button(ui, "Release notes", move || {
            if let Some(u) = link() {
                app.open_url(u, |_| {});
            }
        })
        .semantics(Semantics::new(Role::Link).label("Release notes").action(Action::Click)),
        move || link().is_some(),
    );
    let body = w::column(ui)
        .fill_width()
        .child(version)
        .child(line)
        .child(meter)
        .child(err_el)
        .child(note_el)
        .child(
            row(ui, false)
                .padding(Insets { top: 9.6, bottom: 0.0, start: 0.0, end: 0.0 })
                .child(button(ui, "Check for updates", move || check_update(&c1, s)).disabled(working))
                .child(install)
                .child(notes),
        );
    card(ui, "Updates").child(when(muted(ui, "Update status is not available."), move || !has())).child(when(body, has))
}

fn check_update(ctx: &Ctx, s: State) {
    let up = s.update;
    if up.busy_now() {
        return;
    }
    up.busy.set(true);
    up.error.set(String::new());
    ctx.update.update(|i| {
        if let Some(i) = i {
            i.state = UpdateState::Checking;
            i.error = None;
        }
    });
    let c = ctx.clone();
    ctx.api.run(api::check_update().no_timeout(), &Cancel::new(), move |out| match out {
        Ok(info) => {
            note_update(&c, info);
            up.busy.set(false);
        }
        Err(e) => {
            up.error.set(shown_detail(&e));
            let c2 = c.clone();
            c.api.run(api::update(), &Cancel::new(), move |out| {
                if let Ok(info) = out {
                    note_update(&c2, info);
                }
                up.busy.set(false);
            });
        }
    });
}

/// main.ts `noteUpdate`: a download or install under way is followed to its end.
fn note_update(ctx: &Ctx, info: UpdateInfo) {
    if matches!(info.state, UpdateState::Downloading | UpdateState::Installing) {
        crate::shell::follow_update(ctx, info.current.clone());
    }
    ctx.update.set(Some(info));
}

fn install_update(ctx: &Ctx, s: State) {
    let up = s.update;
    let Some(info) = ctx.update.get_untracked().ok().flatten() else { return };
    if up.busy_now() || !info.available {
        return;
    }
    let c = ctx.clone();
    confirm(&ctx.activity.ui(), "Portal will close, update, and reopen. Anything playing will stop.", "Install", move || {
        up.busy.set(true);
        up.error.set(String::new());
        let mut dl = info.clone();
        dl.state = UpdateState::Downloading;
        dl.progress = None;
        dl.error = None;
        crate::shell::follow_update(&c, info.current.clone());
        c.update.set(Some(dl));
        let (c2, before) = (c.clone(), info.clone());
        c.api.run(api::install_update().no_timeout(), &Cancel::new(), move |out| {
            match out {
                Ok(info) => note_update(&c2, info),
                Err(e) if e.status.is_some() => {
                    up.error.set(shown_detail(&e));
                    let c3 = c2.clone();
                    c2.api.run(api::update(), &Cancel::new(), move |out| {
                        if let Ok(info) = out {
                            note_update(&c3, info);
                        }
                    });
                }
                Err(_) => {
                    // No answer at all: the installer is most likely stopping Portal already.
                    let mut i = before;
                    i.state = UpdateState::Installing;
                    i.progress = None;
                    i.error = None;
                    c2.update.set(Some(i));
                }
            }
            up.busy.set(false);
        });
    });
}

// ---- Jump bar ----

fn jump_bar(ui: &Ui, ctx: &Ctx, targets: Vec<(&'static str, El)>) -> El {
    let (u, c) = (ui.clone(), ctx.clone());
    let holder = w::column(ui).fill_width().layout(|l| l.margin.bottom = 9.6);
    let h2 = holder.clone();
    let holder = holder.bind(move || {
        let narrow = c.width.get() <= 720.0;
        untrack(|| {
            h2.clear();
            let mut bar = w::row(&u).gap(6.4).semantics(Semantics::new(Role::Navigation).label("Sections"));
            for (name, target) in &targets {
                let (t, cx) = (target.clone(), c.clone());
                bar = bar.child(
                    button(&u, *name, move || {
                        let head = cx.head_h.get_untracked().unwrap_or(0.0);
                        let size = t.bounds().map(|b| b.size()).unwrap_or_default();
                        let tall = cx.height.get_untracked().unwrap_or(800.0);
                        t.scroll_rect_into_view(Rect::from_origin_size(Point::new(0.0, -head), Size::new(size.width, tall)));
                    })
                    .variant("jump"),
                );
            }
            let el = if narrow {
                w::column(&u).fill_width().scroll().scroll_x().child(bar)
            } else {
                bar.layout(|l| {
                    l.wrap = true;
                    l.gap = (6.4, 6.4);
                })
            };
            let _ = h2.clone().child(el);
        });
    });
    holder
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule(
        "p_server_card",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .padding(Insets { top: 13.6, bottom: 13.6, start: 14.4, end: 14.4 }),
    );
    s.rule("p_server_card", None, States::HOVER, StylePatch::default().border(mix(k.gold, 0.48, k.line)));
    // `.server-card > .row + .row`: room under the hairline [`row`] draws.
    s.rule("p_server_row_sep", None, States::NONE, StylePatch::default().padding(Insets { top: 7.2, bottom: 0.0, start: 0.0, end: 0.0 }));
    s.rule("p_server_note", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.72));
    s.rule("p_server_small", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.72));
    s.rule("p_key_saved", None, States::NONE, StylePatch::default().foreground(k.gold).text_size(14.72));
    s.rule("p_media_label", None, States::NONE, StylePatch::default().foreground(k.muted));
    // The switch (`.server-card button[role="switch"]`): a well track with a muted knob; on, a
    // gold ring on a gold-tinted track with a gold knob.
    s.rule(
        "p_switch",
        None,
        States::NONE,
        StylePatch::default().background(k.well).border(k.line).border_width(1.0).radius(99.0).foreground(k.muted),
    );
    s.rule("p_switch", None, States::CHECKED, StylePatch::default().background(mix(k.gold, 0.28, k.well)).border(k.gold).foreground(k.gold));
    s.rule("p_switch", None, States::DISABLED, StylePatch::default().opacity(0.45));
    // `select`: the field's well, a hairline, 8 px corners; gold under the pointer.
    s.rule(
        "button",
        Some("settings"),
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .weight(400.0)
            .padding(Insets { top: 6.72, bottom: 6.72, start: 12.0, end: 12.0 }),
    );
    s.rule("button", Some("settings"), States::HOVER, StylePatch::default().border(k.gold));
    s.rule("button", Some("settings"), States::DISABLED, StylePatch::default().opacity(0.45));
    s.rule(
        "p_font_sample",
        None,
        States::NONE,
        // ENGINE GAP: `border-style: dashed` (styles.css `.font-sample`) has no StylePatch
        // equivalent; drawn solid until it does.
        StylePatch::default()
            .background(k.well)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(Insets { top: 11.2, bottom: 11.2, start: 13.6, end: 13.6 }),
    );
    s.rule(
        "p_font_sample_head",
        None,
        States::NONE,
        StylePatch::default().heading(true).text_size(24.0).line_height(LineHeight::Relative(1.3)),
    );
    // `.jump-bar button`: smaller pills.
    s.rule(
        "p_button",
        Some("jump"),
        States::NONE,
        StylePatch::default().radius(999.0).padding(Insets { top: 4.48, bottom: 4.48, start: 11.2, end: 11.2 }).text_size(14.72),
    );
    // `.update-card button.update-install`: gold.
    s.rule("p_button", Some("gold"), States::NONE, StylePatch::default().border(k.gold).foreground(k.gold));
    // `.lang-chip` and its round remove button.
    s.rule(
        "p_lang_chip",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .border(k.line)
            .border_width(1.0)
            .radius(99.0)
            .padding(Insets { top: 3.2, bottom: 3.2, start: 10.4, end: 4.8 }),
    );
    s.rule(
        "p_button",
        Some("chip_x"),
        States::NONE,
        StylePatch::default().radius(12.0).padding(Insets { top: 2.0, bottom: 2.0, start: 7.0, end: 7.0 }).foreground(k.muted).line_height(LineHeight::Relative(1.0)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_as_subtitles_ts_reads_them() {
        let l = |v: &[&str]| browser_languages(&v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>());
        assert_eq!(l(&["en-US", "en", "pt-BR", "zh-Hant-TW", "de", "fr"]), ["en", "pt-BR", "zh-TW", "de"]);
        assert_eq!(l(&[]), ["en"]);
        assert_eq!(primary_language("ENG"), "en");
        assert_eq!(primary_language("pt_PT"), "pt");
    }

    #[test]
    fn words_as_main_ts_says_them() {
        assert_eq!(spoken_list(&["A", "B"]), "A and B");
        assert_eq!(spoken_list(&["A", "B", "C"]), "A, B and C");
        assert_eq!(cast_page_url("http://192.168.0.2:8490/"), "http://192.168.0.2:8490/#/cast");
        assert_eq!(cast_page_url("http://x/#/cast"), "http://x/#/cast");
        assert_eq!(cast_page_url("ftp://x"), "");
        let mut i = UpdateInfo {
            current: "1.0".into(),
            latest: Some("1.1".into()),
            available: true,
            state: UpdateState::Idle,
            progress: None,
            error: None,
            release_url: None,
        };
        assert_eq!(update_line(&i), "Portal 1.1 is available.");
        i.state = UpdateState::Downloading;
        i.progress = Some(0.426);
        assert_eq!(update_line(&i), "Downloading Portal 1.1 (43%)");
        assert_eq!(release_link(Some("https://evil.com/")), None);
    }
}
