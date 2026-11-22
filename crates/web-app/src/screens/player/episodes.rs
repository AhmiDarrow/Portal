//! Theater's episodes panel (player.ts:451-962, index.html `#dock-theater-side`): the tab
//! that opens and closes the list, the show and season over it, season chips, and the
//! season's episodes with their stills, progress and what pressing one does. A resolve owns
//! Kodi, so the list waits for it (`Files.GetDirectory` would steal Kodi from the source
//! search) and loads once the item is on screen.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use portal_api::{BrowseItem, BrowsePage, Episode, ItemKind, PlayKind, TitleDetail, TitleKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets, Point, Vec2};
use remedy_ui::input::Key;
use remedy_ui::layout::{Align, Display, Justify, Length, Position, Track};
use remedy_ui::paint::Affine;
use remedy_ui::reactive::{untrack, Signal};
use remedy_ui::style::{LetterSpacing, LineHeight, States, StylePatch, StyleSheet, TextTransform};
use remedy_ui::text::{ParagraphOptions, TextStyle};
use remedy_ui::view::RichText;
use remedy_ui::{El, Ui};

use super::{display_media_title, human_title, Dock};
use crate::theme::{mix, Look};
use crate::widgets::glyphs::{glyph, Glyph};
use crate::widgets::play::PlayTarget;
use crate::widgets::titles::{clean_episode_title, episode_code, format_clock};
use crate::api;

/// One listed episode (player.ts `ListedEpisode`).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub show_id: String,
    pub show_name: String,
    pub file: String,
    pub season: Option<u32>,
    pub episode: u32,
    pub name: String,
    pub art: Option<String>,
    pub info: Option<portal_api::MediaInfo>,
    pub position: f64,
    pub duration: f64,
    pub finished: bool,
    pub resumable: bool,
}

/// What the panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
struct Listed {
    show: String,
    heading: String,
    show_id: String,
    chips: Vec<u32>,
    selected: Option<u32>,
    note: String,
    rows: Vec<Row>,
}

/// The panel's state (player.ts `episodesOpen`, `episodeWait`, `holdEpisodes`, the caches).
pub struct Episodes {
    pub(crate) open: Signal<bool>,
    wait: Cell<bool>,
    hold: Cell<bool>,
    ticket: Cell<u64>,
    listed_show: RefCell<String>,
    listed_season: Cell<Option<u32>>,
    followed_season: Cell<Option<u32>>,
    queued_season: Cell<Option<u32>>,
    reload: Cell<bool>,
    titles: RefCell<HashMap<String, Option<TitleDetail>>>,
    pages: RefCell<HashMap<String, BrowsePage>>,
    listed: Signal<Listed>,
    /// The playing row was centred for this show, file, season and episode.
    scrolled: RefCell<String>,
}

impl Episodes {
    pub fn new() -> Episodes {
        Episodes {
            open: Signal::new(true),
            wait: Cell::new(false),
            hold: Cell::new(false),
            ticket: Cell::new(0),
            listed_show: RefCell::default(),
            listed_season: Cell::new(None),
            followed_season: Cell::new(None),
            queued_season: Cell::new(None),
            reload: Cell::new(false),
            titles: RefCell::default(),
            pages: RefCell::default(),
            listed: Signal::new(Listed::default()),
            scrolled: RefCell::default(),
        }
    }

    /// From the start of a play until the dock shows that item, the list stays shut.
    pub fn hold(&self, on: bool) {
        self.hold.set(on);
        if on {
            self.wait.set(false);
        }
    }

    /// Teardown: the list must not browse the show that is leaving.
    pub fn release(&self) {
        self.wait.set(false);
    }

    /// Opening theater loads the season again (player.ts `toggleTheater`), without wiping
    /// a list already on screen.
    pub fn reload(&self) {
        self.titles.borrow_mut().clear();
        self.pages.borrow_mut().clear();
        self.scrolled.borrow_mut().clear();
        self.reload.set(true);
    }
}

/// The show an episode belongs to; movies and loose files have none (player.ts `showIdOf`).
pub fn show_id_of(t: Option<&PlayTarget>) -> String {
    let Some(t) = t else { return String::new() };
    if t.kind != PlayKind::Episode && t.progress_kind != TitleKind::Tv {
        return String::new();
    }
    t.show_id.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| t.progress_id.clone())
}

fn season_label(season: u32) -> String {
    if season == 0 { "Specials".into() } else { format!("Season {season}") }
}

fn theater_on(d: &Dock) -> bool {
    let i = d.inner();
    i.open.get_untracked().unwrap_or(false)
        && i.theater.get_untracked().unwrap_or(false)
        && untrack(|| i.player.is_theater() && !i.player.is_compact())
}

fn current(d: &Dock) -> Option<PlayTarget> {
    d.inner().target.get_untracked().ok().flatten()
}

/// Library folders win, then folders this tab opened; Specials last (player.ts `seasonChoices`).
fn season_choices(title: Option<&TitleDetail>, show_id: &str) -> Vec<(u32, String)> {
    let mut by: Vec<(u32, String)> = Vec::new();
    let put = |s: u32, p: String, by: &mut Vec<(u32, String)>| {
        if !by.iter().any(|(x, _)| *x == s) {
            by.push((s, p));
        }
    };
    for f in title.map(|t| t.season_folders.clone()).unwrap_or_default() {
        if !f.path.is_empty() {
            put(f.season, f.path, &mut by);
        }
    }
    for f in crate::widgets::shows::season_folders_for(show_id) {
        if !f.path.is_empty() {
            put(f.season, f.path, &mut by);
        }
    }
    if by.is_empty() {
        if let Some(t) = title {
            for s in &t.seasons {
                put(s.number, String::new(), &mut by);
            }
        }
    }
    by.sort_by(|a, b| match (a.0 == 0, b.0 == 0) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => a.0.cmp(&b.0),
    });
    by
}

/// `S01E02` or `1x02` in the label, when the item names no numbers (player.ts `episodeNumbers`).
fn episode_numbers(item: &BrowseItem, folder: Option<u32>) -> Option<(Option<u32>, u32)> {
    if item.kind == ItemKind::Run || item.path.is_empty() {
        return None;
    }
    let info = item.info.as_ref();
    let mut season = item.season.or(info.and_then(|i| i.season)).or(folder);
    let mut episode = item.episode.or(info.and_then(|i| i.episode));
    if episode.is_none_or(|e| e == 0) {
        let label = format!("{} {}", item.label, info.and_then(|i| i.title.clone()).unwrap_or_default());
        let (s, e) = coded(&label)?;
        if item.season.is_none() && info.and_then(|i| i.season).is_none() {
            season = Some(s);
        }
        episode = Some(e);
    }
    let e = episode.filter(|e| *e >= 1)?;
    Some((season, e))
}

/// `S(\d{1,2})\s*E(\d{1,3})`, else `(\d{1,2})\s*[x×]\s*(\d{1,3})`, case-insensitively.
fn coded(label: &str) -> Option<(u32, u32)> {
    let c: Vec<char> = label.chars().collect();
    let digits = |i: usize, max: usize| -> Option<(u32, usize)> {
        let n = c[i..].iter().take(max + 1).take_while(|ch| ch.is_ascii_digit()).count();
        (1..=max).contains(&n).then(|| (c[i..i + n].iter().collect::<String>().parse().unwrap_or(0), i + n))
    };
    let ws = |mut i: usize| {
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        i
    };
    for i in 0..c.len() {
        if c[i].eq_ignore_ascii_case(&'s') && i + 1 < c.len() {
            if let Some((s, j)) = digits(i + 1, 2) {
                let j = ws(j);
                if j < c.len() && c[j].eq_ignore_ascii_case(&'e') {
                    if let Some((e, _)) = digits(j + 1, 3) {
                        return Some((s, e));
                    }
                }
            }
        }
    }
    for i in 0..c.len() {
        if c[i].is_ascii_digit() && (i == 0 || !c[i - 1].is_ascii_digit() || true) {
            if let Some((s, j)) = digits(i, 2) {
                let j = ws(j);
                if j < c.len() && (c[j] == 'x' || c[j] == 'X' || c[j] == '×') {
                    let k = ws(j + 1);
                    if let Some((e, _)) = digits(k, 3) {
                        return Some((s, e));
                    }
                }
            }
        }
    }
    None
}

fn progress_for<'a>(title: Option<&'a TitleDetail>, path: &str, season: Option<u32>, episode: u32) -> Option<&'a Episode> {
    let title = title?;
    let mut by_number = None;
    for g in &title.seasons {
        for e in &g.episodes {
            if !path.is_empty() && e.path == path {
                return Some(e);
            }
            if by_number.is_none() && Some(e.season) == season && e.episode == episode {
                by_number = Some(e);
            }
        }
    }
    by_number
}

fn from_library(title: &TitleDetail, show: &str, season: Option<u32>) -> Vec<Row> {
    let mut rows: Vec<Row> = title
        .seasons
        .iter()
        .filter(|g| season.is_none_or(|s| g.number == s))
        .flat_map(|g| g.episodes.iter())
        .map(|e| {
            let name = clean_episode_title(&e.name, Some(e.season), Some(e.episode), Some(show));
            Row {
                show_id: title.id.clone(),
                show_name: show.to_owned(),
                file: e.path.clone(),
                season: Some(e.season),
                episode: e.episode,
                name: if name.is_empty() { e.name.clone() } else { name },
                art: e.art.clone().or(title.art.clone()),
                info: None,
                position: e.position,
                duration: e.duration,
                finished: e.finished,
                resumable: e.resumable,
            }
        })
        .collect();
    rows.sort_by_key(|r| r.episode);
    rows
}

fn from_browse(page: &BrowsePage, folder: Option<u32>, title: Option<&TitleDetail>, show_id: &str, show: &str) -> Vec<Row> {
    let mut rows: Vec<Row> = page
        .items
        .iter()
        .filter_map(|item| {
            let (season, episode) = episode_numbers(item, folder)?;
            let known = progress_for(title, &item.path, season, episode);
            let raw = item.info.as_ref().and_then(|i| i.title.clone()).filter(|t| !t.is_empty()).unwrap_or(item.label.clone());
            let mut name = clean_episode_title(&raw, season, Some(episode), Some(show));
            if name.is_empty() {
                name = if item.label.is_empty() { episode_code(season, Some(episode)) } else { item.label.clone() };
            }
            Some(Row {
                show_id: show_id.to_owned(),
                show_name: show.to_owned(),
                file: item.path.clone(),
                season,
                episode,
                name,
                art: item.art.clone().or(known.and_then(|k| k.art.clone())).or(title.and_then(|t| t.art.clone())),
                info: item.info.clone(),
                position: known.map_or(0.0, |k| k.position),
                duration: known.map_or(0.0, |k| k.duration),
                finished: known.is_some_and(|k| k.finished),
                resumable: known.is_some_and(|k| k.resumable),
            })
        })
        .collect();
    rows.sort_by(|a, b| a.episode.cmp(&b.episode).then(a.name.cmp(&b.name)));
    rows
}

fn show_heading(d: &Dock, title: Option<&TitleDetail>) -> String {
    let t = current(d);
    [human_title(t.as_ref().and_then(|t| t.show_title.as_deref())), human_title(title.map(|t| t.name.as_str())), human_title(t.as_ref().map(|t| t.progress_name.as_str()))]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

/// Asks for the playing season, unless the viewer is browsing another one; never calls Kodi
/// while a play resolves (player.ts `requestEpisodes`).
pub fn request(d: &Dock) {
    let e = &d.inner().episodes;
    if e.hold.get() || !theater_on(d) {
        return;
    }
    let show = show_id_of(current(d).as_ref());
    if show.is_empty() {
        return;
    }
    let playing = current(d).and_then(|t| t.season);
    if d.resolving() {
        e.wait.set(true);
        let listed = e.listed.get_untracked().unwrap_or_default();
        if *e.listed_show.borrow() != show || listed.rows.is_empty() {
            note(d, &show, "Episodes will list when this starts playing.");
        }
        return;
    }
    e.wait.set(false);
    let follow = *e.listed_show.borrow() != show || playing != e.followed_season.get();
    let mut season = if follow { playing } else { e.listed_season.get().or(playing) };
    if !follow && e.queued_season.get().is_some() {
        season = e.queued_season.get();
    }
    if follow {
        e.queued_season.set(None);
    }
    fill(d, show, season, follow);
}

/// A resolve finished: the list that waited for it loads (player.ts `clearPlayActivity`).
pub fn after_resolve(d: &Dock) {
    let e = &d.inner().episodes;
    if e.wait.get() && !e.hold.get() {
        e.wait.set(false);
        request(d);
    }
}

fn note(d: &Dock, show: &str, text: &str) {
    d.inner().episodes.listed.update(|l| {
        l.show_id = show.to_owned();
        l.rows.clear();
        l.note = text.to_owned();
    });
}

/// This load lost the race to a resolve: retry when it clears (player.ts `deferEpisodeLoad`).
fn defer(d: &Dock, ticket: u64, show: &str, refetch: bool) {
    let e = &d.inner().episodes;
    if ticket != e.ticket.get() {
        return;
    }
    if refetch {
        e.reload.set(true);
    }
    if !d.resolving() || !theater_on(d) || show_id_of(current(d).as_ref()) != show {
        return;
    }
    e.wait.set(true);
}

/// Loads one season (player.ts `fillEpisodes`); `follow` when playback moved to another show
/// or season, so a season the viewer opened stays put.
fn fill(d: &Dock, show: String, season: Option<u32>, follow: bool) {
    let e = &d.inner().episodes;
    let ticket = e.ticket.get() + 1;
    e.ticket.set(ticket);
    let cached = e.titles.borrow().get(&show).cloned();
    match cached {
        Some(title) => fill_with(d, ticket, show, season, follow, title),
        None => {
            let (s2, w) = (show.clone(), Rc::downgrade(&d.0));
            d.inner().ctx.api.run(api::title(&show), &remedy_ui::reactive::Cancel::new(), move |out| {
                let Some(i) = w.upgrade() else { return };
                let d = Dock(i);
                let mine = mine(&d, ticket, &s2);
                let title = match out {
                    Ok(t) => Some(t),
                    Err(_) if !mine => return defer(&d, ticket, &s2, false),
                    Err(err) if err.status == Some(404) => None,
                    Err(err) => {
                        d.set_status(&err.detail);
                        note(&d, &s2, "Episodes could not be loaded.");
                        return;
                    }
                };
                if !mine {
                    return defer(&d, ticket, &s2, false);
                }
                d.inner().episodes.titles.borrow_mut().insert(s2.clone(), title.clone());
                fill_with(&d, ticket, s2, season, follow, title);
            });
        }
    }
}

fn mine(d: &Dock, ticket: u64, show: &str) -> bool {
    let e = &d.inner().episodes;
    ticket == e.ticket.get() && !e.hold.get() && !d.resolving() && theater_on(d) && show_id_of(current(d).as_ref()) == show
}

fn fill_with(d: &Dock, ticket: u64, show: String, season: Option<u32>, follow: bool, title: Option<TitleDetail>) {
    if !mine(d, ticket, &show) {
        return defer(d, ticket, &show, false);
    }
    let e = &d.inner().episodes;
    let choices = season_choices(title.as_ref(), &show);
    let selected = match season {
        Some(s) if choices.is_empty() || choices.iter().any(|c| c.0 == s) => Some(s),
        _ => choices.first().map(|c| c.0).or(season),
    };
    let refetch = e.reload.replace(false);
    let listed = e.listed.get_untracked().unwrap_or_default();
    let chips: Vec<u32> = choices.iter().map(|c| c.0).collect();
    if !refetch && *e.listed_show.borrow() == show && e.listed_season.get() == selected && !listed.rows.is_empty() {
        if follow {
            e.followed_season.set(current(d).and_then(|t| t.season));
        }
        e.queued_season.set(None);
        e.listed.update(|l| {
            l.chips = chips;
            l.selected = selected;
        });
        return;
    }
    let show_name = show_heading(d, title.as_ref());
    let heading = selected.map_or_else(|| "Episodes".to_owned(), season_label);
    e.listed.update(|l| {
        l.show = show_name.clone();
        l.heading = heading.clone();
        l.chips = chips.clone();
        l.selected = selected;
    });
    let path = choices.iter().find(|c| Some(c.0) == selected).map(|c| c.1.clone()).filter(|p| !p.is_empty());
    let finish = move |d: &Dock, rows: Vec<Row>, note_text: String| {
        if !mine(d, ticket, &show) {
            return defer(d, ticket, &show, refetch);
        }
        let e = &d.inner().episodes;
        *e.listed_show.borrow_mut() = show.clone();
        e.listed_season.set(selected);
        if follow {
            e.followed_season.set(current(d).and_then(|t| t.season));
        }
        if e.queued_season.get() == selected {
            e.queued_season.set(None);
        }
        e.listed.set(Listed {
            show: show_name.clone(),
            heading: heading.clone(),
            show_id: show.clone(),
            chips: chips.clone(),
            selected,
            note: if rows.is_empty() && note_text.is_empty() { "No episodes listed for this season.".into() } else { note_text },
            rows,
        });
    };
    let library = move |title: &Option<TitleDetail>, name: &str| -> (Vec<Row>, String) {
        let rows = title.as_ref().map(|t| from_library(t, name, selected)).unwrap_or_default();
        let note = if rows.is_empty() { String::new() } else { "Only episodes Portal has already opened are listed.".into() };
        (rows, note)
    };
    let Some(path) = path else {
        let (rows, n) = library(&title, &show_heading(d, title.as_ref()));
        return finish(d, rows, n);
    };
    let cached = e.pages.borrow().get(&path).cloned();
    let with_page = move |d: &Dock, page: BrowsePage| {
        let show_id = show_id_of(current(d).as_ref());
        let name = show_heading(d, title.as_ref());
        let mut rows = from_browse(&page, selected, title.as_ref(), &show_id, &name);
        let mut n = String::new();
        if rows.is_empty() {
            (rows, n) = library(&title, &name);
        }
        finish(d, rows, n);
    };
    if let Some(page) = cached {
        return with_page(d, page);
    }
    let (w, p2) = (Rc::downgrade(&d.0), path.clone());
    let show2 = show_id_of(current(d).as_ref());
    d.inner().ctx.api.run(api::browse(&path), &remedy_ui::reactive::Cancel::new(), move |out| {
        let Some(i) = w.upgrade() else { return };
        let d = Dock(i);
        match out {
            Ok(page) => {
                d.inner().episodes.pages.borrow_mut().insert(p2, page.clone());
                with_page(&d, page);
            }
            Err(err) => {
                if !mine(&d, ticket, &show2) {
                    return defer(&d, ticket, &show2, refetch);
                }
                d.set_status(&err.detail);
                note(&d, &show2, "Episodes could not be loaded.");
            }
        }
    });
}

/// player.ts `playListed`.
fn play_row(d: &Dock, row: &Row) {
    let cur = current(d);
    if cur.as_ref().is_some_and(|c| !row.file.is_empty() && c.file == row.file) {
        return;
    }
    let show = [
        human_title(Some(&row.show_name)),
        human_title(cur.as_ref().and_then(|c| c.show_title.as_deref())),
        human_title(cur.as_ref().map(|c| c.progress_name.as_str())),
    ]
    .into_iter()
    .find(|s| !s.is_empty())
    .unwrap_or(row.show_name.clone());
    let resume = (row.resumable && row.position > 60.0).then_some(row.position);
    d.play(PlayTarget {
        kind: PlayKind::Episode,
        id: row.show_id.clone(),
        file: row.file.clone(),
        title: display_media_title(&row.name, row.season, Some(row.episode), Some(&show)),
        season: row.season,
        episode: Some(row.episode),
        art: row.art.clone().or(cur.and_then(|c| c.art)),
        show_title: Some(show.clone()),
        show_id: Some(row.show_id.clone()),
        info: row.info.clone(),
        resume_seconds: resume,
        manual_links: false,
        progress_kind: TitleKind::Tv,
        progress_id: row.show_id.clone(),
        progress_name: show,
        track_progress: true,
    });
}

/// A season chip: loads that season, or waits for the resolve that owns Kodi.
fn pick_season(d: &Dock, show: &str, season: u32) {
    if show_id_of(current(d).as_ref()) != show {
        return;
    }
    let e = &d.inner().episodes;
    e.queued_season.set(Some(season));
    if d.resolving() || e.hold.get() {
        e.wait.set(true);
        return;
    }
    fill(d, show.to_owned(), Some(season), false);
}

/// The rotated "EPISODES" on the tab (`writing-mode: vertical-rl; rotate(180deg)`).
fn vertical_label(ui: &Ui, text: &'static str) -> El {
    let size = Signal::new((14.0_f32, 70.0_f32));
    let el = ui.el("p_theater_tab_label").pass_through();
    let e2 = el.clone();
    el.bind(move || {
        let (w, h) = size.get();
        let _ = e2.clone().width(w).height(h);
    })
    .paint(move |cx| {
        let mut style = TextStyle::default();
        style.families = cx.theme.families(false, false);
        style.size = 11.52;
        style.weight = 600.0;
        style.letter_spacing = 11.52 * 0.14;
        style.color = cx.style.foreground;
        let mut opts = ParagraphOptions::default();
        opts.max_width = None;
        opts.scale = cx.scale;
        let para = cx.layout_text(&RichText::plain(text.to_uppercase()), &style, opts);
        let (pw, ph) = (para.width(), para.height());
        if (size.get_untracked().unwrap_or_default().1 - pw).abs() > 0.5 {
            let _ = size.try_set((ph, pw));
        }
        // Read bottom to top: turned a quarter left about the box's bottom-left corner.
        cx.list.push(remedy_ui::paint::Item::PushTransform(
            Affine::rotate(-std::f32::consts::FRAC_PI_2).then(Affine::translate(Vec2::new(0.0, cx.size.height))),
        ));
        cx.draw_paragraph(&para, Point::new(0.0, 0.0), cx.style.foreground);
        cx.list.push(remedy_ui::paint::Item::PopTransform);
    })
}

/// `.theater-tab`: opens and closes the list.
fn tab(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let open = d.inner().episodes.open;
    let left = glyph(ui, Glyph::ChevronLeft, 18.4);
    let right = glyph(ui, Glyph::ChevronRight, 18.4);
    let (l2, r2) = (left.clone(), right.clone());
    let me = d.clone();
    let b = ui
        .el("p_theater_tab")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        .gap(12.0)
        .width(39.2)
        .layout(|l| {
            l.shrink = 0.0;
            l.height = Length::Percent(1.0);
        })
        .child(left)
        .child(right)
        .child(vertical_label(ui, "Episodes"))
        // border-right: 1px solid var(--line).
        .child(ui.el("p_theater_rule").layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: 0.0, bottom: 0.0, start: remedy_ui::layout::AUTO, end: 0.0 };
            l.width = Length::Px(1.0);
        }))
        .on_click(move || set_open(&me, !me.inner().episodes.open.get_untracked().unwrap_or(true)))
        .semantics(move || Semantics::new(Role::Button).label("Episodes").expanded(open.get()).action(Action::Click));
    remedy_ui::widgets::tooltip(&b, move || if open.get() { "Hide episodes".to_owned() } else { "Show episodes".to_owned() });
    let b2 = b.clone();
    b.bind(move || {
        let on = open.get();
        let _ = l2.clone().hidden(!on);
        let _ = r2.clone().hidden(on);
        b2.state(States::SELECTED, on);
    })
}

/// player.ts `setEpisodesOpen`: a closed list keeps its rows out of the tab order; focus that
/// was inside goes to the tab.
fn set_open(d: &Dock, on: bool) {
    let e = &d.inner().episodes;
    e.open.set_if_changed(on);
    if on {
        e.scrolled.borrow_mut().clear();
    }
}

/// The side column: the tab, and the list when open.
pub fn side(d: &Dock) -> El {
    let i = d.inner();
    let ui = &i.ui;
    let e = &i.episodes;
    let (open, listed, width) = (e.open, e.listed, i.ctx.width);
    let show = ui.el("p_theater_show").max_lines(1).text(move || RichText::plain(listed.with(|l| l.show.clone())));
    let s2 = show.clone();
    let show = show.bind(move || {
        let _ = s2.clone().hidden(listed.with(|l| l.show.is_empty()));
    });
    let heading = ui
        .el("p_theater_heading")
        .text(move || RichText::plain(listed.with(|l| if l.heading.is_empty() { "Episodes".into() } else { l.heading.clone() })))
        .semantics(move || Semantics::new(Role::Heading(3)).label(listed.with(|l| if l.heading.is_empty() { "Episodes".to_owned() } else { l.heading.clone() })));
    let head = ui.el("p_theater_head").column().layout(|l| l.shrink = 0.0).child(show).child(heading);
    let chips = ui
        .el("p_theater_seasons")
        .row()
        .gap(5.6)
        .scroll()
        .scroll_x()
        .layout(|l| {
            l.shrink = 0.0;
            l.max_height = Length::Auto;
        })
        .semantics(Semantics::new(Role::TabList).label("Seasons"));
    let (c2, u2, me) = (chips.clone(), ui.clone(), d.clone());
    let chips = chips.bind(move || {
        let l = listed.get();
        untrack(|| {
            c2.clear();
            let _ = c2.clone().hidden(l.chips.len() < 2);
            if l.chips.len() < 2 {
                return;
            }
            let n = l.chips.len();
            for (k, s) in l.chips.iter().copied().enumerate() {
                let on = l.selected == Some(s);
                let (me2, show) = (me.clone(), l.show_id.clone());
                let (me3, show3, all) = (me.clone(), l.show_id.clone(), l.chips.clone());
                let chip = u2
                    .el("p_theater_chip")
                    .text(season_label(s))
                    .nowrap()
                    .focusable(on)
                    .on_click(move || pick_season(&me2, &show, s))
                    .on_key(move |ev| {
                        // Left and right change season (player.ts `moveTheaterFocus`).
                        if !ev.pressed {
                            return false;
                        }
                        let to = match ev.key {
                            Key::ArrowLeft if k > 0 => all[k - 1],
                            Key::ArrowRight if k + 1 < all.len() => all[k + 1],
                            _ => return false,
                        };
                        pick_season(&me3, &show3, to);
                        true
                    })
                    .semantics(Semantics::new(Role::Tab).label(season_label(s)).selected(on).set_position(k + 1, n).action(Action::Click));
                chip.state(States::SELECTED, on);
                let _ = c2.clone().child(chip);
            }
        });
    });
    let list = ui
        .el("p_theater_list")
        .column()
        .scroll()
        .layout(|l| {
            l.grow = 1.0;
            l.shrink = 1.0;
            l.min_height = Length::Px(0.0);
            l.max_height = Length::Auto;
        });
    let (l2, u3, me) = (list.clone(), ui.clone(), d.clone());
    let list = list.bind(move || {
        let l = listed.get();
        let playing = me.inner().target.get();
        untrack(|| paint_rows(&me, &u3, &l2, &l, playing.as_ref()));
    });
    let aside = ui
        .el("p_theater_episodes")
        .column()
        .clip()
        .semantics(Semantics::new(Role::Complementary).label("Episodes"))
        .child(head)
        .child(chips)
        .child(list)
        .child(ui.el("p_theater_rule").layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: 0.0, bottom: 0.0, start: remedy_ui::layout::AUTO, end: 0.0 };
            l.width = Length::Px(1.0);
        }));
    let me = d.clone();
    let tab_el = tab(d);
    let t2 = tab_el.clone();
    let aside = aside.on_key(move |ev| {
        // Escape collapses the list when focus is inside; the next Escape leaves theater.
        if ev.pressed && ev.key == Key::Escape && me.inner().episodes.open.get_untracked().unwrap_or(false) {
            set_open(&me, false);
            t2.focus();
            return true;
        }
        false
    });
    let a2 = aside.clone();
    let aside = aside.bind(move || {
        let on = open.get();
        let narrow = width.get() <= 720.0;
        let w = (width.get() * 0.38).min(384.0);
        let _ = a2.clone().hidden(!on);
        let _ = a2.clone().layout(move |l| {
            l.height = Length::Percent(1.0);
            if narrow {
                // On a phone the list lies over the picture, beside the tab.
                l.position = Position::Absolute;
                l.inset = Insets { top: 0.0, bottom: 0.0, start: 39.2, end: remedy_ui::layout::AUTO };
                l.width = Length::Px((width.get() - 39.2).min(352.0));
            } else {
                l.position = Position::Relative;
                l.width = Length::Px(w);
            }
            l.min_width = Length::Px(0.0);
        });
    });
    ui.el("p_theater_side")
        .row()
        .layout(|l| {
            l.height = Length::Percent(1.0);
            l.min_width = Length::Px(0.0);
            l.min_height = Length::Px(0.0);
        })
        .child(tab_el)
        .child(aside)
}

/// The rows, or the note (player.ts `paintEpisodeRows`, `markPlayingRow`).
fn paint_rows(d: &Dock, ui: &Ui, list: &El, l: &Listed, playing: Option<&PlayTarget>) {
    let focused_file = ui.tree().focused().map(|f| ui.element(f)).and_then(|f| {
        let mut at = Some(f);
        while let Some(e) = at {
            if let Some(file) = e.info().filter(|i| i.widget == "p_theater_ep").and_then(|i| i.semantics).and_then(|s| s.description_text().map(str::to_owned)) {
                return Some(file);
            }
            at = e.parent();
        }
        None
    });
    list.clear();
    if !l.note.is_empty() || l.rows.is_empty() {
        let _ = list.clone().child(ui.el("p_theater_note").text(if l.note.is_empty() { "No episodes listed for this season.".to_owned() } else { l.note.clone() }));
    }
    let file = playing.map(|p| p.file.clone()).unwrap_or_default();
    let (season, episode) = (playing.and_then(|p| p.season), playing.and_then(|p| p.episode));
    let by_file = l.rows.iter().position(|r| !file.is_empty() && r.file == file);
    let by_number = l.rows.iter().position(|r| season.is_some() && r.season == season && Some(r.episode) == episode);
    let current = by_file.or(by_number);
    let mut current_el = None;
    let mut refocus = None;
    let n = l.rows.len();
    let rows: Rc<Vec<El>> = Rc::new(Vec::new());
    let mut built = Vec::with_capacity(n);
    for (k, r) in l.rows.iter().enumerate() {
        let el = row_el(d, ui, r, current == Some(k));
        if current == Some(k) {
            current_el = Some(el.clone());
        }
        if focused_file.as_deref() == Some(r.file.as_str()) {
            refocus = Some(el.clone());
        }
        built.push(el);
    }
    let _ = rows;
    let all = Rc::new(built.clone());
    for (k, el) in built.into_iter().enumerate() {
        let a = all.clone();
        // Up and down move through episodes; they do not seek the video.
        let el = el.on_key(move |ev| {
            if !ev.pressed {
                return false;
            }
            let to = match ev.key {
                Key::ArrowUp if k > 0 => k - 1,
                Key::ArrowDown if k + 1 < a.len() => k + 1,
                Key::ArrowUp | Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight => return true,
                _ => return false,
            };
            a[to].focus();
            a[to].scroll_into_view();
            true
        });
        let _ = list.clone().child(el);
    }
    if let Some(f) = refocus.or(if focused_file.is_some() { current_el.clone() } else { None }) {
        f.focus();
    }
    // Centre the playing row once per item, after the list has its width.
    let key = format!("{}\n{}\n{:?}\n{:?}", l.show_id, file, season, episode);
    let e = &d.inner().episodes;
    if let Some(c) = current_el {
        if *e.scrolled.borrow() != key {
            *e.scrolled.borrow_mut() = key;
            let (u, list) = (ui.clone(), list.clone());
            ui.after(std::time::Duration::from_millis(260), move || {
                if let (Some(lb), Some(cb)) = (list.bounds(), c.bounds()) {
                    let at = u.scroll_offset(list.id());
                    let top = cb.min.y - lb.min.y;
                    let y = at.y + top - lb.height() / 2.0 + cb.height() / 2.0;
                    u.set_scroll_offset(list.id(), Vec2::new(at.x, y.max(0.0)));
                }
            });
        }
    }
}

/// `.theater-ep`: the still with its progress, the code, the name, Resume or Watched.
fn row_el(d: &Dock, ui: &Ui, r: &Row, current: bool) -> El {
    let code = episode_code(r.season, Some(r.episode));
    let resume = r.resumable && r.position > 60.0;
    let action = if resume { "Resume" } else if r.finished { "Play again" } else { "Play" };
    let label = format!("{action} {}", [code.as_str(), r.name.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · "));
    let mut still = crate::widgets::episode_row::still(ui, r.art.as_deref(), &r.name, 22.0).variant("theater");
    if !r.finished && r.duration > 0.0 && r.position > 0.0 {
        let f = (r.position / r.duration).clamp(0.0, 1.0) as f32;
        still = still.child(
            ui.el("p_theater_bar")
                .layout(|l| {
                    l.position = Position::Absolute;
                    l.inset = Insets { top: remedy_ui::layout::AUTO, start: 0.0, end: 0.0, bottom: 0.0 };
                    l.height = Length::Px(3.0);
                })
                .child(ui.el("p_theater_fill").layout(move |l| {
                    l.width = Length::Percent(f);
                    l.height = Length::Percent(1.0);
                })),
        );
    }
    let meta = if resume { format!("Resume {}", format_clock(r.position)) } else if r.finished { "Watched".into() } else { String::new() };
    let mut text = ui
        .el("p_theater_text")
        .column()
        .layout(|l| l.min_width = Length::Px(0.0))
        .child(ui.el("p_theater_code").variant(if current { "current" } else { "" }).text(code.clone()))
        .child(ui.el("p_theater_name").max_lines(2).text(if r.name.is_empty() { if code.is_empty() { "Episode".to_owned() } else { code.clone() } } else { r.name.clone() }));
    if !meta.is_empty() {
        text = text.child(ui.el("p_theater_meta").text(meta));
    }
    let (me, row) = (d.clone(), r.clone());
    let el = ui
        .el("p_theater_ep")
        .variant(if r.finished { "watched" } else { "" })
        .layout(|l| {
            l.display = Display::Grid;
            l.columns = vec![Track::Px(89.6), Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0)))];
            l.gap = (8.8, 8.8);
            l.align_items = Align::Center;
            l.margin.bottom = 4.8;
        })
        .child(still)
        .child(text)
        .on_click(move || play_row(&me, &row))
        .semantics(Semantics::new(Role::Button).label(label).description(r.file.clone()).action(Action::Click));
    el.state(States::SELECTED, current);
    el
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    s.rule("p_theater_side", None, States::NONE, StylePatch::default().background(k.raised));
    // .theater-tab: muted on the raised surface, a hairline at its end; gold while open.
    s.rule("p_theater_tab", None, States::NONE, StylePatch::default().background(k.raised).foreground(k.muted).padding(Insets { top: 12.8, bottom: 12.8, start: 0.0, end: 0.0 }));
    s.rule("p_theater_rule", None, States::NONE, StylePatch::default().background(k.line));
    s.rule("p_theater_tab", None, States::HOVER, StylePatch::default().foreground(k.text));
    s.rule("p_theater_tab", None, States::SELECTED, StylePatch::default().foreground(k.gold));
    s.rule("p_theater_tab", None, States::FOCUS_VISIBLE, StylePatch::default().outline_offset(-2.0));
    s.rule("p_theater_episodes", None, States::NONE, StylePatch::default().background(k.raised));
    s.rule("p_theater_head", None, States::NONE, StylePatch::default().padding(Insets { top: 12.0, bottom: 5.6, start: 13.6, end: 13.6 }));
    s.rule(
        "p_theater_show",
        None,
        States::NONE,
        StylePatch::default().foreground(k.muted).text_size(12.48).letter_spacing(LetterSpacing::Em(0.04)).text_transform(TextTransform::Upper),
    );
    s.rule(
        "p_theater_heading",
        None,
        States::NONE,
        StylePatch::default().heading(true).weight(400.0).text_size(16.0).line_height(LineHeight::Relative(1.25)).foreground(k.text).padding(Insets { top: 1.6, bottom: 0.0, start: 0.0, end: 0.0 }),
    );
    s.rule("p_theater_seasons", None, States::NONE, StylePatch::default().padding(Insets { top: 5.6, bottom: 8.0, start: 12.0, end: 12.0 }));
    s.rule(
        "p_theater_chip",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.muted)
            .text_size(13.12)
            .radius(999.0)
            .border(k.line)
            .border_width(1.0)
            .padding(Insets { top: 3.52, bottom: 3.52, start: 10.4, end: 10.4 })
            .outline_offset(-2.0),
    );
    s.rule("p_theater_chip", None, States::HOVER, StylePatch::default().border(k.gold));
    s.rule("p_theater_chip", None, States::SELECTED, StylePatch::default().background(k.gold).border(k.gold).foreground(k.bg).weight(600.0));
    s.rule("p_theater_list", None, States::NONE, StylePatch::default().padding(Insets { top: 3.2, bottom: 11.2, start: 7.2, end: 7.2 }));
    s.rule("p_theater_note", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.08).padding(Insets { top: 6.4, bottom: 9.6, start: 6.4, end: 6.4 }));
    s.rule(
        "p_theater_ep",
        None,
        States::NONE,
        StylePatch::default().border(clear).border_width(1.0).radius(10.0).padding(Insets::all(5.6)).foreground(k.text).outline_offset(-2.0),
    );
    s.rule("p_theater_ep", Some("watched"), States::NONE, StylePatch::default().border(clear).border_width(1.0).radius(10.0).padding(Insets::all(5.6)).foreground(k.muted).outline_offset(-2.0));
    for v in [None, Some("watched")] {
        s.rule("p_theater_ep", v, States::HOVER, StylePatch::default().border(mix(k.gold, 0.45, k.line)).background(mix(k.text, 0.05, clear)));
        s.rule("p_theater_ep", v, States::SELECTED, StylePatch::default().border(k.gold).background(mix(k.gold, 0.14, k.bg)));
    }
    s.rule("p_theater_bar", None, States::NONE, StylePatch::default().background(k.track));
    s.rule("p_theater_fill", None, States::NONE, StylePatch::default().background(k.gold));
    s.rule("p_theater_code", None, States::NONE, StylePatch::default().foreground(k.gold).text_size(12.0).letter_spacing(LetterSpacing::Em(0.03)));
    s.rule("p_theater_code", Some("current"), States::NONE, StylePatch::default().foreground(k.gold).text_size(12.0).letter_spacing(LetterSpacing::Em(0.03)).weight(700.0));
    s.rule("p_theater_name", None, States::NONE, StylePatch::default().text_size(14.72).line_height(LineHeight::Relative(1.3)).padding(Insets { top: 0.8, bottom: 0.0, start: 0.0, end: 0.0 }));
    s.rule("p_theater_meta", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(12.0).padding(Insets { top: 1.92, bottom: 0.0, start: 0.0, end: 0.0 }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_in_labels_as_player_ts() {
        assert_eq!(coded("Show S02E05 Name"), Some((2, 5)));
        assert_eq!(coded("Show s2 e15"), Some((2, 15)));
        assert_eq!(coded("3x04 Pilot"), Some((3, 4)));
        assert_eq!(coded("1×10"), Some((1, 10)));
        assert_eq!(coded("Pilot"), None);
        assert_eq!(season_label(0), "Specials");
    }
}
