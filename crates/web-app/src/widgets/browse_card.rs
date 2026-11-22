//! A folder item's card in a browse grid (main.ts `browseCard`): its art, label and what it
//! is (or, for a season on its show's page, how far watching got), and under it Watchlist,
//! Bookmark and Info for what Portal can save. Laid in [`grid`] (`.grid`).

use std::rc::Rc;

use portal_api::{BrowseItem, ItemKind, MediaKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::layout::{Align, Display, Length, Repeat, Track};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::List;
use crate::theme::{mix, Look};
use crate::widgets::card::bar;
use crate::widgets::shows::{media_of, season_number, season_progress, Media, ShowView};
use crate::widgets::titles::{clean_episode_title, episode_code};
use crate::widgets::{art, saved};
use crate::Ctx;

/// What the card does when pressed, and what Info's first action does.
pub type Open = Rc<dyn Fn(&BrowseItem)>;
/// Opens Info for `media` with a first action (its words and run), from the button.
pub type OpenInfo = Rc<dyn Fn(&Media, &El, String)>;

/// The card's label and the line under it (main.ts `browseCard`).
pub fn words(folder: &str, item: &BrowseItem, view: Option<&ShowView>) -> (String, String, Option<(f64, bool)>) {
    let media = media_of(folder, item);
    let season = if view.is_some() && item.kind == ItemKind::Open { season_number(&item.path) } else { None };
    let known = media.as_ref().filter(|m| m.kind == MediaKind::Movie).and_then(|_| item.info.as_ref().and_then(|i| i.year));
    // Add-ons often put the year in the label already.
    let year = known.filter(|y| !item.label.contains(&y.to_string()));
    let mark = if media.as_ref().is_some_and(|m| m.kind == MediaKind::Tv) {
        "Show".to_owned()
    } else {
        match item.kind {
            ItemKind::Open => "Folder".into(),
            ItemKind::Run => "Action".into(),
            ItemKind::Play => year.map(|y| y.to_string()).unwrap_or_default(),
        }
    };
    let code = if item.kind == ItemKind::Play { episode_code(item.season, item.episode) } else { String::new() };
    let cleaned = if code.is_empty() { String::new() } else { clean_episode_title(&item.label, item.season, item.episode, item.show_title.as_deref()) };
    let label = if !cleaned.is_empty() {
        cleaned
    } else if !item.label.is_empty() {
        item.label.clone()
    } else {
        "Untitled".into()
    };
    let progress = match (view, season) {
        (Some(v), Some(s)) => season_progress(v, s),
        _ => None,
    };
    match progress {
        Some((note, f, current)) => (label, note, Some((f, current))),
        None => (label, if code.is_empty() { mark } else { code }, None),
    }
}

pub fn card(ui: &Ui, ctx: &Ctx, folder: &str, item: &BrowseItem, view: Option<&ShowView>, open: Open, info: OpenInfo) -> El {
    let (label, line, progress) = words(folder, item, view);
    let media = media_of(folder, item);
    let art = art::art(ui, item.art.as_deref(), &label, "card");
    let mut play = ui
        .el("p_card_play")
        .column()
        .gap(7.2)
        .align(Align::Stretch)
        .fill_width()
        .child(art)
        .child(ui.el("p_card_name").text(label.clone()));
    if !line.is_empty() {
        play = play.child(ui.el("p_card_kind").text(line.clone()));
    }
    if let Some((f, _)) = progress.filter(|(f, _)| *f > 0.0) {
        play = play.child(bar(ui, f).layout(|l| l.margin = Insets { top: 0.0, bottom: 0.0, start: 11.2, end: 11.2 }));
    }
    // The button's name is all it says, as its text reads: "Season 4 Up next S04E02".
    let name = if line.is_empty() { label.clone() } else { format!("{label} {line}") };
    let (it, o) = (item.clone(), open.clone());
    let play = play
        // .card-play > :last-child: 0.55rem under it.
        .padding(Insets { top: 0.0, bottom: 8.8, start: 0.0, end: 0.0 })
        .on_click(move || o(&it))
        .semantics(Semantics::new(Role::Button).label(name).action(Action::Click));
    let mut card = ui
        .el("p_card")
        .variant(if progress.is_some_and(|p| p.1) { "current" } else { "" })
        .column()
        .clip()
        .child(play);
    if let Some(m) = media {
        let run_label = match (item.kind, m.kind) {
            (ItemKind::Play, _) => "Play",
            (_, MediaKind::Tv) => "Open show",
            _ => "Open",
        }
        .to_owned();
        let m2 = m.clone();
        let tools = ui
            .el("p_card_actions")
            .row()
            .gap(1.6)
            .layout(|l| {
                l.margin.top = remedy_ui::layout::AUTO;
            })
            .padding(Insets { top: 2.4, bottom: 6.4, start: 5.6, end: 5.6 })
            .semantics(Semantics::new(Role::Group).label(label.clone()))
            .child(saved::save_button_for(ui, ctx, List::Watchlist, m.save.clone(), "card"))
            .child(saved::save_button_for(ui, ctx, List::Bookmarks, m.save.clone(), "card"))
            .child(w::spacer(ui))
            .child(saved::info_button(ui, &m.save.name, "card", move |el| info(&m2, el, run_label.clone())));
        card = card.child(tools);
    }
    card
}

/// `.grid`: as many 180 px columns as fit (140 px on a phone), 0.9rem apart (0.6rem).
pub fn grid(ui: &Ui, cards: Vec<El>, phone: bool) -> El {
    let min = if phone { 140.0 } else { 180.0 };
    let gap = if phone { 9.6 } else { 14.4 };
    ui.el("p_grid").fill_width().children(cards).layout(move |l| {
        l.display = Display::Grid;
        l.columns = vec![Track::Repeat(Repeat::AutoFill, vec![Track::MinMax(Box::new(Track::Px(min)), Box::new(Track::Fr(1.0)))])];
        l.gap = (gap, gap);
        l.width = Length::Percent(1.0);
        l.shrink = 0.0;
    })
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    // .card.is-current: a gold-tinted border on the season the show is up to.
    s.rule("p_card", Some("current"), States::NONE, StylePatch::default().border(mix(k.gold, 0.65, k.line)));
}
