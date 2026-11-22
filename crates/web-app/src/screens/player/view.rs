//! The dock's layout (index.html `#dock`, styles.css `.dock`, `.dock-side`, theater and
//! `.dock.mini`): the stage with the player widget, the side panel, the theater grid with
//! the episodes panel, the mini player in the corner, and the full-screen stage.

use std::cell::Cell;
use std::rc::Rc;

use portal_api::{PlayKind, Quality, TitleKind};
use remedy_ui::a11y::{Action, Live, Role, Semantics};
use remedy_ui::core::{Color, CornerRadii, Insets};
use remedy_ui::layout::{Align, Display, Length, Position, Track, AUTO};
use remedy_ui::playback::{player_with, NextUpPlacement, PlayerParts};
use remedy_ui::reactive::{untrack, Signal};
use remedy_ui::style::{LineHeight, Shadow, States, StylePatch, StyleSheet};
use remedy_ui::view::RichText;
use remedy_ui::widgets::{self as w, Icon};
use remedy_ui::{El, Ui};

use super::{episodes, human_title, next_detail, next_is_episode, Dock};
use crate::api::List;
use crate::theme::{mix, Look};
use crate::widgets::glyphs::{glyph, Glyph};
use crate::widgets::play::PlayTarget;
use crate::widgets::saved::{self, SaveTitle};
use crate::widgets::titles::episode_code;
use crate::DockView;

/// The narrow layout's width (styles.css `@media (max-width: 720px)`).
const NARROW: f32 = 720.0;

/// How the dock is laid out now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    Theater,
    Mini,
    Full,
}

/// titles.ts `splitTitle`: the show on top and the episode below it; a movie is one line.
pub fn split_title(raw: &str, season: Option<u32>, episode: Option<u32>, show: Option<&str>) -> (String, String) {
    let series = show.map(str::trim).unwrap_or("");
    if season.is_some() && episode.is_some_and(|e| e > 0) {
        let described = super::display_media_title(raw, season, episode, Some(series));
        return if series.is_empty() { (described, String::new()) } else { (series.to_owned(), described) };
    }
    (raw.trim().to_owned(), String::new())
}

/// player.ts `setDockTitle`: the heading and the episode line under it.
fn dock_titles(t: Option<&PlayTarget>, title: &str) -> (String, String) {
    let Some(t) = t else { return (String::new(), String::new()) };
    let (heading, detail) = split_title(title, t.season, t.episode, t.show_title.as_deref());
    let episode = t.season.is_some() && t.episode.is_some_and(|e| e > 0);
    let heading = if episode {
        heading
    } else {
        let h = human_title(Some(title));
        if h.is_empty() { title.trim().to_owned() } else { h }
    };
    (heading, detail)
}

/// The full-screen title over the picture (`#dock-stage-title`): heading · episode.
pub fn stage_title(t: &PlayTarget, title: &str) -> String {
    let (h, d) = dock_titles(Some(t), title);
    if d.is_empty() { h } else { format!("{h} · {d}") }
}

/// What the side panel's Watchlist and Bookmark save (player.ts `saveTarget`).
fn save_target(t: Option<&PlayTarget>) -> Option<SaveTitle> {
    let t = t?;
    match t.kind {
        PlayKind::Episode => {
            let id = t.show_id.clone().filter(|s| !s.is_empty()).unwrap_or(t.id.clone());
            let mut name = human_title(t.show_title.as_deref());
            if name.is_empty() {
                name = human_title(Some(&t.progress_name));
            }
            (!id.is_empty() && !name.is_empty()).then(|| SaveTitle { id, kind: TitleKind::Tv, name, art: t.art.clone(), year: None, imdb_id: None })
        }
        PlayKind::Movie => {
            let h = human_title(Some(&t.title));
            let name = if h.is_empty() { t.title.trim().to_owned() } else { h };
            let info = t.info.as_ref();
            (!t.id.is_empty() && !name.is_empty()).then(|| SaveTitle {
                id: t.id.clone(),
                kind: TitleKind::Movie,
                name,
                art: t.art.clone(),
                year: info.and_then(|i| i.year),
                imdb_id: info.and_then(|i| i.imdb_id.clone()),
            })
        }
        _ => None,
    }
}

/// What Info shows for the target before the lookup answers (player.ts `infoSeed`).
fn info_seed(t: &PlayTarget) -> portal_api::MediaInfo {
    let episode = t.kind == PlayKind::Episode && t.season.is_some() && t.episode.is_some_and(|e| e > 0);
    let show = human_title(t.show_title.as_deref());
    let mut base = portal_api::MediaInfo::default();
    if episode {
        base.kind = Some(portal_api::MediaKind::Episode);
        let clean = crate::widgets::titles::clean_episode_title(&t.title, t.season, t.episode, Some(&show));
        base.title = Some(if clean.is_empty() { t.title.clone() } else { clean });
        base.show_title = (!show.is_empty()).then_some(show);
        base.season = t.season;
        base.episode = t.episode;
    } else {
        base.kind = (t.kind == PlayKind::Movie).then_some(portal_api::MediaKind::Movie);
        let h = human_title(Some(&t.title));
        base.title = Some(if h.is_empty() { t.title.clone() } else { h });
    }
    match &t.info {
        Some(extra) => crate::screens::info::merge(&base, extra),
        None => base,
    }
}

/// Whether keyboard focus is somewhere in the dock.
pub fn dock_has_focus(d: &Dock) -> bool {
    let ui = &d.inner().ui;
    let Some(f) = ui.tree().focused() else { return false };
    let mut at = Some(ui.element(f));
    while let Some(e) = at {
        if e.info().is_some_and(|i| i.widget == "p_dock") {
            return true;
        }
        at = e.parent();
    }
    false
}

/// The first shown descendant of `root` made as `widget`.
fn find(root: &El, widget: &str) -> Option<El> {
    let mut stack = vec![root.clone()];
    while let Some(e) = stack.pop() {
        if e.info().is_some_and(|i| i.widget == widget && !i.hidden) {
            return Some(e);
        }
        stack.extend(e.child_elements().into_iter().rev());
    }
    None
}

/// Portal's glyphs for the player's icons.
fn player_icons(ui: &Ui, icon: Icon, size: f32) -> Option<El> {
    let g = match icon {
        Icon::Play => Glyph::Play,
        Icon::Pause => Glyph::Pause,
        Icon::Back10 => Glyph::Back10,
        Icon::Forward10 => Glyph::Forward10,
        Icon::Volume => Glyph::Volume,
        Icon::VolumeMuted => Glyph::VolumeMuted,
        Icon::Captions => Glyph::Captions,
        Icon::PictureInPicture => Glyph::Pip,
        Icon::Theater => Glyph::Theater,
        Icon::Fullscreen => Glyph::Fullscreen,
        Icon::FullscreenExit => Glyph::FullscreenExit,
        Icon::Close => Glyph::Close,
        _ => return None,
    };
    Some(glyph(ui, g, size))
}

/// A round glyph button of the player's bar (`.icon-btn` in `.chrome-row`).
fn bar_button(ui: &Ui, g: Glyph, label: impl Into<String>) -> El {
    let size = ui.theme().metric("player-glyph", 20.8);
    ui.el("player_button")
        .row()
        .align(Align::Center)
        .justify(remedy_ui::layout::Justify::Center)
        .child(glyph(ui, g, size))
        .semantics(Semantics::new(Role::Button).label(label.into()).action(Action::Click))
}

/// P9's seam: the subtitles control, disabled until subtitles are ported, where
/// index.html's `#dock-subs` sits (after the clock, before picture in picture).
fn captions_seam(ui: &Ui) -> El {
    let b = bar_button(ui, Glyph::Captions, "Subtitles off").disabled(true);
    w::tooltip(&b, "Subtitles off (c)");
    b
}

/// `#dock-pip`: shown where the browser has picture in picture, not on a phone-sized page,
/// disabled while a TV plays.
fn pip_button(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let p = d.inner().player.clone();
    let (p2, p3, width) = (p.clone(), p.clone(), d.inner().ctx.width);
    let b = bar_button(ui, Glyph::Pip, "Picture in picture").on_click(move || {
        if p2.is_pip() {
            p2.exit_pip();
        } else {
            p2.request_pip();
        }
    });
    w::tooltip(&b, "Picture in picture");
    let b2 = b.clone();
    b.bind(move || {
        let _ = b2.clone().hidden(!p3.pip_available() || width.get() <= NARROW);
        b2.state(States::CHECKED, p3.is_pip());
    })
}

/// A dock head button (`#dock-mini`, `#dock-close`): muted, the text color under the pointer.
fn head_button(ui: &Ui, g: Glyph, label: impl Into<String>, on: impl Fn() + 'static) -> El {
    let label = label.into();
    let b = ui
        .el("p_icon_btn")
        .variant("dock_head")
        .row()
        .align(Align::Center)
        .justify(remedy_ui::layout::Justify::Center)
        .width(36.0)
        .height(36.0)
        .layout(|l| l.shrink = 0.0)
        .child(glyph(ui, g, 20.8))
        .on_click(on)
        .semantics(Semantics::new(Role::Button).label(label.clone()).action(Action::Click));
    w::tooltip(&b, label);
    b
}

/// Minimize / Expand (one button, so focus stays on it).
fn mini_button(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let p = d.inner().player.clone();
    let (p2, p3) = (p.clone(), p.clone());
    let down = glyph(ui, Glyph::ChevronDown, 20.8);
    let up = glyph(ui, Glyph::ChevronUp, 20.8);
    let (d2, u2) = (down.clone(), up.clone());
    let b = ui
        .el("p_icon_btn")
        .variant("dock_head")
        .row()
        .align(Align::Center)
        .justify(remedy_ui::layout::Justify::Center)
        .width(36.0)
        .height(36.0)
        .layout(|l| l.shrink = 0.0)
        .child(down)
        .child(up)
        .on_click(move || p2.set_compact(!p2.is_compact()));
    let label = move || if p3.is_compact() { "Expand player" } else { "Minimize player" };
    let l2 = label.clone();
    w::tooltip(&b, move || l2().to_owned());
    let b = b.semantics(move || Semantics::new(Role::Button).label(label()).expanded(!p.is_compact()).action(Action::Click));
    let p4 = d.inner().player.clone();
    b.bind(move || {
        let mini = p4.is_compact();
        let _ = d2.clone().hidden(mini);
        let _ = u2.clone().hidden(!mini);
    })
}

fn close_button(d: &Dock) -> El {
    let me = d.clone();
    head_button(&d.inner().ui, Glyph::Close, "Close player", move || me.close())
}

/// The titles (`.dock-titles`): the heading and the episode line, as `variant` lays them.
fn titles(d: &Dock, variant: &'static str) -> El {
    let i = d.inner();
    let ui = &i.ui;
    let (target, title) = (i.target, i.title);
    let both = move || target.with(|t| dock_titles(t.as_ref(), &title.get()));
    let b2 = both.clone();
    let heading = ui
        .el("p_dock_title")
        .variant(variant)
        .text(move || RichText::plain(both().0))
        .semantics(move || Semantics::new(Role::Heading(2)).label(b2().0));
    let b3 = both.clone();
    w::tooltip(&heading, move || {
        let (h, d) = b3();
        if d.is_empty() { h } else { format!("{h} · {d}") }
    });
    let b4 = both.clone();
    let line = ui.el("p_dock_episode").variant(variant).text(move || RichText::plain(b4().1));
    let (l2, b5) = (line.clone(), both.clone());
    let line = line.bind(move || {
        let _ = l2.clone().hidden(b5().1.is_empty());
    });
    let (h2, l3) = (heading.clone(), line.clone());
    let (theater, pl) = (i.theater, i.player.clone());
    // In theater the titles take one line each, the heading at 1.05rem.
    let heading = heading.bind(move || {
        let wide = variant.is_empty() && theater.get() && pl.is_theater();
        let v = if wide { "theater" } else { variant };
        let lines = if variant.is_empty() && !wide { 2 } else { 1 };
        let _ = h2.clone().variant(v).max_lines(lines);
        let _ = l3.clone().variant(v).max_lines(lines);
    });
    ui.el("p_dock_titles").column().grow(1.0).layout(|l| l.min_width = Length::Px(0.0)).child(heading).child(line)
}

/// The status line (`#dock-status`): gold, polite, hidden while empty.
fn status_line(d: &Dock, variant: &'static str) -> El {
    let s = d.inner().status;
    let el = d.inner().ui.el("p_dock_status").variant(variant).text(move || RichText::plain(s.get()));
    let e2 = el.clone();
    el.semantics(move || Semantics::new(Role::Status).label(s.get()).live(Live::Polite)).bind(move || {
        let _ = e2.clone().hidden(s.with(String::is_empty));
    })
}

/// The Info button: what plays, disabled while nothing does.
fn info_button(d: &Dock) -> El {
    let i = d.inner();
    let ui = &i.ui;
    let target = i.target;
    let b = saved::frame(ui, "dock").child(glyph(ui, Glyph::Info, saved::glyph_size("dock")));
    let me = d.clone();
    let b2 = b.clone();
    let b = b
        .on_click(move || {
            let Some(t) = me.inner().target.get_untracked().ok().flatten() else { return };
            let req = crate::screens::info::InfoRequest {
                seed: info_seed(&t),
                art: t.art.clone(),
                save: save_target(Some(&t)),
                play: None,
                opener: Some(b2.clone()),
            };
            crate::screens::info::open(&me.inner().ui, &me.inner().ctx, req);
        })
        .disabled(move || target.with(Option::is_none))
        .semantics(Semantics::new(Role::Button).label("Info").action(Action::Click));
    w::tooltip(&b, move || {
        let name = target.with(|t| t.as_ref().map(|t| {
            let h = human_title(Some(&t.title));
            if h.is_empty() { t.title.clone() } else { h }
        }));
        match name.filter(|n| !n.is_empty()) {
            Some(n) => format!("Info about “{n}”"),
            None => "Info".into(),
        }
    });
    b
}

/// `.dock-head`: titles, Watchlist, Bookmark, Info, Minimize, Close.
fn head(d: &Dock) -> El {
    let i = d.inner();
    let ui = &i.ui;
    let save = Signal::new(None::<SaveTitle>);
    let target = i.target;
    let c = i.ctx.clone();
    let group = ui
        .el("p_dock_save")
        .row()
        .align(Align::Center)
        .semantics(Semantics::new(Role::Group).label("This title"))
        .child(saved::save_button(ui, &i.ctx, List::Watchlist, save, "dock"))
        .child(saved::save_button(ui, &i.ctx, List::Bookmarks, save, "dock"))
        .child(info_button(d))
        .bind(move || {
            let s = target.with(|t| save_target(t.as_ref()));
            if let Some(id) = s.as_ref().map(|s| s.id.clone()) {
                let want = id.clone();
                untrack(|| saved::load(&c, vec![id], move || save.get_untracked().ok().flatten().is_some_and(|s| s.id == want)));
            }
            save.set_if_changed(s);
        });
    ui.el("p_dock_head")
        .row()
        .align(Align::Start)
        .gap(8.0)
        .child(titles(d, ""))
        .child(group)
        .child(mini_button(d))
        .child(close_button(d))
}

/// The mini player's head (the player's compact title): titles, Expand, Close, the status.
fn mini_head(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let row = ui
        .el("p_dock_head")
        .variant("mini")
        .row()
        .align(Align::Center)
        .gap(3.2)
        .child(titles(d, "mini"))
        .child(mini_button(d))
        .child(close_button(d));
    ui.el("p_dock_side").variant("mini").column().gap(3.2).child(row).child(status_line(d, "mini"))
}

/// `#dock-next`: what is next, shortened past 36 characters.
fn next_button(d: &Dock) -> El {
    let i = d.inner();
    let next = i.next;
    let full = move || {
        next.with(|n| match n {
            None => ("Next episode".to_owned(), "Next episode".to_owned()),
            Some(n) => {
                let detail = next_detail(n);
                let lead = if next_is_episode(n) { "Next episode" } else { "Next" };
                let full = if detail.is_empty() { lead.to_owned() } else { format!("{lead} · {detail}") };
                let code = episode_code(n.season, n.episode);
                let compact = if next_is_episode(n) && !code.is_empty() { format!("Next · {code}") } else { lead.to_owned() };
                let shown = if full.chars().count() > 36 { compact } else { full.clone() };
                (shown, full)
            }
        })
    };
    let me = d.clone();
    let (f2, f3) = (full.clone(), full.clone());
    let b = crate::widgets::button(&i.ui, move || full().0, move || me.play_next())
        .variant("primary")
        .disabled(move || next.with(Option::is_none))
        .semantics(move || Semantics::new(Role::Button).label(f2().1).action(Action::Click));
    let _ = b.clone().max_lines(1).layout(|l| l.max_width = Length::Px(320.0));
    w::tooltip(&b, move || f3().1);
    b
}

/// `#dock-links`: the add-on's source list for what plays.
fn links_button(d: &Dock) -> El {
    let i = d.inner();
    let (target, job) = (i.target, i.job);
    let ready = move || target.with(|t| t.as_ref().is_some_and(|t| !t.file.is_empty())) && job.with(Option::is_some);
    let r2 = ready.clone();
    let r3 = ready.clone();
    let label = move || if r2() { "Choose a stream link" } else { "Links unavailable" };
    let (l2, l3) = (label.clone(), label.clone());
    let me = d.clone();
    let b = crate::widgets::button(&i.ui, move || label().to_owned(), move || me.play_with_links())
        .disabled(move || !r3())
        .semantics(move || Semantics::new(Role::Button).label(l2()).action(Action::Click));
    w::tooltip(&b, move || l3().to_owned());
    b
}

/// P10's seam: `#dock-cast`, disabled until casting is ported ([`Player::on_cast`]).
fn cast_button(d: &Dock) -> El {
    let b = crate::widgets::button(&d.inner().ui, "Cast", || {}).disabled(true);
    w::tooltip(&b, "Cast");
    b
}

/// The Quality menu: Auto, Source, 4K, 1080p, 720p, 480p.
fn quality_field(d: &Dock) -> El {
    const QUALITIES: [(Quality, &str); 6] = [
        (Quality::Auto, "Auto"),
        (Quality::Source, "Source"),
        (Quality::P2160, "4K"),
        (Quality::P1080, "1080p"),
        (Quality::P720, "720p"),
        (Quality::P480, "480p"),
    ];
    let i = d.inner();
    let ui = &i.ui;
    let index_of = |q: Quality| QUALITIES.iter().position(|(x, _)| *x == q).unwrap_or(0);
    let shown = Signal::new(index_of(crate::prefs::now().quality));
    let select = w::select(ui, "Quality", QUALITIES.iter().map(|(_, n)| (*n).to_owned()).collect(), shown).variant("dock");
    // Choices made on another screen show here; a pick here applies after the menu settles.
    let ps = crate::prefs::signal();
    let last = Rc::new(Cell::new(crate::prefs::now().quality));
    let l2 = last.clone();
    let select = select.bind(move || {
        let q = ps.with(|p| p.quality);
        if l2.replace(q) != q {
            shown.set_if_changed(index_of(q));
        }
    });
    let me = d.clone();
    let select = select.bind(move || {
        let q = QUALITIES[shown.get().min(QUALITIES.len() - 1)].0;
        if q != last.get() {
            untrack(|| me.choose_quality(q));
        }
    });
    field(ui, "Quality", select)
}

/// The Audio menu: the file's tracks; one track shows and cannot be changed.
fn audio_field(d: &Dock) -> El {
    let i = d.inner();
    let (u, audio, me) = (i.ui.clone(), i.audio, d.clone());
    // Rebuilt when the file's tracks or the one playing change.
    let menu = i.ui.el("p_dock_audio").row();
    let m2 = menu.clone();
    let menu = menu.bind(move || {
        let (tracks, chosen) = audio.get();
        untrack(|| {
            m2.clear();
            let names: Vec<String> = if tracks.is_empty() { vec!["Audio".into()] } else { tracks.iter().map(|t| t.label.clone()).collect() };
            let at = chosen.and_then(|c| tracks.iter().position(|t| t.index == c)).unwrap_or(0);
            let shown = Signal::new(at);
            let tip = if tracks.len() < 2 { names[0].clone() } else { "Audio track".into() };
            let s = w::select(&u, "Audio", names, shown).variant("dock").disabled(tracks.len() < 2);
            w::tooltip(&s, tip);
            let (me2, tracks2) = (me.clone(), tracks.clone());
            let first = Cell::new(true);
            let s = s.bind(move || {
                let n = shown.get();
                if first.replace(false) {
                    return;
                }
                if let Some(t) = tracks2.get(n) {
                    untrack(|| me2.choose_audio(t.index));
                }
            });
            let _ = m2.clone().child(s);
        });
    });
    field(&i.ui, "Audio", menu)
}

/// A label and its menu (`.dock-field`).
fn field(ui: &Ui, label: &str, control: El) -> El {
    ui.el("p_dock_field")
        .row()
        .align(Align::Center)
        .gap(7.2)
        .layout(|l| l.min_height = Length::Px(33.6))
        .child(ui.el("label").variant("muted").text(label.to_owned()).nowrap())
        .child(control)
}

/// "Autoplay next", shared through `/api/prefs`.
fn autoplay_field(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let ps = crate::prefs::signal();
    let on = Signal::new(crate::prefs::now().autoplay);
    let c = d.inner().ctx.clone();
    w::check(ui, "Autoplay next", on)
        .variant("dock")
        .bind(move || {
            let a = ps.with(|p| p.autoplay);
            untrack(|| on.set_if_changed(a));
        })
        .bind(move || {
            let a = on.get();
            if untrack(|| crate::prefs::now().autoplay) != a {
                untrack(|| crate::prefs::set(&c, crate::prefs::Patch { autoplay: Some(a), ..Default::default() }));
            }
        })
}

/// `.dock-side`: what plays, what to do next, then the preferences.
fn side(d: &Dock) -> El {
    let ui = &d.inner().ui;
    let controls = ui
        .el("p_dock_controls")
        .row()
        .align(Align::Center)
        .layout(|l| {
            l.wrap = true;
            l.gap = (8.0, 8.0);
        })
        .child(next_button(d))
        .child(cast_button(d))
        .child(links_button(d));
    let prefs = ui
        .el("p_dock_prefs")
        .row()
        .align(Align::Center)
        .layout(|l| {
            l.wrap = true;
            l.gap = (17.6, 6.4);
        })
        .child(quality_field(d))
        .child(audio_field(d))
        .child(autoplay_field(d));
    // .dock-prefs { border-top: 1px solid var(--line); padding-top: 0.65rem } (none in theater).
    let (theater, pl) = (d.inner().theater, d.inner().player.clone());
    let rule = ui.el("p_dock_rule").height(1.0).fill_width().layout(|l| l.shrink = 0.0);
    let (r2, p2) = (rule.clone(), prefs.clone());
    let prefs = ui.el("p_dock_prefs_box").column().child(rule).child(prefs).bind(move || {
        let wide = theater.get() && pl.is_theater();
        let _ = r2.clone().hidden(wide);
        let _ = p2.clone().padding(Insets { top: if wide { 0.0 } else { 10.4 }, ..Insets::ZERO });
    });
    // body.theater .dock-side { border-top: 1px solid var(--line) }.
    let (theater, pl) = (d.inner().theater, d.inner().player.clone());
    let top = ui.el("p_dock_rule").layout(|l| {
        l.position = Position::Absolute;
        l.inset = Insets { top: 0.0, start: 0.0, end: 0.0, bottom: AUTO };
        l.height = Length::Px(1.0);
    });
    let t2 = top.clone();
    let top = top.bind(move || {
        let _ = t2.clone().hidden(!(theater.get() && pl.is_theater()));
    });
    ui.el("p_dock_side").column().child(top).child(head(d)).child(status_line(d, "")).child(controls).child(prefs)
}

/// The dock: built once, shown while something plays.
pub fn dock(d: &Dock) -> El {
    let i = d.inner();
    let ui = &i.ui;
    let player = i.player.clone();
    let parts = PlayerParts::default().next_up_placement(NextUpPlacement::Corner).icons(player_icons).end(vec![captions_seam(ui), pip_button(d)]).title(mini_head(d));
    let widget = player_with(ui, &player, "Player", parts);
    let stage = ui.el("p_dock_stage").column().clip().child(widget.clone());
    let side_el = side(d);
    let theater_side = episodes::side(d);
    let root = ui
        .el("p_dock")
        .semantics(Semantics::new(Role::ContentInfo).label("Player"))
        .child(theater_side.clone())
        .child(stage.clone())
        .child(side_el.clone());
    // The picture's height: 16:9 of the stage's width, at most 52% of the window (34% on a
    // phone-sized page); the bar's own height comes on top.
    let video_h = Signal::new(0.0_f32);
    let stage_w = Signal::new(0.0_f32);
    let sw = stage_w;
    let stage = stage.on_resize(move |s| {
        if (sw.get_untracked().unwrap_or(0.0) - s.width).abs() > 0.5 {
            sw.set(s.width);
        }
    });
    // The bar under the picture, measured: the player's height is the picture's and the bar's.
    let chrome = Signal::new(None::<f32>);
    let wid = widget.clone();
    let _ = widget.clone().on_resize(move |s| {
        if let Some(st) = find(&wid, "player_stage").and_then(|e| e.bounds()) {
            let h = (s.height - st.height()).max(0.0);
            if chrome.get_untracked().ok().flatten().is_none_or(|c| (c - h).abs() > 0.5) {
                chrome.set(Some(h));
            }
        }
    });
    let ctx = i.ctx.clone();
    let (target, theater, open) = (i.target, i.theater, i.open);
    let (pl, r2, st2, sd2, ts2, wd2, ch2) = (player.clone(), root.clone(), stage.clone(), side_el.clone(), theater_side.clone(), widget.clone(), chrome);
    let root = root.bind(move || {
        let win = (ctx.width.get(), ctx.height.get());
        let narrow = ctx.width.get() <= NARROW;
        let is_open = open.get();
        let mode = if pl.is_fullscreen() && !pl.is_compact() {
            Mode::Full
        } else if pl.is_compact() {
            Mode::Mini
        } else if theater.get() && pl.is_theater() {
            Mode::Theater
        } else {
            Mode::Normal
        };
        let show_side = mode == Mode::Theater && target.with(|t| !super::episodes::show_id_of(t.as_ref()).is_empty());
        let head_h = ctx.head_h.get();
        let vh = {
            let w = stage_w.get();
            let cap = win.1 * if narrow { 0.34 } else { 0.52 };
            (w * 9.0 / 16.0).min(cap).max(0.0)
        };
        video_h.set_if_changed(vh);
        let _ = r2.clone().hidden(!is_open);
        let variant = match (mode, narrow) {
            (Mode::Full, _) => "full",
            (Mode::Theater, _) => "theater",
            (Mode::Mini, true) | (Mode::Normal, true) => "narrow",
            _ => "",
        };
        let _ = r2.clone().variant(variant);
        let _ = ts2.clone().hidden(!show_side);
        let _ = sd2.clone().hidden(matches!(mode, Mode::Mini | Mode::Full));
        let _ = st2.clone().variant(match mode {
            Mode::Normal => "",
            Mode::Theater | Mode::Full => "wide",
            Mode::Mini => "mini",
        });
        let _ = sd2.clone().variant(if mode == Mode::Theater { "theater" } else if narrow { "narrow" } else { "" });
        let _ = r2.clone().layout(move |l| {
            l.position = Position::Absolute;
            l.display = Display::Flex;
            l.columns = Vec::new();
            l.rows = Vec::new();
            l.width = Length::Auto;
            l.height = Length::Auto;
            l.max_height = Length::Auto;
            l.wrap = false;
            match mode {
                Mode::Normal if narrow => {
                    l.inset = Insets { top: AUTO, start: 0.0, end: 0.0, bottom: 0.0 };
                    l.axis = remedy_ui::layout::Axis::Column;
                    l.align_items = Align::Stretch;
                    l.gap = (9.6, 9.6);
                    
                }
                Mode::Normal => {
                    l.inset = Insets { top: AUTO, start: 11.2, end: 11.2, bottom: 11.2 };
                    l.axis = remedy_ui::layout::Axis::Row;
                    l.align_items = Align::Start;
                    l.gap = (16.0, 16.0);
                    
                }
                Mode::Mini => {
                    l.inset = if narrow {
                        Insets { top: AUTO, start: 0.0, end: 0.0, bottom: 0.0 }
                    } else {
                        Insets { top: AUTO, start: AUTO, end: 11.2, bottom: 11.2 }
                    };
                    if !narrow {
                        l.width = Length::Px((win.0 - 22.4).min(432.0));
                    }
                    l.axis = remedy_ui::layout::Axis::Column;
                    l.align_items = Align::Stretch;
                    l.gap = (0.0, 0.0);
                    
                }
                Mode::Theater => {
                    l.inset = Insets { top: head_h, start: 0.0, end: 0.0, bottom: 0.0 };
                    l.display = Display::Grid;
                    l.columns = if show_side { vec![Track::MaxContent, Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0)))] } else { vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0)))] };
                    l.rows = vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0))), Track::MaxContent];
                    l.gap = (0.0, 0.0);
                    
                    l.align_items = Align::Stretch;
                }
                Mode::Full => {
                    l.inset = Insets::ZERO;
                    l.axis = remedy_ui::layout::Axis::Column;
                    l.align_items = Align::Stretch;
                    l.gap = (0.0, 0.0);
                    
                }
            }
        });
        // Padding is the element's style (a layout padding would lose to the sheet's).
        let _ = r2.clone().padding(match mode {
            Mode::Normal if narrow => Insets::all(8.0),
            Mode::Normal => Insets::all(12.0),
            Mode::Mini => Insets::all(8.0),
            Mode::Theater | Mode::Full => Insets::ZERO,
        });
        let col = if show_side { 2 } else { 1 };
        let _ = ts2.clone().layout(move |l| {
            l.grid_column = Some((1, 1));
            // On a phone the list covers the picture only; the bar under it stays.
            l.grid_row = Some((1, if narrow { 1 } else { 2 }));
        });
        let _ = st2.clone().layout(move |l| {
            l.grid_column = Some((col, 1));
            l.grid_row = Some((1, 1));
            l.min_width = Length::Px(0.0);
            l.min_height = Length::Px(0.0);
            l.width = Length::Auto;
            l.height = Length::Auto;
            match mode {
                Mode::Normal if narrow => {
                    l.grow = 0.0;
                    l.shrink = 0.0;
                    l.basis = Length::Auto;
                }
                Mode::Normal => {
                    l.grow = 1.4;
                    l.shrink = 1.0;
                    l.basis = Length::Px(448.0);
                }
                Mode::Mini => {
                    l.grow = 0.0;
                    l.basis = Length::Auto;
                }
                Mode::Theater | Mode::Full => {
                    l.grow = 1.0;
                    l.basis = Length::Auto;
                    l.height = Length::Percent(1.0);
                }
            }
        });
        let _ = sd2.clone().layout(move |l| {
            l.grid_column = Some((col, 1));
            l.grid_row = Some((2, 1));
            l.min_width = Length::Px(0.0);
            match mode {
                Mode::Theater => {
                    l.axis = remedy_ui::layout::Axis::Row;
                    l.wrap = true;
                    l.align_items = Align::Center;
                    l.gap = (16.0, 8.8);
                    l.grow = 0.0;
                    l.basis = Length::Auto;
                }
                _ => {
                    l.axis = remedy_ui::layout::Axis::Column;
                    l.wrap = false;
                    l.align_items = Align::Stretch;
                    l.gap = (11.2, 11.2);
                    l.grow = if narrow { 0.0 } else { 1.0 };
                    l.shrink = 1.0;
                    l.basis = if narrow { Length::Auto } else { Length::Px(272.0) };
                }
            }
        });
        let wide = matches!(mode, Mode::Theater | Mode::Full);
        let fixed = mode == Mode::Normal;
        let chrome_h = ch2.get().unwrap_or(67.4);
        let _ = wd2.clone().layout(move |l| {
            l.height = if wide {
                Length::Percent(1.0)
            } else if fixed {
                Length::Px(vh + chrome_h)
            } else {
                Length::Auto
            };
        });
    });
    // Keeps the page clear of the dock (player.ts `fitPlayingPadding`), and tells the shell
    // how the dock lies (the header stays put under a phone-sized player).
    let (ctx, pl, theater, open) = (i.ctx.clone(), player.clone(), i.theater, i.open);
    let root = root.on_resize(move |s| {
        let _ = s;
    });
    let r3 = root.clone();
    let root = root.bind(move || {
        let view = DockView {
            open: open.get(),
            theater: open.get() && theater.get() && pl.is_theater(),
            mini: pl.is_compact(),
            cover: 0.0,
        };
        let cover = if view.open && !view.theater {
            let win = (ctx.width.get(), ctx.height.get());
            r3.bounds().map_or(0.0, |b| (win.1 - b.min.y).max(0.0))
        } else {
            0.0
        };
        ctx.dock.set_if_changed(DockView { cover, ..view });
    });
    // The chrome's measured height changes the stage's: lay out again once it is known.
    let (ctx2, r4) = (i.ctx.clone(), root.clone());
    root.on_resize(move |s| {
        let win = (ctx2.width.get_untracked().unwrap_or(0.0), ctx2.height.get_untracked().unwrap_or(0.0));
        let top = r4.bounds().map_or(win.1 - s.height, |b| b.min.y);
        ctx2.dock.update(|v| {
            let cover = if v.open && !v.theater { (win.1 - top).max(0.0) } else { 0.0 };
            if (v.cover - cover).abs() > 0.5 {
                v.cover = cover;
            }
        });
    })
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    let black = Color::BLACK;
    // .dock.open: the raised surface, a hairline, 18 px corners, a deep shadow.
    s.rule(
        "p_dock",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(18.0)
            .shadow(Shadow::new(0.0, 22.0, 56.0, mix(black, 0.42, clear))),
    );
    // Under 720 px: a sheet along the bottom, round at the top.
    s.rule(
        "p_dock",
        Some("narrow"),
        States::NONE,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .corners(CornerRadii { top_start: 16.0, top_end: 16.0, ..CornerRadii::ZERO })
            .shadow(Shadow::new(0.0, 22.0, 56.0, mix(black, 0.42, clear))),
    );
    // body.theater .dock.open, and the full-screen stage: black, edge to edge.
    for v in ["theater", "full"] {
        s.rule("p_dock", Some(v), States::NONE, StylePatch::default().background(black).border(clear).border_width(0.0).radius(0.0));
    }
    // .stage: black under a hairline, 12 px corners.
    s.rule("p_dock_stage", None, States::NONE, StylePatch::default().background(black).border(k.line).border_width(1.0).radius(12.0));
    s.rule("p_dock_stage", Some("wide"), States::NONE, StylePatch::default().background(black).border(clear).border_width(0.0).radius(0.0));
    s.rule("p_dock_stage", Some("mini"), States::NONE, StylePatch::default().background(clear).border(clear).border_width(0.0).radius(0.0));
    // .dock-side: 0.25rem 0.2rem 0.1rem; in theater the bar under the picture.
    s.rule("p_dock_side", None, States::NONE, StylePatch::default().padding(Insets { top: 4.0, bottom: 1.6, start: 3.2, end: 3.2 }));
    s.rule("p_dock_side", Some("narrow"), States::NONE, StylePatch::default().padding(Insets { top: 4.0, bottom: 1.6, start: 3.2, end: 3.2 }));
    s.rule(
        "p_dock_side",
        Some("theater"),
        States::NONE,
        StylePatch::default().background(k.raised).padding(Insets { top: 8.8, bottom: 10.4, start: 13.6, end: 13.6 }),
    );
    s.rule("p_dock_side", Some("mini"), States::NONE, StylePatch::default().padding(Insets::ZERO));
    // #dock-title: 1.35rem; 1.05rem in theater; 0.95rem minimized.
    s.rule("p_dock_title", None, States::NONE, StylePatch::default().heading(true).weight(400.0).text_size(21.6).line_height(LineHeight::Relative(1.25)).foreground(k.text));
    s.rule("p_dock_title", Some("theater"), States::NONE, StylePatch::default().heading(true).weight(400.0).text_size(16.8).line_height(LineHeight::Relative(1.25)).foreground(k.text));
    s.rule("p_dock_episode", Some("theater"), States::NONE, StylePatch::default().foreground(k.muted).padding(Insets::ZERO));
    s.rule("p_dock_title", Some("mini"), States::NONE, StylePatch::default().heading(true).weight(400.0).text_size(15.2).line_height(LineHeight::Relative(1.25)).foreground(k.text));
    s.rule("p_dock_episode", None, States::NONE, StylePatch::default().foreground(k.muted).padding(Insets { top: 4.0, bottom: 0.0, start: 0.0, end: 0.0 }));
    s.rule("p_dock_episode", Some("mini"), States::NONE, StylePatch::default().foreground(k.muted).text_size(12.8).padding(Insets::ZERO));
    // #dock-status: gold, lines kept.
    s.rule("p_dock_status", None, States::NONE, StylePatch::default().foreground(k.gold));
    s.rule("p_dock_status", Some("mini"), States::NONE, StylePatch::default().foreground(k.gold).text_size(12.8));
    // .dock-prefs: a hairline over it.
    s.rule("p_dock_rule", None, States::NONE, StylePatch::default().background(k.line));
    // #dock-mini, #dock-close: muted, the text color under the pointer.
    let bare = StylePatch::default().background(clear).border(clear).border_width(0.0).radius(999.0).padding(Insets::ZERO);
    s.rule("p_icon_btn", Some("dock_head"), States::NONE, bare.foreground(k.muted));
    s.rule("p_icon_btn", Some("dock_head"), States::HOVER, StylePatch::default().foreground(k.text).background(mix(k.text, 0.13, clear)));
    // .dock button.primary: gold, the background's color on it.
    s.rule("p_button", Some("primary"), States::NONE, StylePatch::default().background(k.gold).border(k.gold).foreground(k.bg).weight(600.0));
    s.rule("p_button", Some("primary"), States::HOVER, StylePatch::default().background(mix(k.gold, 0.82, Color::WHITE)));
    s.rule("p_button", Some("primary"), States::DISABLED, StylePatch::default().background(k.bg).border(k.line).foreground(k.text).weight(400.0).opacity(0.45));
    // .dock select: the well, a hairline, 8 px corners.
    s.rule(
        "button",
        Some("dock"),
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(Insets { top: 6.72, bottom: 6.72, start: 12.0, end: 12.0 })
            .weight(400.0)
            .text_size(16.0),
    );
    s.rule("button", Some("dock"), States::HOVER, StylePatch::default().border(k.gold).background(mix(k.gold, 0.10, k.bg)));
    s.rule("button", Some("dock"), States::DISABLED, StylePatch::default().opacity(0.45));
    s.rule("check", Some("dock"), States::CHECKED, StylePatch::default().background(k.gold).foreground(k.bg));
    episodes::rules(s, k);
}
