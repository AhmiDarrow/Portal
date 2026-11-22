//! Watchlist and bookmark state shared by browse cards, the details and show pages, the
//! Info dialog and the player (saved.ts), and the toggles that show and flip it: Watchlist,
//! Bookmark and Info, round (`icon`) or as pills with words (`pill`), with the glyphs of
//! `index.html`.
//!
//! ```ignore
//! let watch = saved::save_button_for(ui, ctx, List::Watchlist, title.clone(), "pill");
//! let info = saved::info_button(ui, &title.name, "pill", move |opener| open_info(opener));
//! saved::load(ctx, vec![title.id.clone()], || true);
//! ```
//!
//! A toggle flips at once and is put back, with a notice, if the server refuses. A state read
//! from the server never overwrites a change made after the read started.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use portal_api::{ListPut, SavedState, TitleKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets};
use remedy_ui::layout::{Align, Justify};
use remedy_ui::reactive::{Cancel, Scope, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::RichText;
use remedy_ui::{El, Ui};

use crate::api::{self, List};
use crate::theme::{mix, Look};
use crate::widgets::glyphs::{glyph, Glyph};
use crate::Ctx;

/// What a toggle saves: a movie, or a show (an episode saves its show).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveTitle {
    pub id: String,
    pub kind: TitleKind,
    pub name: String,
    pub art: Option<String>,
    pub year: Option<u32>,
    pub imdb_id: Option<String>,
}

#[derive(Default)]
struct Store {
    states: HashMap<String, SavedState>,
    /// Sequence number of the last local change per title; older server answers lose to it.
    touched: HashMap<String, u64>,
    busy: HashSet<(&'static str, String)>,
    seq: u64,
}

thread_local! {
    static STORE: RefCell<Store> = RefCell::default();
    static VERSION: Cell<Option<Signal<u64>>> = const { Cell::new(None) };
}

/// Bumped on every change, so every toggle repaints (saved.ts `paintSaved`). It lives as
/// long as the page: no screen owns it.
fn version() -> Signal<u64> {
    VERSION.with(|v| {
        v.get().unwrap_or_else(|| {
            let (s, _) = Scope::detached(|| Signal::new(0u64));
            v.set(Some(s));
            s
        })
    })
}

fn bump() {
    version().update(|n| *n += 1);
}

/// The known state of `id` (both off until known).
pub fn state(id: &str) -> SavedState {
    STORE.with(|s| s.borrow().states.get(id).copied().unwrap_or_default())
}

/// `state`, read so the caller repaints when it changes.
pub fn state_tracked(id: &str) -> SavedState {
    version().get();
    state(id)
}

/// Records state the page already knows (the details page's answer has it).
pub fn note(id: &str, watchlist: Option<bool>, bookmarks: Option<bool>) {
    if id.is_empty() {
        return;
    }
    STORE.with(|s| {
        let mut s = s.borrow_mut();
        let st = s.states.entry(id.to_owned()).or_default();
        if let Some(w) = watchlist {
            st.watchlist = w;
        }
        if let Some(b) = bookmarks {
            st.bookmarks = b;
        }
    });
    bump();
}

/// Fetches the state of `ids` in one request, then repaints every toggle, unless `wanted`
/// says the page moved on.
pub fn load(ctx: &Ctx, ids: Vec<String>, wanted: impl Fn() -> bool + 'static) {
    let mut unique: Vec<String> = Vec::new();
    for id in ids {
        if !id.is_empty() && !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        return;
    }
    let started = STORE.with(|s| s.borrow().seq);
    let asked = unique.clone();
    ctx.api.run(api::saved(&unique), &Cancel::new(), move |out| {
        let Ok(map) = out else { return };
        if !wanted() {
            return;
        }
        STORE.with(|s| {
            let mut s = s.borrow_mut();
            for id in &asked {
                let Some(row) = map.items.get(id) else { continue };
                if s.touched.get(id).copied().unwrap_or(0) > started {
                    continue;
                }
                s.states.insert(id.clone(), *row);
            }
        });
        bump();
    });
}

fn list_of(list: List, st: SavedState) -> bool {
    match list {
        List::Watchlist => st.watchlist,
        List::Bookmarks => st.bookmarks,
    }
}

fn set_local(list: List, id: &str, on: bool) {
    STORE.with(|s| {
        let mut s = s.borrow_mut();
        s.seq += 1;
        let seq = s.seq;
        let st = s.states.entry(id.to_owned()).or_default();
        match list {
            List::Watchlist => st.watchlist = on,
            List::Bookmarks => st.bookmarks = on,
        }
        s.touched.insert(id.to_owned(), seq);
    });
    bump();
}

/// What saved.ts `bodyFor` sends.
pub fn body_for(title: &SaveTitle) -> ListPut {
    ListPut {
        kind: Some(title.kind),
        name: Some(title.name.clone()),
        art: title.art.clone(),
        year: title.year,
        imdb_id: title.imdb_id.clone().filter(|s| !s.is_empty()),
    }
}

/// Flips one list for `title` right away, then undoes it with a notice if the server refuses.
pub fn toggle(ctx: &Ctx, list: List, title: &SaveTitle) {
    if title.id.is_empty() {
        return;
    }
    let key = (list.name(), title.id.clone());
    if !STORE.with(|s| s.borrow_mut().busy.insert(key.clone())) {
        return;
    }
    let was = list_of(list, state(&title.id));
    set_local(list, &title.id, !was);
    let call = if was { api::delete_list(list, &title.id) } else { api::put_list(list, &title.id, &body_for(title)) };
    let (c, id) = (ctx.clone(), title.id.clone());
    ctx.api.run(call, &Cancel::new(), move |out| {
        STORE.with(|s| s.borrow_mut().busy.remove(&key));
        if let Err(e) = out {
            set_local(list, &id, was);
            let what = if list == List::Watchlist { "the watchlist" } else { "bookmarks" };
            let why = Some(e.detail.trim()).filter(|d| !d.is_empty()).unwrap_or("Portal could not save that.");
            c.activity.notice(format!("Couldn't update {what}. {why}"));
        }
    });
}

/// saved.ts `tip`: the button's tooltip.
pub fn tip(list: List, on: bool, name: &str) -> String {
    let quoted = if name.is_empty() { "this title".to_owned() } else { format!("“{name}”") };
    match (list, on) {
        (List::Watchlist, true) => format!("Remove {quoted} from the watchlist"),
        (List::Watchlist, false) => format!("Add {quoted} to the watchlist"),
        (List::Bookmarks, true) => format!("Remove the bookmark for {quoted}"),
        (List::Bookmarks, false) => format!("Bookmark {quoted}"),
    }
}

/// The pill's words: "Watchlist" / "In watchlist", "Bookmark" / "Bookmarked".
pub fn words(list: List, on: bool) -> &'static str {
    match (list, on) {
        (List::Watchlist, false) => "Watchlist",
        (List::Watchlist, true) => "In watchlist",
        (List::Bookmarks, false) => "Bookmark",
        (List::Bookmarks, true) => "Bookmarked",
    }
}

/// The name a toggle keeps whatever its state (`aria-pressed` carries the state).
fn name(list: List) -> &'static str {
    match list {
        List::Watchlist => "Watchlist",
        List::Bookmarks => "Bookmark",
    }
}

/// Glyph sizes: `.pill-btn svg` 1.1rem; `.icon-btn svg` 1.3rem, 1.2rem in a card's actions.
pub fn glyph_size(variant: &str) -> f32 {
    match variant {
        "pill" => 17.6,
        "card" => 19.2,
        _ => 20.8,
    }
}

pub fn frame(ui: &Ui, variant: &'static str) -> El {
    let widget = if variant == "pill" { "p_pill" } else { "p_icon_btn" };
    let el = ui.el(widget).variant(variant).row().align(Align::Center);
    if variant == "pill" {
        el.gap(6.4)
    } else {
        let side = if variant == "card" { 33.6 } else { 36.0 };
        el.justify(Justify::Center).width(side).height(side).layout(|l| l.shrink = 0.0)
    }
}

/// A Watchlist or Bookmark toggle for whatever `title` holds (disabled while it holds none),
/// in `variant`: `pill` (with words), `icon` (round), `card` (a browse card's actions) or
/// `dock` (the player's side panel).
#[track_caller]
pub fn save_button(ui: &Ui, ctx: &Ctx, list: List, title: Signal<Option<SaveTitle>>, variant: &'static str) -> El {
    let on = move || title.with(|t| t.as_ref().is_some_and(|t| list_of(list, state_tracked(&t.id))));
    let (off_glyph, on_glyph) = match list {
        List::Watchlist => (Glyph::WatchlistAdd, Glyph::WatchlistOn),
        List::Bookmarks => (Glyph::BookmarkAdd, Glyph::BookmarkOn),
    };
    let size = glyph_size(variant);
    let off = glyph(ui, off_glyph, size);
    let lit = glyph(ui, on_glyph, size);
    let (o2, l2) = (off.clone(), lit.clone());
    let mut b = frame(ui, variant).child(off).child(lit);
    if variant == "pill" {
        b = b.child(ui.el("p_pill_text").text(move || RichText::plain(words(list, on()))).nowrap());
    }
    let c = ctx.clone();
    let b = b.on_click(move || {
        if let Some(t) = title.get_untracked().ok().flatten() {
            toggle(&c, list, &t);
        }
    });
    let b2 = b.clone();
    let b = b
        .bind(move || {
            let v = on();
            o2.clone().hidden(v);
            l2.clone().hidden(!v);
            b2.state(States::CHECKED, v);
            let none = title.with(Option::is_none);
            b2.state(States::DISABLED, none);
        })
        .disabled(move || title.with(Option::is_none))
        .semantics(move || {
            Semantics::new(Role::Button).label(name(list)).checked(on()).action(Action::Click)
        });
    remedy_ui::widgets::tooltip(&b, move || {
        let n = title.with(|t| t.as_ref().map(|t| t.name.clone()).unwrap_or_default());
        tip(list, on(), &n)
    });
    b
}

/// [`save_button`] for one title that does not change.
#[track_caller]
pub fn save_button_for(ui: &Ui, ctx: &Ctx, list: List, title: SaveTitle, variant: &'static str) -> El {
    save_button(ui, ctx, list, Signal::new(Some(title)), variant)
}

/// An Info button (saved.ts `infoButtonHtml`): `on_press` gets the button, to give focus
/// back to when the dialog closes.
#[track_caller]
pub fn info_button(ui: &Ui, name: &str, variant: &'static str, on_press: impl Fn(&El) + 'static) -> El {
    let mut b = frame(ui, variant).child(glyph(ui, Glyph::Info, glyph_size(variant)));
    if variant == "pill" {
        b = b.child(ui.el("p_pill_text").text("Info").nowrap());
    }
    let me: Rc<RefCell<Option<El>>> = Rc::default();
    let m2 = me.clone();
    let b = b
        .on_click(move || {
            if let Some(el) = m2.borrow().as_ref() {
                on_press(el);
            }
        })
        .semantics(Semantics::new(Role::Button).label("Info").action(Action::Click));
    *me.borrow_mut() = Some(b.clone());
    let tip = if name.is_empty() { "Info".to_owned() } else { format!("Info about “{name}”") };
    remedy_ui::widgets::tooltip(&b, tip);
    b
}

/// The toggles' look (styles.css `.pill-btn`, `.icon-btn`, `.card-actions .icon-btn`,
/// `.dock-save .icon-btn`).
pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    // button + .pill-btn: 0.4rem 0.85rem 0.4rem 0.7rem, a full round, a hairline of the line
    // color on the page's background; a gold border under the pointer (button:hover).
    s.rule(
        "p_pill",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.bg)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(999.0)
            .padding(Insets { top: 6.4, bottom: 6.4, start: 11.2, end: 13.6 }),
    );
    s.rule("p_pill", None, States::HOVER, StylePatch::default().border(k.gold).background(mix(k.gold, 0.08, k.bg)));
    s.rule("p_pill", None, States::CHECKED, StylePatch::default().foreground(k.gold).border(k.gold).background(mix(k.gold, 0.12, k.bg)));
    s.rule("p_pill", None, States::CHECKED | States::HOVER, StylePatch::default().background(mix(k.gold, 0.18, k.bg)));
    s.rule("p_pill", None, States::DISABLED, StylePatch::default().opacity(0.45).border(k.line).background(k.bg));
    // .icon-btn: round, bare, the text color; gold while on.
    let bare = StylePatch::default().background(clear).border(clear).border_width(0.0).radius(999.0).padding(Insets::ZERO);
    s.rule("p_icon_btn", None, States::NONE, bare.clone().foreground(k.text));
    s.rule("p_icon_btn", None, States::CHECKED, StylePatch::default().foreground(k.gold));
    s.rule("p_icon_btn", None, States::DISABLED, StylePatch::default().opacity(0.45));
    // .card-actions .icon-btn: muted; the text color on a 10% wash under the pointer.
    s.rule("p_icon_btn", Some("card"), States::NONE, StylePatch::default().foreground(k.muted));
    s.rule("p_icon_btn", Some("card"), States::HOVER, StylePatch::default().foreground(k.text).background(mix(k.text, 0.10, clear)));
    // .dock-save .icon-btn: muted; the text color (on the dock's 13% wash) under the pointer.
    s.rule("p_icon_btn", Some("dock"), States::NONE, StylePatch::default().foreground(k.muted));
    s.rule("p_icon_btn", Some("dock"), States::HOVER, StylePatch::default().foreground(k.text).background(mix(k.text, 0.13, clear)));
    for v in ["card", "dock"] {
        s.rule("p_icon_btn", Some(v), States::CHECKED, StylePatch::default().foreground(k.gold));
        s.rule("p_icon_btn", Some(v), States::CHECKED | States::HOVER, StylePatch::default().foreground(k.gold));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_and_tips_follow_saved_ts() {
        assert_eq!(tip(List::Watchlist, false, "Heat"), "Add “Heat” to the watchlist");
        assert_eq!(tip(List::Watchlist, true, ""), "Remove this title from the watchlist");
        assert_eq!(tip(List::Bookmarks, false, "Heat"), "Bookmark “Heat”");
        assert_eq!(tip(List::Bookmarks, true, "Heat"), "Remove the bookmark for “Heat”");
        assert_eq!((words(List::Watchlist, true), words(List::Bookmarks, false)), ("In watchlist", "Bookmark"));
    }

    #[test]
    fn the_body_leaves_out_what_is_unknown() {
        let t = SaveTitle { id: "x".into(), kind: TitleKind::Movie, name: "X".into(), art: None, year: Some(1999), imdb_id: Some(String::new()) };
        let b = serde_json::to_value(body_for(&t)).unwrap_or_default();
        assert_eq!(b, serde_json::json!({"kind": "movie", "name": "X", "year": 1999}));
    }
}
