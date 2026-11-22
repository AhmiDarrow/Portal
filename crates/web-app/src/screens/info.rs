//! The Info dialog (info.ts, all of it): what the page knows shows at once, then what an
//! online lookup adds. A modal over any page: Tab stays inside, Esc and a press outside close
//! it, keys never reach the player's shortcuts, focus goes back to what opened it. The close
//! button stays at the top while it scrolls; a refresh after the lookup keeps the focused
//! control and the scroll; a picture that cannot load is taken out (the fanart, its scrim
//! and the hero's room for it; a poster; a cast photo, leaving the initials).
//!
//! ```ignore
//! info::open(ui, ctx, InfoRequest { seed, art, save: Some(save), play: None, opener: Some(button) });
//! ```

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use portal_api::{MediaInfo, MediaKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets, Vec2};
use remedy_ui::image::{image_ext, Fit, ImageOptions, ImageState, Source};
use remedy_ui::input::{Key, PointerPhase};
use remedy_ui::layout::{Align, Justify, Length, Position, Repeat, Track};
use remedy_ui::reactive::Cancel;
use remedy_ui::style::{LetterSpacing, Shadow, States, StickyInsets, StylePatch, StyleSheet, TextTransform};
use remedy_ui::view::{RichText, TextStylePatch};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, List};
use crate::theme::{gradient, mix, Look};
use crate::widgets::art::art_url;
use crate::widgets::glyphs::{glyph, Glyph};
use crate::widgets::saved::{self, SaveTitle};
use crate::widgets::titles::episode_code;
use crate::Ctx;

/// info.ts `CAST_LIMIT`: people shown, at most. [`set_cast_limit`] changes it.
pub const CAST_LIMIT: usize = 12;

thread_local! {
    static LIMIT: Cell<usize> = const { Cell::new(CAST_LIMIT) };
    static OPEN: RefCell<Option<Open>> = const { RefCell::new(None) };
    static SESSION: Cell<u64> = const { Cell::new(0) };
}

/// How many cast members the dialog lists (12 unless changed).
pub fn set_cast_limit(n: usize) {
    LIMIT.with(|l| l.set(n));
}

/// What opens the dialog (info.ts `InfoRequest`).
#[derive(Clone)]
pub struct InfoRequest {
    /// What the page already knows; shown at once and sent to the lookup.
    pub seed: MediaInfo,
    /// Server art for the title, used when the info has no poster (an episode's still).
    pub art: Option<String>,
    /// What Watchlist and Bookmark save; an episode saves its show.
    pub save: Option<SaveTitle>,
    /// A first action such as Play: its words and what it does.
    pub play: Option<(String, Rc<dyn Fn()>)>,
    /// Gets focus back when the dialog closes.
    pub opener: Option<El>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lookup {
    Loading,
    Done,
    Failed,
}

struct Open {
    layer: El,
    dialog: El,
    content: El,
    status: El,
    sources: El,
    foot: El,
    opener: Option<El>,
    req: InfoRequest,
    info: MediaInfo,
    lookup: Lookup,
    /// Controls by key (info.ts `data-key`), so a refresh can focus the same one again.
    keys: HashMap<&'static str, El>,
    links: Vec<(String, El)>,
    ui: Ui,
    ctx: Ctx,
}

/// True while the dialog is open (info.ts `infoOpen`).
pub fn is_open() -> bool {
    OPEN.with(|o| o.borrow().is_some())
}

/// The dialog's layer is built on open; there is nothing to show under a page.
pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    w::column(ui)
}

fn present(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::String(s) => !s.trim().is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        _ => true,
    }
}

/// Fields `extra` has replace the ones `base` had; empty ones do not (info.ts `merge`).
pub fn merge(base: &MediaInfo, extra: &MediaInfo) -> MediaInfo {
    let mut out = serde_json::to_value(base).unwrap_or_default();
    if let (Some(o), serde_json::Value::Object(e)) = (out.as_object_mut(), serde_json::to_value(extra).unwrap_or_default()) {
        for (k, v) in e {
            if present(&v) {
                o.insert(k, v);
            }
        }
        o.retain(|_, v| present(v));
    }
    serde_json::from_value(out).unwrap_or_else(|_| base.clone())
}

/// Opens the dialog for `req`, or shows `req` in it when it is open already.
pub fn open(ui: &Ui, ctx: &Ctx, req: InfoRequest) {
    let mine = SESSION.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let seed = merge(&MediaInfo::default(), &req.seed);
    let fresh = !is_open();
    if fresh {
        let opener = req.opener.clone().or_else(|| ui.tree().focused().map(|f| ui.element(f)));
        build(ui, ctx, req.clone(), seed.clone(), opener);
    } else {
        OPEN.with(|o| {
            if let Some(o) = o.borrow_mut().as_mut() {
                if req.opener.is_some() {
                    o.opener = req.opener.clone();
                }
                o.req = req.clone();
                o.info = seed.clone();
                o.lookup = Lookup::Loading;
            }
        });
    }
    paint(false);
    with_open(|o| {
        o.ui.set_scroll_offset(o.dialog.id(), Vec2::ZERO);
        o.dialog.focus();
    });
    ctx.api.run(api::info(&seed), &Cancel::new(), move |out| {
        if SESSION.with(Cell::get) != mine {
            return;
        }
        let still = OPEN.with(|o| {
            let mut b = o.borrow_mut();
            let Some(o) = b.as_mut() else { return false };
            match out {
                Ok(found) => {
                    o.info = merge(&o.info, &found);
                    o.lookup = Lookup::Done;
                }
                Err(_) => o.lookup = Lookup::Failed,
            }
            true
        });
        if still {
            paint(true);
        }
    });
}

/// Closes the dialog; focus goes back to what opened it unless `restore_focus` is false.
pub fn close(restore_focus: bool) {
    let Some(o) = OPEN.with(|o| o.borrow_mut().take()) else { return };
    SESSION.with(|s| s.set(s.get() + 1));
    o.layer.remove();
    if let (true, Some(back)) = (restore_focus, o.opener) {
        back.focus();
    }
}

fn with_open<R>(f: impl FnOnce(&Open) -> R) -> Option<R> {
    OPEN.with(|o| o.borrow().as_ref().map(f))
}

fn narrow(ctx: &Ctx) -> bool {
    ctx.width.get_untracked().unwrap_or(1280.0) <= 720.0
}

fn build(ui: &Ui, ctx: &Ctx, req: InfoRequest, info: MediaInfo, opener: Option<El>) {
    let phone = narrow(ctx);
    let close_btn = ui
        .el("p_info_close")
        .row()
        .align(Align::Center)
        .justify(Justify::Center)
        .width(36.0)
        .height(36.0)
        .layout(|l| {
            l.shrink = 0.0;
            l.margin = Insets { top: 10.4, bottom: 0.0, start: 0.0, end: 10.4 };
        })
        .child(glyph(ui, Glyph::Close, 20.8))
        .on_click(|| close(true))
        .semantics(Semantics::new(Role::Button).label("Close").action(Action::Click));
    w::tooltip(&close_btn, "Close (Esc)");
    // .info-top: sticky, no height of its own, so the close button floats over the hero.
    let top = ui
        .el("p_info_top")
        .row()
        .justify(Justify::End)
        .height(0.0)
        .layout(|l| l.shrink = 0.0)
        .sticky(StickyInsets { top: Some(0.0), ..StickyInsets::default() })
        .child(close_btn.clone());
    let content = ui.el("p_info_content").column().fill_width().layout(|l| l.shrink = 0.0);
    let status = ui.el("p_info_status").row().align(Align::Center).gap(7.2).semantics(Semantics::new(Role::Status));
    let sources = ui.el("p_info_sources");
    let foot = ui
        .el("p_info_foot")
        .row()
        .layout(move |l| {
            l.wrap = true;
            l.gap = (16.0, 4.8);
            l.shrink = 0.0;
        })
        .padding(if phone {
            Insets { top: 11.2, bottom: 14.4, start: 16.0, end: 16.0 }
        } else {
            Insets { top: 12.0, bottom: 16.0, start: 27.2, end: 27.2 }
        })
        // .info-status is order 2 with margin-left auto: the sources line comes first.
        .child(sources.clone())
        .child(w::spacer(ui))
        .child(status.clone());
    // .info-foot's border-top.
    let rule = ui.el("p_info_rule").height(1.0).fill_width().layout(|l| l.shrink = 0.0);
    let foot = ui.el("p_info_foot_box").column().fill_width().layout(|l| l.shrink = 0.0).child(rule.clone()).child(foot);
    let spacer = w::spacer(ui);
    let dialog = ui
        .el("p_info_dialog")
        .variant(if phone { "phone" } else { "" })
        .column()
        .scroll()
        .focusable(true)
        .semantics(Semantics::new(Role::Dialog).label(info.title.clone().unwrap_or_default()))
        .child(top)
        .child(content.clone())
        .child(spacer)
        .child(foot.clone());
    let layer = ui
        .el("p_info_layer")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        .focus_scope()
        .layout(move |l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
            l.width = Length::Percent(1.0);
            l.height = Length::Percent(1.0);
            l.align_items = if phone { Align::Stretch } else { Align::Center };
        })
        .padding(if phone { Insets::ZERO } else { Insets::all(24.0) })
        .child(dialog.clone());
    // max-height: min(92vh, 56rem); width: min(54rem, 100%); a full-screen sheet on a phone.
    let d2 = dialog.clone();
    let layer = layer.on_resize(move |size| {
        let _ = d2.clone().layout(move |l| {
            if phone {
                l.width = Length::Percent(1.0);
                l.height = Length::Percent(1.0);
                l.max_height = Length::Auto;
            } else {
                l.width = Length::Px(864.0_f32.min(size.width - 48.0).max(0.0));
                l.max_height = Length::Px((size.height * 0.92).min(896.0));
            }
        });
    });
    // A press that starts and ends outside the dialog closes it.
    let down_outside = Rc::new(Cell::new(false));
    let (d3, d4) = (dialog.clone(), down_outside.clone());
    let layer = layer
        .on_pointer(move |e| {
            let outside = !d3.bounds().is_some_and(|b| b.contains(e.position));
            match e.phase {
                PointerPhase::Down => d4.set(outside),
                PointerPhase::Up => {
                    if outside && d4.replace(false) {
                        close(true);
                        return true;
                    }
                    d4.set(false);
                }
                _ => {}
            }
            false
        })
        // Esc closes; no key reaches the page's own shortcuts while it is open (Tab stays
        // inside through the focus scope).
        .on_key(|e| {
            if e.pressed && e.key == Key::Escape {
                close(true);
                return true;
            }
            e.key != Key::Tab
        });
    ui.overlay(layer.clone());
    // A new address closes it, leaving focus where the new page puts it.
    let route = ctx.router.signal();
    let first = Rc::new(Cell::new(true));
    let _ = layer.clone().bind(move || {
        route.get();
        if !first.replace(false) {
            remedy_ui::reactive::untrack(|| close(false));
        }
    });
    OPEN.with(|o| {
        *o.borrow_mut() = Some(Open {
            layer,
            dialog,
            content,
            status,
            sources,
            foot,
            opener,
            req,
            info,
            lookup: Lookup::Loading,
            keys: HashMap::new(),
            links: Vec::new(),
            ui: ui.clone(),
            ctx: ctx.clone(),
        });
    });
}

// ---- the words (info.ts helpers) ----

fn list(values: &[String], limit: usize) -> String {
    values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()).take(limit).collect::<Vec<_>>().join(", ")
}

/// More than one name, counting names an add-on joined with commas.
fn several(values: &[String]) -> bool {
    let named: Vec<&String> = values.iter().filter(|v| !v.trim().is_empty()).collect();
    named.len() > 1 || named.iter().any(|v| v.contains(", "))
}

/// "1h 52m", "45m", "2h".
pub fn runtime(minutes: Option<u32>) -> String {
    let Some(total) = minutes.filter(|m| *m > 0) else { return String::new() };
    let (h, m) = (total / 60, total % 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

fn year_of(info: &MediaInfo) -> String {
    if let Some(y) = info.year.filter(|y| *y > 1800) {
        return y.to_string();
    }
    let p = info.premiered.as_deref().unwrap_or("");
    if p.len() >= 4 && p.as_bytes()[..4].iter().all(u8::is_ascii_digit) { p[..4].to_owned() } else { String::new() }
}

/// The page's locale for dates and numbers: the viewer's own English, else plain English
/// (html.ts `PAGE_LOCALE`; the page is in English).
pub fn page_locale(ui: &Ui) -> String {
    let own = ui.locale();
    if own.to_ascii_lowercase().split(['-', '_']).next() == Some("en") { own } else { "en".into() }
}

/// "Apr 4, 2005" from an add-on's `YYYY-MM-DD` date (main.ts `airedOn`), empty otherwise.
pub fn aired_on(ui: &Ui, value: Option<&str>) -> String {
    let Some(v) = value else { return String::new() };
    let b = v.as_bytes();
    let digits = |r: std::ops::Range<usize>| b.get(r.clone()).is_some_and(|s| s.iter().all(u8::is_ascii_digit));
    if !(digits(0..4) && b.get(4) == Some(&b'-') && digits(5..7) && b.get(7) == Some(&b'-') && digits(8..10)) {
        return String::new();
    }
    let n = |r: std::ops::Range<usize>| v[r].parse::<i32>().unwrap_or(0);
    let (y, m, d) = (n(0..4), n(5..7), n(8..10));
    // JS rolls an impossible day over (2005-02-30 is Mar 2); Date::new refuses it, and so
    // does a month out of range, which JS shows as Invalid Date (empty here).
    let Some(date) = remedy_ui::i18n::Date::new(y, m as u8, d as u8).or_else(|| roll_over(y, m, d)) else {
        return String::new();
    };
    remedy_ui::i18n::CalendarSymbols::for_locale(&page_locale(ui)).medium_date(date)
}

fn roll_over(y: i32, m: i32, d: i32) -> Option<remedy_ui::i18n::Date> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = remedy_ui::i18n::days_in_month(y, m as u8) as i32;
    let (y2, m2) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
    remedy_ui::i18n::Date::new(y2, m2 as u8, (d - days) as u8)
}

/// "1.2K votes", "12.3K votes": the page's English compact form with at most one digit
/// after the point (info.ts `votes`, `maximumFractionDigits: 1`); empty for none.
pub fn votes(ui: &Ui, count: Option<u64>) -> String {
    votes_in(&page_locale(ui), count)
}

/// [`votes`] in `locale`.
pub fn votes_in(locale: &str, count: Option<u64>) -> String {
    let Some(c) = count.filter(|c| *c > 0) else { return String::new() };
    let n = remedy_ui::i18n::NumberSymbols::for_locale(locale);
    let one = remedy_ui::i18n::CompactOptions::default().max_fraction_digits(1);
    let text = remedy_ui::i18n::Decimal::new(i128::from(c), 0).map(|d| n.compact_with(d, one)).unwrap_or_else(|| c.to_string());
    format!("{text} {}", if c == 1 { "vote" } else { "votes" })
}

/// The line over the title: "Show · S01E02", "Show", "Movie".
pub fn kicker(info: &MediaInfo) -> String {
    match info.kind {
        Some(MediaKind::Episode) => {
            let parts: Vec<String> = [info.show_title.as_deref().unwrap_or("").trim().to_owned(), episode_code(info.season, info.episode)]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect();
            if parts.is_empty() { "Episode".into() } else { parts.join(" · ") }
        }
        Some(MediaKind::Tv) => "Show".into(),
        Some(MediaKind::Movie) => "Movie".into(),
        None => String::new(),
    }
}

/// html.ts `webUrl`: only http(s) addresses become links.
pub fn web_url(value: Option<&str>) -> Option<String> {
    let t = value?.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower.strip_prefix("http://").or_else(|| lower.strip_prefix("https://"))?;
    // `new URL` needs a host.
    if rest.is_empty() || rest.starts_with(['/', '?', '#']) || t.contains(char::is_whitespace) {
        return None;
    }
    Some(t.to_owned())
}

/// The links row: the add-on's, then IMDb, TMDB and the trailer, one per label.
pub fn links(info: &MediaInfo) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |label: &str, url: Option<String>| {
        let clean = label.trim();
        let Some(url) = url else { return };
        if clean.is_empty() || out.iter().any(|(l, _)| l.to_lowercase() == clean.to_lowercase()) {
            return;
        }
        out.push((clean.to_owned(), url));
    };
    for l in &info.links {
        add(&l.label, web_url(Some(&l.url)));
    }
    let imdb = info.imdb_id.as_deref().filter(|id| {
        id.len() >= 7 && id.len() <= 14 && id.starts_with("tt") && id[2..].bytes().all(|b| b.is_ascii_digit())
    });
    if let Some(id) = imdb {
        add("IMDb", Some(format!("https://www.imdb.com/title/{id}/")));
    }
    let tmdb = info.tmdb_id.as_deref().filter(|id| (1..=12).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit()));
    if let Some(id) = tmdb {
        let kind = match info.kind {
            Some(MediaKind::Movie) => Some("movie"),
            Some(MediaKind::Tv) => Some("tv"),
            _ => None,
        };
        if let Some(kind) = kind {
            add("TMDB", Some(format!("https://www.themoviedb.org/{kind}/{id}")));
        }
    }
    add("Trailer", web_url(info.trailer.as_deref()));
    out
}

/// "JS" from "Jane Smith", "?" from nothing.
pub fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let first = parts.first().and_then(|p| p.chars().next());
    let last = if parts.len() > 1 { parts.last().and_then(|p| p.chars().next()) } else { None };
    let s: String = first.into_iter().chain(last).collect::<String>().to_uppercase();
    if s.is_empty() { "?".into() } else { s }
}

// ---- painting ----

fn text(ui: &Ui, widget: &'static str, s: impl Into<String>) -> El {
    ui.el(widget).text(RichText::plain(s.into()))
}

fn italic(ui: &Ui, widget: &'static str, s: &str) -> El {
    let mut p = TextStylePatch::default();
    p.italic = Some(true);
    ui.el(widget).text(RichText::new().styled(s, p))
}

fn margin(el: El, top: f32, bottom: f32) -> El {
    el.layout(move |l| {
        l.margin.top = top;
        l.margin.bottom = bottom;
    })
}

/// A picture that is taken out when it cannot load (info.ts `picture`).
fn picture(ui: &Ui, url: String, on_fail: impl Fn() + 'static) -> El {
    let fail = Rc::new(on_fail);
    let options = ImageOptions::new().placeholder(ui.el("p_info_wait")).on_state(move |s| {
        if let ImageState::Failed(_) = s {
            fail();
        }
    });
    image_ext(ui, Source::Url(url), "", Fit::Cover, options).layout(|l| {
        l.width = Length::Percent(1.0);
        l.height = Length::Percent(1.0);
    })
}

/// Rebuilds the dialog's content; keeps focus and scroll when it is a refresh.
fn paint(refresh: bool) {
    let Some((ui, ctx)) = with_open(|o| (o.ui.clone(), o.ctx.clone())) else { return };
    let (focus_key, scroll) = with_open(|o| {
        let focused = o.ui.tree().focused();
        let key = if refresh {
            o.keys.iter().find(|(_, el)| Some(el.id()) == focused).map(|(k, _)| *k).or_else(|| {
                o.links.iter().position(|(_, el)| Some(el.id()) == focused).map(|_| "link")
            })
        } else {
            None
        };
        let link = if refresh {
            o.links.iter().find(|(_, el)| Some(el.id()) == focused).map(|(l, _)| l.clone())
        } else {
            None
        };
        ((key, link), o.ui.scroll_offset(o.dialog.id()))
    })
    .unwrap_or(((None, None), Vec2::ZERO));
    let Some((req, info, lookup, content)) = with_open(|o| (o.req.clone(), o.info.clone(), o.lookup, o.content.clone())) else {
        return;
    };
    let phone = narrow(&ctx);
    content.clear();
    let mut keys: HashMap<&'static str, El> = HashMap::new();
    let mut found_links: Vec<(String, El)> = Vec::new();

    let title = Some(info.title.as_deref().unwrap_or("").trim())
        .filter(|t| !t.is_empty())
        .or(info.show_title.as_deref().map(str::trim).filter(|t| !t.is_empty()))
        .unwrap_or("Untitled")
        .to_owned();
    let fanart = art_url(info.fanart.as_deref());
    let has_fanart = fanart.is_some();
    // An episode shows its own still; the poster the add-on gives it is the season's or the show's.
    let still = if info.kind == Some(MediaKind::Episode) { art_url(req.art.as_deref()) } else { None };
    let is_still = still.is_some();
    let poster = still.or_else(|| art_url(info.poster.as_deref())).or_else(|| art_url(req.art.as_deref()));

    let hero = ui.el("p_info_hero").column().fill_width().layout(|l| l.position = Position::Relative);
    let head_pad = move |fanart: bool| {
        if phone {
            Insets { top: if fanart { 104.0 } else { 54.4 }, bottom: 0.0, start: 16.0, end: 16.0 }
        } else if fanart {
            Insets { top: 136.0, bottom: 0.0, start: 27.2, end: 27.2 }
        } else {
            Insets { top: 27.2, bottom: 0.0, start: 27.2, end: 57.6 }
        }
    };
    let head = ui
        .el("p_info_head")
        .row()
        .gap(if phone { 15.2 } else { 22.4 })
        .align(if fanart.is_some() { Align::End } else { Align::Start })
        .padding(head_pad(has_fanart));
    let mut hero = hero;
    if let Some(url) = fanart.clone() {
        let backdrop = ui
            .el("p_info_backdrop")
            .variant(if phone { "phone" } else { "" })
            .clip()
            .layout(move |l| {
                l.position = Position::Absolute;
                l.inset = Insets { top: 0.0, bottom: remedy_ui::layout::AUTO, start: 0.0, end: 0.0 };
                l.height = Length::Px(if phone { 208.0 } else { 272.0 });
            });
        let (b2, h2) = (backdrop.clone(), head.clone());
        // ENGINE GAP: `object-position: center 25%`; RemedyUI's Fit::Cover always centers
        // (remedy-ui-media-image/src/lib.rs `place`).
        let img = ui.el("p_info_backdrop_img").child(picture(&ui, url, move || {
            b2.remove();
            let _ = h2.clone().align(Align::Start).padding(head_pad(false));
        }));
        let scrim = ui.el("p_info_scrim");
        let fill = |el: El| {
            el.layout(|l| {
                l.position = Position::Absolute;
                l.inset = Insets::ZERO;
            })
        };
        hero = hero.child(backdrop.child(fill(img)).child(fill(scrim)));
    }
    let mut head = head;
    if let Some(url) = poster {
        let (pw, ratio) = match (phone, is_still) {
            (false, false) => (168.0, 2.0 / 3.0),
            (false, true) => (272.0, 16.0 / 9.0),
            (true, false) => (105.6, 2.0 / 3.0),
            (true, true) => (160.0, 16.0 / 9.0),
        };
        let frame = ui.el("p_info_poster").variant(if phone { "phone" } else { "" }).clip().width(pw).layout(move |l| {
            l.shrink = 0.0;
            l.aspect_ratio = Some(ratio);
        });
        let f2 = frame.clone();
        head = head.child(frame.child(picture(&ui, url, move || f2.remove())));
    }
    let mut heading = ui.el("p_info_heading").column().grow(1.0).layout(|l| {
        l.shrink = 1.0;
        l.min_width = Length::Px(0.0);
    });
    let kick = kicker(&info);
    if !kick.is_empty() {
        heading = heading.child(margin(text(&ui, "p_info_kicker", kick), 0.0, 4.8));
    }
    heading = heading.child(w::heading(&ui, 2, title.clone()).variant(if phone { "info_phone" } else { "info" }));
    if let Some(original) = info.original_title.as_deref().map(str::trim).filter(|o| !o.is_empty() && o.to_lowercase() != title.to_lowercase()) {
        heading = heading.child(margin(italic(&ui, "p_info_muted", original), 3.2, 0.0));
    }
    let facts: Vec<String> = [
        if info.kind == Some(MediaKind::Episode) {
            let a = aired_on(&ui, info.premiered.as_deref());
            if a.is_empty() { year_of(&info) } else { format!("Aired {a}") }
        } else {
            year_of(&info)
        },
        runtime(info.runtime_minutes),
        info.mpaa.as_deref().unwrap_or("").trim().to_owned(),
        list(&info.genres, 4),
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    if !facts.is_empty() {
        heading = heading.child(margin(text(&ui, "p_info_muted", facts.join(" · ")), 7.2, 0.0));
    }
    if let Some(r) = info.rating.filter(|r| r.is_finite() && *r > 0.0) {
        let score = format!("{:.1}", r.min(10.0));
        let count = votes(&ui, info.votes);
        let label = if count.is_empty() { format!("Rated {score} out of 10") } else { format!("Rated {score} out of 10, {count}") };
        let mut row = ui
            .el("p_info_rating")
            .row()
            .align(Align::Baseline)
            .gap(3.2)
            .layout(|l| {
                l.wrap = true;
                l.margin.top = 6.4;
            })
            .semantics(Semantics::new(Role::Label).label(label).merge_descendants())
            .child(text(&ui, "p_info_star", "★"))
            .child(text(&ui, "p_info_score", score))
            .child(text(&ui, "p_info_scale", "/10"));
        if !count.is_empty() {
            row = row.child(text(&ui, "p_info_scale", format!(" · {count}")));
        }
        heading = heading.child(row);
    }
    let mut actions = ui.el("p_info_actions").row().gap(8.0).layout(|l| {
        l.wrap = true;
        l.margin.top = 16.0;
    });
    let mut any = false;
    if let Some((label, run)) = req.play.clone() {
        let b = ui
            .el("p_info_play")
            .row()
            .align(Align::Center)
            .gap(6.4)
            .child(glyph(&ui, Glyph::Play, 17.6))
            .child(ui.el("p_pill_text").text(label.clone()).nowrap())
            .on_click(move || {
                // Focus goes back to what opened Info first, so closing the player returns there.
                close(true);
                run();
            })
            .semantics(Semantics::new(Role::Button).label(label).action(Action::Click));
        keys.insert("play", b.clone());
        actions = actions.child(b);
        any = true;
    }
    if let Some(save) = req.save.clone() {
        let watch = saved::save_button_for(&ui, &ctx, List::Watchlist, save.clone(), "pill");
        let book = saved::save_button_for(&ui, &ctx, List::Bookmarks, save, "pill");
        keys.insert("watchlist", watch.clone());
        keys.insert("bookmarks", book.clone());
        actions = actions.child(watch).child(book);
        any = true;
    }
    if any {
        heading = heading.child(actions);
    }
    let hero = hero.child(head.child(heading));

    let mut body = ui.el("p_info_body").column().fill_width().padding(if phone {
        Insets { top: 17.6, bottom: 4.8, start: 16.0, end: 16.0 }
    } else {
        Insets { top: 20.8, bottom: 6.4, start: 27.2, end: 27.2 }
    });
    if let Some(t) = info.tagline.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        body = body.child(margin(italic(&ui, "p_info_tagline", t), 0.0, 9.6));
    }
    match info.plot.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        // max-width: 68ch, a "0" in Remedy Sans at 16 px being 10.68 px wide.
        Some(p) => body = body.child(margin(text(&ui, "p_info_plot", p).layout(|l| l.max_width = Length::Px(68.0 * 10.68)), 0.0, 17.6)),
        None if lookup != Lookup::Loading => {
            body = body.child(margin(text(&ui, "p_info_plot_none", "No summary is available for this title."), 0.0, 17.6));
        }
        None => {}
    }
    let credits: Vec<(&str, String)> = [
        (if several(&info.directors) { "Directors" } else { "Director" }, list(&info.directors, 4)),
        (if several(&info.writers) { "Writers" } else { "Writer" }, list(&info.writers, 4)),
        (if several(&info.studios) { "Studios" } else { "Studio" }, list(&info.studios, 3)),
        (if several(&info.countries) { "Countries" } else { "Country" }, list(&info.countries, 3)),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .collect();
    if !credits.is_empty() {
        let mut dl = ui.el("p_info_credits").layout(|l| {
            l.display = remedy_ui::layout::Display::Grid;
            l.columns = vec![Track::MaxContent, Track::Fr(1.0)];
            l.gap = (17.6, 4.8);
            l.margin.bottom = 19.2;
        });
        for (label, value) in credits {
            dl = dl.child(text(&ui, "p_info_dt", label)).child(text(&ui, "p_info_dd", value));
        }
        body = body.child(dl);
    }
    let limit = LIMIT.with(Cell::get);
    let cast: Vec<_> = info.cast.iter().filter(|m| !m.name.trim().is_empty()).take(limit).cloned().collect();
    if !cast.is_empty() {
        let mut grid = ui.el("p_info_cast").semantics(Semantics::new(Role::List).label("Cast")).layout(move |l| {
            l.display = remedy_ui::layout::Display::Grid;
            l.columns = if phone {
                vec![Track::Repeat(Repeat::Count(2), vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0)))])]
            } else {
                vec![Track::Repeat(Repeat::AutoFill, vec![Track::MinMax(Box::new(Track::Px(200.0)), Box::new(Track::Fr(1.0)))])]
            };
            l.gap = if phone { (6.4, 6.4) } else { (8.0, 8.0) };
            l.margin.bottom = 19.2;
        });
        for m in cast {
            let side = if phone { 33.6 } else { 40.0 };
            let face = ui
                .el("p_info_face")
                .variant(if phone { "phone" } else { "" })
                .clip()
                .width(side)
                .height(side)
                .row()
                .align(Align::Center)
                .justify(Justify::Center)
                .layout(|l| {
                    l.shrink = 0.0;
                    l.position = Position::Relative;
                })
                .child(ui.el("p_info_initials").text(initials(&m.name)).nowrap());
            let face = match art_url(m.thumb.as_deref()) {
                Some(url) => {
                    let holder = ui.el("p_info_photo").layout(|l| {
                        l.position = Position::Absolute;
                        l.inset = Insets::ZERO;
                    });
                    let h2 = holder.clone();
                    face.child(holder.child(picture(&ui, url, move || h2.remove())))
                }
                None => face,
            };
            let mut words = ui.el("p_info_words").column().layout(|l| {
                l.shrink = 1.0;
                l.min_width = Length::Px(0.0);
            });
            words = words.child(text(&ui, "p_info_person", m.name.trim()).variant(if phone { "phone" } else { "" }).truncate());
            if let Some(role) = m.role.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                words = words.child(text(&ui, "p_info_role", role).variant(if phone { "phone" } else { "" }).truncate());
            }
            let chip = ui
                .el("p_info_chip")
                .row()
                .align(Align::Center)
                .gap(if phone { 7.2 } else { 9.6 })
                .layout(move |l| l.min_width = Length::Px(0.0))
                .padding(Insets { top: 5.6, bottom: 5.6, start: 5.6, end: if phone { 8.8 } else { 12.8 } })
                .semantics(Semantics::new(Role::ListItem))
                .child(face)
                .child(words);
            grid = grid.child(chip);
        }
        let section = ui
            .el("p_info_section")
            .column()
            .child(margin(w::heading(&ui, 3, "Cast"), 0.0, 8.8))
            .child(grid);
        body = body.child(section);
    }
    let found = links(&info);
    if !found.is_empty() {
        let mut row = ui.el("p_info_links").row().gap(8.0).layout(|l| {
            l.wrap = true;
            l.margin.bottom = 16.0;
        });
        for (label, url) in found {
            let app = ctx.app.clone();
            let u2 = url.clone();
            let a = ui
                .el("p_link_btn")
                .row()
                .align(Align::Center)
                .gap(6.4)
                .child(ui.el("p_link_btn_text").text(label.clone()).nowrap())
                .child(ui.el("p_link_btn_glyph").child(glyph(&ui, Glyph::External, 15.2)))
                .on_click(move || app.open_url(u2.clone(), |_| {}))
                .semantics(
                    Semantics::new(Role::Link).label(format!("{label} (opens in a new tab)")).url(url).action(Action::Click),
                );
            found_links.push((label, a.clone()));
            row = row.child(a);
        }
        body = body.child(row);
    }
    let _ = content.clone().child(hero).child(body);

    let status_text = match lookup {
        Lookup::Loading => "Looking up more…",
        Lookup::Failed => "Couldn't look up more info right now.",
        Lookup::Done => "",
    };
    let sources: Vec<&str> = info.sources.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    let sources_text = if sources.is_empty() { String::new() } else { format!("Info from: {}", sources.join(", ")) };
    with_open(|o| {
        o.status.clear();
        if lookup == Lookup::Loading {
            let _ = o.status.clone().child(spinner(&o.ui));
        }
        let _ = o
            .status
            .clone()
            .variant(if lookup == Lookup::Loading { "loading" } else { "" })
            .hidden(status_text.is_empty())
            .child(text(&o.ui, "p_info_status_text", status_text).variant(if lookup == Lookup::Loading { "loading" } else { "" }));
        let _ = o.sources.clone().text(RichText::plain(sources_text.clone())).hidden(sources_text.is_empty());
        let _ = o.foot.clone().hidden(status_text.is_empty() && sources_text.is_empty());
        let _ = o.dialog.clone().semantics(Semantics::new(Role::Dialog).label(title.clone()));
    });
    let focus_el = match focus_key {
        (Some("link"), Some(l)) => found_links.iter().find(|(n, _)| *n == l).map(|(_, e)| e.clone()),
        (Some(k), _) => keys.get(k).cloned(),
        _ => None,
    };
    OPEN.with(|o| {
        if let Some(o) = o.borrow_mut().as_mut() {
            o.keys = keys;
            o.links = found_links;
        }
    });
    if refresh {
        with_open(|o| o.ui.set_scroll_offset(o.dialog.id(), scroll));
        if let Some(el) = focus_el {
            el.focus();
        }
    }
}

/// `.info-status.is-loading::before`: a 0.8rem ring, a quarter of it gold, turning once in
/// 0.9 s (2.4 s under reduced motion).
fn spinner(ui: &Ui) -> El {
    let _ = ui;
    ui.el("p_info_spinner").width(12.8).height(12.8).layout(|l| l.shrink = 0.0).paint(move |cx| {
        use remedy_ui::paint::{Brush, Item, Path, Stroke, StrokeCap};
        let r = cx.size.width / 2.0 - 1.0;
        let c = (cx.size.width / 2.0, cx.size.height / 2.0);
        let period = if cx.reduced_motion { 2.4 } else { 0.9 };
        let turn = (cx.time.as_secs_f32() / period).fract() * std::f32::consts::TAU;
        let ring = |from: f32, to: f32| {
            let mut p = Path::new();
            let steps = 24;
            for i in 0..=steps {
                let a = from + (to - from) * i as f32 / steps as f32;
                let pt = remedy_ui::core::Point::new(c.0 + r * a.cos(), c.1 + r * a.sin());
                if i == 0 {
                    p.move_to(pt);
                } else {
                    p.line_to(pt);
                }
            }
            p
        };
        let gold = cx.style.foreground;
        let stroke = Stroke { cap: StrokeCap::Butt, ..Stroke::new(2.0) };
        cx.list.push(Item::StrokePath {
            path: ring(0.0, std::f32::consts::TAU),
            stroke: stroke.clone(),
            brush: Brush::Solid(gold.with_alpha(gold.a * 0.25)),
        });
        let top = -std::f32::consts::FRAC_PI_2 - std::f32::consts::FRAC_PI_4 + turn;
        cx.list.push(Item::StrokePath { path: ring(top, top + std::f32::consts::FRAC_PI_2), stroke, brush: Brush::Solid(gold) });
        cx.request_next_frame();
    })
}

/// The dialog's look (styles.css "Info dialog").
pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    let none = States::NONE;
    let black = |a: f32| mix(Color::BLACK, a, clear);
    s.rule("p_info_layer", None, none, StylePatch::default().background(mix(k.bg, 0.72, clear)).backdrop_blur(10.0));
    s.rule(
        "p_info_dialog",
        None,
        none,
        StylePatch::default()
            .background(k.raised)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(16.0)
            .shadow(Shadow::new(0.0, 28.0, 70.0, black(0.45)))
            .outline_offset(-2.0),
    );
    s.rule("p_info_dialog", Some("phone"), none, StylePatch::default().border_width(0.0).radius(0.0));
    s.rule(
        "p_info_close",
        None,
        none,
        StylePatch::default()
            .background(mix(k.bg, 0.78, clear))
            .foreground(k.text)
            .radius(999.0)
            .shadow(Shadow::new(0.0, 2.0, 10.0, Color::BLACK.with_alpha(0.3))),
    );
    s.rule("p_info_close", None, States::HOVER, StylePatch::default().background(k.bg).foreground(k.gold));
    s.rule("p_info_backdrop", None, none, StylePatch::default().background(k.well).radius(15.0));
    s.rule("p_info_backdrop", Some("phone"), none, StylePatch::default().radius(0.0));
    s.rule("p_info_backdrop_img", None, none, StylePatch::default().opacity(0.62));
    s.rule(
        "p_info_scrim",
        None,
        none,
        StylePatch::default().background_layers(gradient(
            "linear-gradient(to bottom, color-mix(in srgb, var(--raised) 10%, transparent) 20%, var(--raised) 98%)",
        )),
    );
    s.rule(
        "p_info_poster",
        None,
        none,
        StylePatch::default()
            .background(k.well)
            .border(k.line)
            .border_width(1.0)
            .radius(10.0)
            .shadow(Shadow::new(0.0, 14.0, 32.0, Color::BLACK.with_alpha(0.45))),
    );
    s.rule("p_info_poster", Some("phone"), none, StylePatch::default().radius(8.0));
    s.rule(
        "p_info_kicker",
        None,
        none,
        StylePatch::default()
            .foreground(k.gold)
            .text_size(12.48)
            .weight(600.0)
            .letter_spacing(LetterSpacing::Em(0.1))
            .text_transform(TextTransform::Upper),
    );
    s.rule("heading", Some("info"), none, StylePatch::default().heading(true).weight(400.0).text_size(32.0).foreground(k.text).line_height(remedy_ui::style::LineHeight::Relative(1.2)));
    s.rule("heading", Some("info_phone"), none, StylePatch::default().heading(true).weight(400.0).text_size(24.0).foreground(k.text).line_height(remedy_ui::style::LineHeight::Relative(1.2)));
    s.rule("p_info_muted", None, none, StylePatch::default().foreground(k.muted));
    s.rule("p_info_star", None, none, StylePatch::default().foreground(k.gold).text_size(16.8));
    s.rule("p_info_score", None, none, StylePatch::default().text_size(17.92).weight(700.0));
    s.rule("p_info_scale", None, none, StylePatch::default().foreground(k.muted).text_size(14.72));
    // .info-dialog button.primary: a gold pill; 82% gold into the text under the pointer.
    s.rule(
        "p_info_play",
        None,
        none,
        StylePatch::default()
            .background(k.gold)
            .border(k.gold)
            .border_width(1.0)
            .foreground(k.bg)
            .weight(600.0)
            .radius(999.0)
            .padding(Insets { top: 6.4, bottom: 6.4, start: 13.6, end: 17.6 }),
    );
    s.rule("p_info_play", None, States::HOVER, StylePatch::default().background(mix(k.gold, 0.82, k.text)));
    s.rule(
        "p_info_tagline",
        None,
        none,
        StylePatch::default().heading(true).text_size(17.92).foreground(k.muted).line_height(remedy_ui::style::LineHeight::Relative(1.4)),
    );
    s.rule("p_info_plot_none", None, none, StylePatch::default().foreground(k.muted));
    s.rule("p_info_dt", None, none, StylePatch::default().foreground(k.muted));
    s.rule(
        "p_info_chip",
        None,
        none,
        StylePatch::default().background(k.well).border(k.line).border_width(1.0).radius(999.0),
    );
    s.rule(
        "p_info_face",
        None,
        none,
        StylePatch::default().background(mix(k.gold, 0.16, k.raised)).foreground(k.gold).radius(999.0),
    );
    s.rule(
        "p_info_initials",
        None,
        none,
        StylePatch::default().text_size(13.12).weight(600.0).letter_spacing(LetterSpacing::Em(0.03)),
    );
    s.rule("p_info_initials", Some("phone"), none, StylePatch::default().text_size(12.0));
    s.rule("p_info_words", None, none, StylePatch::default().line_height(remedy_ui::style::LineHeight::Relative(1.25)));
    s.rule("p_info_person", None, none, StylePatch::default().weight(600.0).text_size(15.04));
    s.rule("p_info_person", Some("phone"), none, StylePatch::default().text_size(14.08));
    s.rule("p_info_role", None, none, StylePatch::default().foreground(k.muted).text_size(13.44));
    s.rule("p_info_role", Some("phone"), none, StylePatch::default().foreground(k.muted).text_size(12.8));
    s.rule(
        "p_link_btn",
        None,
        none,
        StylePatch::default()
            .background(k.bg)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(999.0)
            .text_size(15.04)
            .padding(Insets { top: 5.6, bottom: 5.6, start: 12.8, end: 12.8 }),
    );
    s.rule("p_link_btn", None, States::HOVER, StylePatch::default().border(k.gold).foreground(k.gold));
    s.rule("p_link_btn_glyph", None, none, StylePatch::default().foreground(k.muted));
    s.rule(
        "p_info_foot",
        None,
        none,
        StylePatch::default().foreground(k.muted).text_size(13.6),
    );
    s.rule("p_info_rule", None, none, StylePatch::default().background(k.line));
    s.rule("p_info_status_text", Some("loading"), none, StylePatch::default().foreground(k.text));
    s.rule("p_info_spinner", None, none, StylePatch::default().foreground(k.gold));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_known_fields_and_takes_found_ones() {
        let seed = MediaInfo { title: Some("Heat".into()), year: Some(1995), plot: Some("   ".into()), ..MediaInfo::default() };
        let found = MediaInfo { title: Some(String::new()), plot: Some("A heist.".into()), genres: vec!["Crime".into()], ..MediaInfo::default() };
        let m = merge(&merge(&MediaInfo::default(), &seed), &found);
        assert_eq!((m.title.as_deref(), m.year, m.plot.as_deref()), (Some("Heat"), Some(1995), Some("A heist.")));
        assert_eq!(m.genres, vec!["Crime".to_owned()]);
    }

    #[test]
    fn words_follow_info_ts() {
        assert_eq!((runtime(Some(112)), runtime(Some(45)), runtime(Some(120)), runtime(Some(0))), ("1h 52m".into(), "45m".into(), "2h".into(), String::new()));
        assert_eq!(initials(" jane  q smith "), "JS");
        assert_eq!(initials(""), "?");
        let ep = MediaInfo { kind: Some(MediaKind::Episode), show_title: Some("Tracker".into()), season: Some(4), episode: Some(2), ..MediaInfo::default() };
        assert_eq!(kicker(&ep), "Tracker · S04E02");
        assert_eq!(kicker(&MediaInfo { kind: Some(MediaKind::Episode), ..MediaInfo::default() }), "Episode");
        assert!(several(&["A, B".into()]) && !several(&["A".into(), " ".into()]));
        assert_eq!(year_of(&MediaInfo { premiered: Some("2005-04-04".into()), ..MediaInfo::default() }), "2005");
    }

    #[test]
    fn votes_read_as_intl_compact_with_one_fraction_digit() {
        assert_eq!(votes_in("en", Some(1)), "1 vote");
        assert_eq!(votes_in("en", Some(950)), "950 votes");
        assert_eq!(votes_in("en", Some(12_345)), "12.3K votes");
        assert_eq!(votes_in("en", Some(123_456)), "123.5K votes");
        assert_eq!(votes_in("en", Some(0)), "");
    }

    #[test]
    fn links_are_web_only_and_unique() {
        let info = MediaInfo {
            kind: Some(MediaKind::Movie),
            imdb_id: Some("tt0113277".into()),
            tmdb_id: Some("949".into()),
            trailer: Some("javascript:alert(1)".into()),
            links: vec![portal_api::InfoLink { label: "imdb".into(), url: "https://example.org/x".into() }],
            ..MediaInfo::default()
        };
        let l = links(&info);
        assert_eq!(l, vec![("imdb".to_owned(), "https://example.org/x".to_owned()), ("TMDB".to_owned(), "https://www.themoviedb.org/movie/949".to_owned())]);
    }
}
