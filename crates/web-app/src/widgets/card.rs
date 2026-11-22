//! A title's card on a shelf or in a grid (main.ts `cardHtml`): its art, name, what it is,
//! how far along it is, and what pressing it does ("Resume 12:34"), with a link to its
//! episodes or details under it and, on the watchlist and bookmarks, Rename and Remove.
//!
//! ```ignore
//! let c = card::card(ui, ctx, &item, None, card::play_card(ctx));
//! ```

use std::rc::Rc;

use portal_api::{Card, ListPut, TitleKind};
use remedy_ui::reactive::Cancel;

use crate::api::{self, List};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{CornerRadii, Insets, Rect, Vec2};
use remedy_ui::layout::{Align, Justify};
use remedy_ui::motion::{CubicBezier, Easing, Motion, Target};
use remedy_ui::paint::Brush;
use remedy_ui::reactive::Signal;
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::{El, Ui};

use crate::pages::Page;
use crate::theme::{mix, Look};
use crate::widgets::{art, titles};
use crate::Ctx;

/// What pressing a card does, in words: Play, Resume 12:34, Play again, or Episodes.
pub fn card_action(card: &Card) -> String {
    if card.kind == TitleKind::Tv && card.file.is_none() {
        return "Episodes".into();
    }
    if let Some(at) = titles::resume_of(card.resume_seconds) {
        return format!("Resume {}", titles::format_clock(at));
    }
    if card.fraction.is_some_and(|f| f >= 0.95) {
        return "Play again".into();
    }
    "Play".into()
}

/// The line under the name: the episode, else "Show", else "Movie · 1999".
pub fn kind_line(card: &Card) -> String {
    let extra = titles::episode_line(card.season, card.episode, card.episode_name.as_deref());
    if !extra.is_empty() {
        return extra;
    }
    match (card.kind, card.year) {
        (TitleKind::Tv, _) => "Show".into(),
        (TitleKind::Movie, Some(y)) => format!("Movie · {y}"),
        (TitleKind::Movie, None) => "Movie".into(),
    }
}

/// The link under the art: a show's episodes from any of its cards with a file to play,
/// else its details.
pub fn more_page(card: &Card) -> (Page, &'static str) {
    if card.kind == TitleKind::Tv && card.file.is_some() {
        (Page::Show { id: card.id.clone(), season: card.season }, "Episodes")
    } else {
        (Page::Title(card.id.clone()), "Details")
    }
}

/// What a card's press does. Home and the lists pass [`play_card`].
pub type OnPlay = Rc<dyn Fn(&Card)>;

/// main.ts `playCard`: a show with nothing to resume opens its episodes; an episode plays
/// with its season open under the player, so stopping lands on its episodes; a movie plays.
pub fn play_card(ctx: &Ctx) -> OnPlay {
    let ctx = ctx.clone();
    Rc::new(move |card: &Card| {
        let resume = titles::resume_of(card.resume_seconds);
        if card.kind == TitleKind::Tv {
            let episodes = Page::Show { id: card.id.clone(), season: card.season };
            let Some(file) = card.file.clone().filter(|f| !f.is_empty()) else {
                ctx.go(&episodes);
                return;
            };
            let line = titles::episode_line(card.season, card.episode, card.episode_name.as_deref());
            let title = [line, card.episode_name.clone().unwrap_or_default(), card.name.clone()].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
            crate::widgets::play::play(
                &ctx,
                crate::widgets::play::PlayTarget {
                    kind: portal_api::PlayKind::Episode,
                    id: card.id.clone(),
                    file,
                    title,
                    season: card.season,
                    episode: card.episode,
                    art: card.art.clone(),
                    show_title: Some(card.name.clone()),
                    show_id: Some(card.id.clone()),
                    info: None,
                    resume_seconds: resume,
                    manual_links: false,
                    progress_kind: TitleKind::Tv,
                    progress_id: card.id.clone(),
                    progress_name: card.name.clone(),
                    track_progress: true,
                },
            );
            // The season opens under the player, so stopping lands on its episodes.
            ctx.go(&episodes);
            return;
        }
        let file = card.file.clone().filter(|f| !f.is_empty()).unwrap_or_else(|| card.id.clone());
        crate::widgets::play::play(&ctx, crate::widgets::play::movie_target(&card.id, &file, &card.name, card.art.clone(), resume));
    })
}

/// A card for `item`. `tools` go beside the Episodes/Details link (Rename and Remove).
pub fn card(ui: &Ui, ctx: &Ctx, item: &Card, tools: Option<El>, on_play: OnPlay) -> El {
    let action = card_action(item);
    let fine = {
        let caps = ui.pointer_caps();
        move || caps.with(|c| c.hover && c.fine)
    };
    let reduced = ui.reduced_motion();
    // The add-on's poster for the title reads better on a shelf than an episode still
    // squeezed into a poster frame; the line below names the episode.
    let picture = art::poster_from_id(&item.id).or_else(|| item.art.clone());
    let art = art::art(ui, picture.as_deref(), &item.name, "card");
    let mut play = ui
        .el("p_card_play")
        .column()
        .gap(7.2)
        .align(Align::Stretch)
        .fill_width()
        .child(art.clone())
        .child(ui.el("p_card_name").text(item.name.clone()))
        .child(ui.el("p_card_kind").text(kind_line(item)));
    if let Some(f) = item.fraction.filter(|f| f.is_finite()) {
        play = play.child(bar(ui, f).layout(|l| l.margin = Insets { top: 0.0, bottom: 0.0, start: 11.2, end: 11.2 }));
    }
    let cta = ui.el("p_card_cta").text(action.clone()).nowrap().layout(|l| {
        l.align_self = Some(Align::Start);
        l.margin = Insets { top: 0.0, bottom: 8.8, start: 11.2, end: 11.2 };
    });
    let c2 = cta.clone();
    let (target, who) = (item.clone(), on_play);
    let label = format!("{action}: {}", item.name);
    let lit = Signal::new(false);
    let play = play
        .child(cta)
        .on_click(move || who(&target))
        .on_hover(move |on| lit.set(on))
        .semantics(Semantics::new(Role::Button).label(label).action(Action::Click));
    let p2 = play.clone();
    let play = play.on_focus_change(move |on| {
        // `:focus-visible`: a keyboard focus, not the press of a click.
        lit.set(on && p2.states().contains(States::FOCUS_VISIBLE));
    });
    let _ = c2.clone().bind(move || c2.state(States::HOVER, lit.get()));

    let (page, more) = more_page(item);
    let mut meta = ui
        .el("p_card_meta")
        .row()
        .justify(Justify::SpaceBetween)
        .gap(6.4)
        .layout(|l| l.wrap = true)
        .child(crate::widgets::link(ui, &ctx.router, &page.path(), more, "meta", None));
    if let Some(t) = tools {
        meta = meta.child(t);
    }

    let card = ui
        .el("p_card")
        .column()
        .clip()
        .variant(move || if fine() { "fine" } else { "" })
        .child(play)
        .child(meta);
    let (c3, a2) = (card.clone(), art);
    let fine2 = {
        let caps = ui.pointer_caps();
        move || caps.with(|c| c.hover && c.fine)
    };
    card.on_hover(move |on| {
        if reduced || !fine2() {
            return;
        }
        // .card:hover: up 3 px in 0.16 s; its art grows 4.5% in 0.35 s (CSS `ease`).
        let ease = Easing::Bezier(CubicBezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 });
        let lift = if on { -3.0 } else { 0.0 };
        c3.animate(Target::Translate(Vec2::new(0.0, lift)), Motion::Tween { duration: ms(160), easing: ease });
        a2.animate(Target::Scale(if on { 1.045 } else { 1.0 }), Motion::Tween { duration: ms(350), easing: ease });
    })
}

/// Which saved title's name is being edited (main.ts `editing`), shared by a page's cards.
pub type Editing = Signal<Option<(List, String)>>;

/// Rename and Remove for a watchlist or bookmarks card (main.ts `listTools`), or the
/// rename form while `editing` names this card. `changed` runs after a save or removal
/// (the page fetches its list again).
pub fn list_tools(ui: &Ui, ctx: &Ctx, list: List, item: &Card, editing: Editing, changed: Rc<dyn Fn()>) -> El {
    let key = (list, item.id.clone());
    let (u, c, it) = (ui.clone(), ctx.clone(), item.clone());
    let k2 = key.clone();
    let host = ui.el("p_row_actions").row().gap(5.6).align(Align::Center).layout(|l| l.wrap = true);
    let h2 = host.clone();
    host.bind(move || {
        let open = editing.with(|e| e.as_ref() == Some(&k2));
        h2.clear();
        let el = if open {
            rename_form(&u, &c, list, &it, editing, changed.clone())
        } else {
            tool_buttons(&u, &c, list, &it, editing, changed.clone())
        };
        let _ = h2.clone().child(el);
    })
}

fn tool_buttons(ui: &Ui, ctx: &Ctx, list: List, item: &Card, editing: Editing, changed: Rc<dyn Fn()>) -> El {
    let id = item.id.clone();
    let ch = changed.clone();
    let rename = crate::widgets::button(ui, "Rename", move || {
        editing.set(Some((list, id.clone())));
        ch();
    })
    .variant("quiet");
    let (c, id) = (ctx.clone(), item.id.clone());
    let from = if list == List::Watchlist { "the watchlist" } else { "bookmarks" };
    let label = format!("Remove {} from {from}", item.name);
    let remove = crate::widgets::button(ui, "Remove", move || {
        let (c2, ch, id2) = (c.clone(), changed.clone(), id.clone());
        c.api.run(api::delete_list(list, &id), &Cancel::new(), move |out| match out {
            Ok(_) => {
                if editing.with(|e| e.as_ref().is_some_and(|(l, k)| *l == list && *k == id2)) {
                    editing.set(None);
                }
                ch();
            }
            Err(e) => c2.activity.notice(crate::api::shown_detail(&e)),
        });
    })
    .variant("quiet")
    .semantics(Semantics::new(Role::Button).label(label).action(Action::Click));
    ui.el("p_row_actions").row().gap(5.6).align(Align::Center).layout(|l| l.wrap = true).child(rename).child(remove)
}

fn rename_form(ui: &Ui, ctx: &Ctx, list: List, item: &Card, editing: Editing, changed: Rc<dyn Fn()>) -> El {
    let (c, it, ch) = (ctx.clone(), item.clone(), changed.clone());
    let save = Rc::new(move |name: String| {
        if it.id.trim().is_empty() {
            c.activity.notice("That item is gone.");
            editing.set(None);
            ch();
            return;
        }
        let body = ListPut { kind: Some(it.kind), name: Some(name), art: it.art.clone().filter(|a| !a.is_empty()), year: None, imdb_id: None };
        let (c2, ch2) = (c.clone(), ch.clone());
        c.api.run(api::put_list(list, &it.id, &body), &Cancel::new(), move |out| match out {
            Ok(_) => {
                editing.set(None);
                ch2();
            }
            Err(e) => c2.activity.notice(crate::api::shown_detail(&e)),
        });
    });
    rename(ui, ctx, &item.name, save, Rc::new(move || {
        editing.set(None);
        changed();
    }))
}

/// The rename form (main.ts `renameForm`, `.rename`): the name field, focused with its text
/// selected, then Save and Cancel. `save` gets the trimmed name; an empty one says "Enter a
/// name." and keeps the field.
pub fn rename(ui: &Ui, ctx: &Ctx, name: &str, save: Rc<dyn Fn(String)>, cancel: Rc<dyn Fn()>) -> El {
    let field = remedy_ui::widgets::text_input(ui, "Name").max_length(200);
    field.set_value(name);
    let (c, f2) = (ctx.clone(), field.clone());
    let submit = Rc::new(move || {
        let name = f2.value().trim().to_owned();
        if name.is_empty() {
            c.activity.notice("Enter a name.");
            f2.el().focus();
            return;
        }
        save(name);
    });
    let s2 = submit.clone();
    let input = field.clone().on_submit(move || s2()).el().layout(|l| {
        l.grow = 1.0;
        l.shrink = 1.0;
        l.basis = remedy_ui::layout::Length::Px(128.0);
        l.min_width = remedy_ui::layout::Length::Px(0.0);
    });
    let f3 = field.clone();
    ui.after(std::time::Duration::ZERO, move || {
        f3.el().focus();
        f3.select_all();
    });
    ui.el("p_rename")
        .row()
        .gap(5.6)
        .fill_width()
        .layout(|l| l.wrap = true)
        .child(input)
        .child(crate::widgets::button(ui, "Save", move || submit()))
        .child(crate::widgets::button(ui, "Cancel", move || cancel()))
}

fn ms(n: u64) -> std::time::Duration {
    std::time::Duration::from_millis(n)
}

/// A progress bar (`.bar`): 5 px of the track, the part done in gold.
pub fn bar(ui: &Ui, fraction: f64) -> El {
    let f = fraction.clamp(0.0, 1.0) as f32;
    ui.el("p_bar").height(5.0).layout(|l| l.shrink = 0.0).paint(move |cx| {
        let (w, h) = (cx.size.width, cx.size.height);
        let r = CornerRadii::all(h / 2.0);
        cx.list.push_clip(Rect::new(0.0, 0.0, w, h), r);
        cx.list.fill_rect(Rect::new(0.0, 0.0, w, h), Brush::Solid(cx.style.background));
        if f > 0.0 {
            let x1 = if cx.rtl { w * (1.0 - f) } else { w * f };
            let (a, b) = if cx.rtl { (x1, w) } else { (0.0, x1) };
            cx.list.fill_rect(Rect::new(a, 0.0, b, h), Brush::Solid(cx.style.foreground));
        }
        cx.list.pop_clip();
    })
}

/// A card's shelf width: 188 px, 140 px at 720 px and under (`.scroller`).
pub fn shelf_width(narrow: bool) -> f32 {
    if narrow { 140.0 } else { 188.0 }
}

/// `true` at 720 px and under (styles.css `@media (max-width: 720px)`).
pub fn is_narrow(width: f32) -> bool {
    width <= 720.0
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let card = StylePatch::default().background(k.raised).border(k.line).border_width(1.0).radius(14.0);
    s.rule("p_card", None, States::NONE, card);
    let lit = mix(k.gold, 0.55, k.line);
    s.rule("p_card", None, States::FOCUS_WITHIN, StylePatch::default().border(lit));
    s.rule(
        "p_card",
        Some("fine"),
        States::HOVER,
        StylePatch::default().border(lit).shadow(remedy_ui::style::Shadow::new(
            0.0,
            8.0,
            18.0,
            mix(remedy_ui::core::Color::BLACK, 0.30, remedy_ui::core::Color::TRANSPARENT),
        )),
    );
    s.rule("p_card_play", None, States::NONE, StylePatch::default().foreground(k.text));
    s.rule("p_card_play", None, States::HOVER, StylePatch::default().foreground(k.gold));
    // The card clips, so the focus ring goes inside it.
    s.rule("p_card_play", None, States::NONE, StylePatch::default().outline_offset(-2.0));
    let side = |x: f32| Insets { top: 0.0, bottom: 0.0, start: x, end: x };
    s.rule("p_card_name", None, States::NONE, StylePatch::default().text_size(16.8).line_height(remedy_ui::style::LineHeight::Relative(1.25)).padding(side(11.2)));
    s.rule(
        "p_card_kind",
        None,
        States::NONE,
        StylePatch::default().text_size(14.08).foreground(k.muted).padding(side(11.2)),
    );
    s.rule(
        "p_card_cta",
        None,
        States::NONE,
        StylePatch::default()
            .text_size(13.6)
            .foreground(k.gold)
            .border(mix(k.gold, 0.45, k.line))
            .border_width(1.0)
            .radius(99.0)
            .padding(Insets { top: 3.52, bottom: 3.52, start: 10.4, end: 10.4 }),
    );
    s.rule("p_card_cta", None, States::HOVER, StylePatch::default().background(k.gold).border(k.gold).foreground(k.bg));
    s.rule(
        "p_card_meta",
        None,
        States::NONE,
        StylePatch::default().padding(Insets { top: 0.0, bottom: 11.2, start: 11.2, end: 11.2 }),
    );
    s.rule("p_bar", None, States::NONE, StylePatch::default().background(k.track).foreground(k.gold));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(kind: TitleKind) -> Card {
        Card {
            id: "x".into(),
            kind,
            name: "Name".into(),
            art: None,
            fraction: None,
            resume_seconds: None,
            season: None,
            episode: None,
            episode_name: None,
            file: None,
            year: None,
            imdb_id: None,
        }
    }

    #[test]
    fn words_follow_card_html() {
        let mut c = card(TitleKind::Tv);
        assert_eq!((card_action(&c), kind_line(&c), more_page(&c).1), ("Episodes".into(), "Show".into(), "Details"));
        c.file = Some("f".into());
        c.season = Some(1);
        c.episode = Some(2);
        c.episode_name = Some("1x02. Two".into());
        c.resume_seconds = Some(754.0);
        assert_eq!((card_action(&c), kind_line(&c), more_page(&c).1), ("Resume 12:34".into(), "S01E02 · Two".into(), "Episodes"));
        let mut m = card(TitleKind::Movie);
        m.year = Some(1999);
        m.fraction = Some(0.97);
        m.resume_seconds = Some(30.0);
        assert_eq!((card_action(&m), kind_line(&m)), ("Play again".into(), "Movie · 1999".into()));
        m.fraction = Some(0.5);
        assert_eq!(card_action(&m), "Play");
    }
}
