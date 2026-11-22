//! History (main.ts:1665-1732 `renderHistory`, `historyRow`, `playHistory`; plan P4): a row
//! per thing played, newest first, with its art, name, episode, when and whether it was
//! watched; Episodes or Details; Resume and Play; Rename and Remove (sent with the row's
//! title, so a row that shifted meanwhile is still found); and Clear all on the PC.

use std::rc::Rc;

use portal_api::{HistoryEntry, HistoryList, HistoryName, HistoryTitle, PlayKind, TitleKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::i18n::{CalendarSymbols, Date, Time};
use remedy_ui::image::{image_ext, Fit, ImageOptions, Source};
use remedy_ui::layout::{Align, Length};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail};
use crate::pages::Page;
use crate::screens::info::page_locale;
use crate::theme::{mix, Look};
use crate::widgets::activity::{describe_library, Library, Owner};
use crate::widgets::art::{art_url, letter_tile};
use crate::widgets::page::{h1, KeepRow};
use crate::widgets::play::{movie_target, play, PlayTarget};
use crate::widgets::titles::{episode_code, format_clock, resume_of};
use crate::widgets::{button, card, link};
use crate::Ctx;

/// The slot whose name is being edited (main.ts `editing` on the history page).
type Editing = Signal<Option<usize>>;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let editing: Editing = Signal::new(None);
    let shown = Signal::new(None::<Result<HistoryList, String>>);
    let again = Signal::new(0u32);
    let keep = KeepRow::default();
    let (c, at) = (ctx.clone(), ctx.router.route_untracked());
    let root = w::column(ui).fill_width().bind(move || {
        let _ = again.get();
        untrack(|| {
            let ticket = c.activity.begin(Owner::Page, describe_library(Library::History), 350);
            let (c2, at2) = (c.clone(), at.clone());
            c.api.run(api::history(), &Cancel::new(), move |out| {
                if c2.router.route_untracked() != at2 {
                    return;
                }
                c2.activity.end(ticket);
                shown.set(Some(out.map_err(|e| shown_detail(&e))));
            });
        });
    });
    let k2 = keep.clone();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        k2.note();
        again.update(|n| *n += 1);
    });
    let (r2, u, c) = (root.clone(), ui.clone(), ctx.clone());
    root.bind(move || {
        let Some(now) = shown.get() else { return };
        let remote = c.is_remote();
        untrack(|| {
            r2.clear();
            let el = match now {
                Err(e) => w::text(&u, e).variant("error"),
                Ok(body) => paint(&u, &c, &body, remote, editing, refresh.clone(), &keep),
            };
            let _ = r2.clone().child(el);
            keep.restore(&u);
        });
    })
}

fn paint(ui: &Ui, ctx: &Ctx, body: &HistoryList, remote: bool, editing: Editing, refresh: Rc<dyn Fn()>, keep: &KeepRow) -> El {
    let mut head = w::row(ui).gap(9.6).align(Align::Center).layout(|l| l.wrap = true).child(h1(ui, "History").layout(|l| l.margin = Insets::ZERO)); // .row > h1 { margin: 0 }
    if !body.items.is_empty() && !remote {
        let (u, c, r) = (ui.clone(), ctx.clone(), refresh.clone());
        head = head.child(button(ui, "Clear all", move || {
            let (c2, r2) = (c.clone(), r.clone());
            crate::screens::prompts::confirm(&u, "Clear all history?", move || {
                editing.set(None);
                let (c3, r3) = (c2.clone(), r2.clone());
                c2.api.run(api::clear_history(), &Cancel::new(), move |out| match out {
                    Ok(_) => r3(),
                    Err(e) => c3.activity.notice(shown_detail(&e)),
                });
            });
        }));
    }
    let col = w::column(ui).fill_width().child(head);
    if body.items.is_empty() {
        keep.set_rows(Vec::new());
        return col.child(w::text(ui, "Nothing has been played in Portal yet.").variant("muted").layout(|l| l.margin.top = 16.0));
    }
    let rows: Vec<El> = body.items.iter().map(|e| row(ui, ctx, e, editing, refresh.clone())).collect();
    keep.set_rows(rows.clone());
    // .history-item { margin: 0.55rem 0 }, the margins between rows collapsing.
    col.child(w::column(ui).gap(8.8).fill_width().layout(|l| l.margin.top = 8.8).children(rows))
}

/// The line under the name: episode, when, and "Watched".
pub fn facts(ui: &Ui, e: &HistoryEntry) -> String {
    let when = if e.at > 0 { played_at(ui, e.at) } else { String::new() };
    let state = if e.finished { "Watched" } else { "" };
    [episode_code(e.season, e.episode), when, state.to_owned()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// When it was played, on this device's clock (`toLocaleString(PAGE_LOCALE)`).
// ENGINE GAP: the engine has neither the UTC offset of a past instant (today's is used) nor
// the numeric "11/22/2026, 3:04:05 PM" form; this is CalendarSymbols::date_time's form.
fn played_at(ui: &Ui, at_ms: i64) -> String {
    let local = at_ms.div_euclid(1000) + i64::from(ui.local_time().utc_offset);
    let date = Date::from_days_since_epoch(local.div_euclid(86_400));
    let secs = local.rem_euclid(86_400);
    let Some(time) = Time::new((secs / 3600) as u8, (secs / 60 % 60) as u8) else { return String::new() };
    CalendarSymbols::for_locale(&page_locale(ui)).date_time(date, time)
}

fn row(ui: &Ui, ctx: &Ctx, e: &HistoryEntry, editing: Editing, refresh: Rc<dyn Fn()>) -> El {
    let resume = if e.finished { None } else { resume_of(Some(e.position)) };
    let editing_this = editing.get_untracked().ok().flatten() == Some(e.slot);
    let title = if editing_this {
        let (c, slot, r, r2) = (ctx.clone(), e.slot, refresh.clone(), refresh.clone());
        let at = HistoryTitle { id: Some(e.id.clone()), season: e.season, episode: e.episode };
        let save = Rc::new(move |name: String| {
            let (c2, r3) = (c.clone(), r.clone());
            c.api.run(api::update_history(slot, &HistoryName { name, title: at.clone() }), &Cancel::new(), move |out| match out {
                Ok(_) => {
                    editing.set(None);
                    r3();
                }
                Err(err) => c2.activity.notice(shown_detail(&err)),
            });
        });
        card::rename(ui, ctx, &e.name, save, Rc::new(move || {
            editing.set(None);
            r2();
        }))
    } else {
        ui.el("p_history_name").text(e.name.clone())
    };
    let grow = w::column(ui)
        .layout(|l| {
            l.grow = 1.0;
            l.shrink = 1.0;
            l.basis = Length::Px(0.0);
            l.min_width = Length::Px(0.0);
        })
        .child(title)
        .child(w::text(ui, facts(ui, e)).variant("muted"));
    let (more, words) = if e.kind == TitleKind::Tv {
        (Page::Show { id: e.id.clone(), season: e.season }, "Episodes")
    } else {
        (Page::Title(e.id.clone()), "Details")
    };
    let mut r = ui
        .el("p_history_item")
        .row()
        .align(Align::Center)
        .gap(12.8)
        .fill_width()
        .layout(|l| l.wrap = true)
        .semantics(Semantics::new(Role::Article))
        .child(thumb(ui, e.art.as_deref(), &e.name))
        .child(grow)
        .child(link(ui, &ctx.router, &more.path(), words, "", None));
    if e.file.is_some() {
        if let Some(at) = resume {
            let (c, it) = (ctx.clone(), e.clone());
            r = r.child(button(ui, format!("Resume {}", format_clock(at)), move || play_history(&c, &it, false)));
        }
        let (c, it) = (ctx.clone(), e.clone());
        let again = if e.finished || resume.is_some() { "Play again" } else { "Play" };
        r = r.child(button(ui, again, move || play_history(&c, &it, true)));
    }
    if !editing_this {
        r = r.child(tools(ui, ctx, e, editing, refresh));
    }
    r
}

/// Rename and Remove (main.ts `historyTools`).
fn tools(ui: &Ui, ctx: &Ctx, e: &HistoryEntry, editing: Editing, refresh: Rc<dyn Fn()>) -> El {
    let slot = e.slot;
    let r1 = refresh.clone();
    let rename = button(ui, "Rename", move || {
        editing.set(Some(slot));
        r1();
    })
    .variant("quiet");
    let (c, at) = (ctx.clone(), HistoryTitle { id: Some(e.id.clone()), season: e.season, episode: e.episode });
    let remove = button(ui, "Remove", move || {
        let (c2, r2) = (c.clone(), refresh.clone());
        c.api.run(api::delete_history(slot, &at), &Cancel::new(), move |out| match out {
            Ok(_) => {
                if editing.get_untracked().ok().flatten() == Some(slot) {
                    editing.set(None);
                }
                r2();
            }
            Err(err) => c2.activity.notice(shown_detail(&err)),
        });
    })
    .variant("quiet")
    .semantics(Semantics::new(Role::Button).label(format!("Remove {} from history", e.name)).action(Action::Click));
    ui.el("p_row_actions").row().gap(5.6).align(Align::Center).layout(|l| l.wrap = true).child(rename).child(remove)
}

/// `.history-item .art`: 48 px square, 8 px corners, the letter tile without a picture.
fn thumb(ui: &Ui, art: Option<&str>, name: &str) -> El {
    let tile = {
        let (u, n) = (ui.clone(), name.to_owned());
        move || letter_tile(&u, &n)
    };
    let inner = match art_url(art) {
        None => tile(),
        Some(url) => image_ext(ui, Source::Url(url), "", Fit::Cover, ImageOptions::new().placeholder(ui.el("p_art_wait")).fallback(tile)).layout(|l| {
            l.width = Length::Percent(1.0);
            l.height = Length::Percent(1.0);
        }),
    };
    ui.el("p_art").variant("history").column().clip().width(48.0).height(48.0).layout(|l| l.shrink = 0.0).child(inner)
}

/// main.ts `playHistory`: with no file it opens the title; else it plays, from where it
/// stopped unless `from_start` or watched.
fn play_history(ctx: &Ctx, e: &HistoryEntry, from_start: bool) {
    let Some(file) = e.file.clone() else {
        ctx.go(&if e.kind == TitleKind::Tv { Page::Show { id: e.id.clone(), season: e.season } } else { Page::Title(e.id.clone()) });
        return;
    };
    let resume = if from_start || e.finished { None } else { resume_of(Some(e.position)) };
    if e.kind == TitleKind::Tv && e.season.is_some() && e.episode.is_some_and(|n| n > 0) {
        play(
            ctx,
            PlayTarget {
                kind: PlayKind::Episode,
                id: e.id.clone(),
                file,
                title: e.name.clone(),
                season: e.season,
                episode: e.episode,
                art: e.art.clone(),
                show_title: Some(e.name.clone()),
                show_id: Some(e.id.clone()),
                info: None,
                resume_seconds: resume,
                manual_links: false,
                progress_kind: TitleKind::Tv,
                progress_id: e.id.clone(),
                progress_name: e.name.clone(),
                track_progress: true,
            },
        );
        return;
    }
    play(ctx, movie_target(&e.id, &file, &e.name, e.art.clone(), resume));
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let none = States::NONE;
    s.rule(
        "p_history_item",
        None,
        none,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .padding(Insets { top: 13.6, bottom: 13.6, start: 14.4, end: 14.4 }),
    );
    s.rule("p_history_item", None, States::HOVER, StylePatch::default().border(mix(k.gold, 0.48, k.line)));
    s.rule("p_art", Some("history"), none, StylePatch::default().radius(8.0));
}
