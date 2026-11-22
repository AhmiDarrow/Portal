//! Details of a movie or show: Resume, Play, Episodes, Watchlist, Bookmark, Info, and the
//! library's episodes by season (main.ts:1350-1468 `renderTitle`, `episodeRow`; plan P6).

use std::rc::Rc;

use portal_api::{Episode, MediaInfo, MediaKind, TitleDetail, TitleKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::layout::{Align, Display, Justify, Length, Position, Track};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::{RichText, TextStylePatch};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail, List};
use crate::pages::Page;
use crate::screens::info::{self, InfoRequest};
use crate::theme::{mix, Look};
use crate::widgets::activity::{describe_library, Library, Owner};
use crate::widgets::art;
use crate::widgets::card::bar;
use crate::widgets::episode_row::still;
use crate::widgets::page::h1;
use crate::widgets::play::{episode_target, play, title_target};
use crate::widgets::saved::{self, info_button, save_button_for, SaveTitle};
use crate::widgets::titles::{clean_episode_title, episode_code, format_clock, resume_of};
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, id: &str) -> El {
    let root = w::column(ui).fill_width();
    if id.is_empty() {
        return root.child(w::text(ui, "This title is not in Portal yet.").variant("error"));
    }
    let ticket = ctx.activity.begin(Owner::Page, describe_library(Library::Title), 350);
    let title = Signal::new(None::<Result<TitleDetail, String>>);
    let c = ctx.clone();
    ctx.api.run(api::title(id), &Cancel::new(), move |out| {
        c.activity.end(ticket);
        title.set(Some(out.map_err(|e| shown_detail(&e))));
    });
    let (r2, u, c) = (root.clone(), ui.clone(), ctx.clone());
    root.bind(move || {
        let t = title.get();
        let phone = c.width.get() <= 720.0;
        untrack(|| {
            r2.clear();
            let el = match t {
                None => return,
                Some(Err(e)) => w::text(&u, e).variant("error"),
                Some(Ok(t)) => details(&u, &c, &t, phone),
            };
            let _ = r2.clone().child(el);
        });
    })
}

/// `.hero .row > button`: every action in the row is a pill; the first is gold.
fn hero_button(ui: &Ui, label: &str, first: bool, on_press: impl Fn() + 'static) -> El {
    ui.el("p_details_btn")
        .variant(if first { "first" } else { "" })
        .row()
        .align(Align::Center)
        .child(ui.el("p_pill_text").text(label.to_owned()).nowrap())
        .on_click(on_press)
        .semantics(Semantics::new(Role::Button).label(label.to_owned()).action(Action::Click))
}

fn details(ui: &Ui, ctx: &Ctx, title: &TitleDetail, phone: bool) -> El {
    let resume = resume_of(title.resume_seconds);
    saved::note(&title.id, Some(title.watchlisted), Some(title.bookmarked));
    let save = SaveTitle {
        id: title.id.clone(),
        kind: title.kind,
        name: title.name.clone(),
        art: title.art.clone(),
        year: title.year,
        imdb_id: title.imdb_id.clone(),
    };
    let playable = title.kind == TitleKind::Movie || title.next.is_some();
    let mut row = ui.el("p_details_row").row().align(Align::Center).gap(9.6).layout(|l| l.wrap = true);
    let mut first = true;
    if let Some(r) = resume {
        let (c, t) = (ctx.clone(), title.clone());
        row = row.child(hero_button(ui, &format!("Resume {}", format_clock(r)), first, move || {
            if let Some(p) = title_target(&t, false) {
                play(&c, p);
            }
        }));
        first = false;
    }
    if playable {
        let (c, t) = (ctx.clone(), title.clone());
        row = row.child(hero_button(ui, "Play", first, move || {
            if let Some(p) = title_target(&t, true) {
                play(&c, p);
            }
        }));
        first = false;
    }
    if title.kind == TitleKind::Tv {
        let (c, id, s) = (ctx.clone(), title.id.clone(), title.next.as_ref().map(|n| n.season));
        row = row.child(hero_button(ui, "Episodes", first, move || c.go(&Page::Show { id: id.clone(), season: s })));
    }
    let seed = MediaInfo {
        kind: Some(if title.kind == TitleKind::Tv { MediaKind::Tv } else { MediaKind::Movie }),
        title: Some(title.name.clone()),
        year: title.year,
        imdb_id: title.imdb_id.clone(),
        ..MediaInfo::default()
    };
    let (u, c, t, sv) = (ui.clone(), ctx.clone(), title.clone(), save.clone());
    let info_btn = info_button(ui, &title.name, "pill", move |el| {
        let run: Option<(String, Rc<dyn Fn()>)> = if playable {
            let (c2, t2) = (c.clone(), t.clone());
            Some((
                "Play".into(),
                Rc::new(move || {
                    if let Some(p) = title_target(&t2, true) {
                        play(&c2, p);
                    }
                }),
            ))
        } else {
            None
        };
        info::open(&u, &c, InfoRequest { seed: seed.clone(), art: t.art.clone(), save: Some(sv.clone()), play: run, opener: Some(el.clone()) });
    });
    let row = row
        .child(save_button_for(ui, ctx, List::Watchlist, save.clone(), "pill"))
        .child(save_button_for(ui, ctx, List::Bookmarks, save, "pill"))
        .child(info_btn);
    let kind = format!("{}{}", if title.kind == TitleKind::Tv { "Show" } else { "Movie" }, if title.watched { " · Watched" } else { "" });
    // `.hero > div`: as wide as its content wants, so the actions stay on one line while
    // they fit.
    let text = w::column(ui)
        .grow(1.0)
        .layout(|l| {
            l.min_width = Length::Px(0.0);
            l.shrink = 1.0;
        })
        .child(h1(ui, &title.name))
        .child(w::text(ui, kind).variant("muted").layout(|l| l.margin = Insets { top: 6.4, bottom: 16.0, start: 0.0, end: 0.0 }))
        .child(row);
    let poster = ui
        .el("p_details_art")
        .clip()
        .width(188.0)
        .layout(|l| l.shrink = 0.0)
        .child(art::art(ui, title.art.as_deref(), &title.name, ""));
    let hero = ui.el("p_details_hero");
    // `.hero { flex-direction: column }` at 720 px and under.
    let hero = if phone { hero.column() } else { hero.row() };
    let hero = hero
        .align(Align::Start)
        .gap(18.4)
        .fill_width()
        .layout(|l| {
            l.margin.bottom = 21.6;
            l.shrink = 0.0;
        })
        .semantics(Semantics::new(Role::Article))
        .child(poster)
        .child(text);
    let mut col = w::column(ui).fill_width().child(hero);
    for season in &title.seasons {
        let name = if season.number == 0 { "Specials".to_owned() } else { format!("Season {}", season.number) };
        let mut sec = ui
            .el("p_season")
            .column()
            .fill_width()
            .layout(|l| {
                l.margin = Insets { top: 18.4, bottom: 18.4, start: 0.0, end: 0.0 };
                l.shrink = 0.0;
            })
            .child(w::heading(ui, 2, name).variant("season").layout(|l| {
                l.margin = Insets { top: 3.2, bottom: 0.0, start: 0.0, end: 0.0 };
            }).padding(Insets { top: 0.0, bottom: 6.4, start: 0.0, end: 0.0 }))
            // `.season > h2`'s border-bottom, with its 0.6rem margin under it.
            .child(ui.el("p_season_rule").height(1.0).fill_width().layout(|l| {
                l.shrink = 0.0;
                l.margin.bottom = 9.6;
            }));
        for e in &season.episodes {
            sec = sec.child(episode_row(ui, ctx, title, e, phone));
        }
        col = col.child(sec);
    }
    col
}

/// main.ts `episodeRow`: a small still, the code and name, Resume and Play.
fn episode_row(ui: &Ui, ctx: &Ctx, title: &TitleDetail, e: &Episode, phone: bool) -> El {
    let code = episode_code(Some(e.season), Some(e.episode));
    let cleaned = clean_episode_title(&e.name, Some(e.season), Some(e.episode), None);
    let label = if cleaned.is_empty() { e.name.clone() } else { cleaned };
    let resume = if e.resumable { resume_of(Some(e.position)) } else { None };
    let fraction = if e.finished { Some(1.0) } else if e.duration > 0.0 { Some(e.position / e.duration) } else { None };
    let tile_name = if label.is_empty() { code.clone() } else { label.clone() };
    let mut thumb = ui
        .el("p_episode_thumb")
        .clip()
        .fill_width()
        .layout(|l| l.position = Position::Relative)
        .child(still(ui, e.art.as_deref(), &tile_name, 19.2));
    // `fraction ? bar(..) : ""`: no bar at 0.
    if let Some(f) = fraction.filter(|f| *f != 0.0 && f.is_finite()) {
        thumb = thumb.child(bar(ui, f).variant("still_small").height(3.0).layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: remedy_ui::layout::AUTO, bottom: 0.0, start: 0.0, end: 0.0 };
        }));
    }
    let mut tnum = TextStylePatch::default();
    tnum.features = vec![remedy_ui::text::Feature { tag: *b"tnum", value: 1 }];
    let code_line = format!("{code}{}", if e.finished { " · Watched" } else { "" });
    let name = w::column(ui)
        .layout(|l| l.min_width = Length::Px(0.0))
        .child(ui.el("p_episode_code").text(RichText::new().styled(&code_line, tnum)))
        .child(w::text(ui, label));
    let mut actions = ui.el("p_episode_actions").row().gap(5.6).justify(Justify::End).layout(move |l| {
        l.wrap = true;
        if phone {
            l.grid_column = Some((1, 2));
            l.justify = Justify::End;
        }
    });
    if let Some(r) = resume {
        let (c, t, ep) = (ctx.clone(), title.clone(), e.clone());
        actions = actions.child(crate::widgets::button(ui, format!("Resume {}", format_clock(r)), move || play(&c, episode_target(&t, &ep, false))).variant("episode"));
    }
    let (c, t, ep) = (ctx.clone(), title.clone(), e.clone());
    actions = actions.child(crate::widgets::button(ui, if e.finished { "Play again" } else { "Play" }, move || play(&c, episode_target(&t, &ep, true))).variant("episode"));
    ui.el("p_episode")
        .layout(move |l| {
            l.display = Display::Grid;
            l.columns = if phone { vec![Track::Px(88.0), Track::Fr(1.0)] } else { vec![Track::Px(120.0), Track::Fr(1.0), Track::MaxContent] };
            l.gap = (11.2, 11.2);
            l.align_items = Align::Center;
            l.margin = Insets { top: 7.2, bottom: 7.2, start: 0.0, end: 0.0 };
            l.shrink = 0.0;
        })
        .padding(Insets { top: 9.92, bottom: 9.92, start: 12.0, end: 12.0 })
        .child(thumb)
        .child(name)
        .child(actions)
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let none = States::NONE;
    s.rule(
        "p_details_btn",
        None,
        none,
        StylePatch::default()
            .background(k.bg)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(999.0)
            .padding(Insets { top: 7.2, bottom: 7.2, start: 16.0, end: 16.0 }),
    );
    s.rule("p_details_btn", None, States::HOVER, StylePatch::default().border(k.gold));
    s.rule("p_details_btn", Some("first"), none, StylePatch::default().background(k.gold).border(k.gold).foreground(k.bg).weight(600.0));
    let lift = mix(k.gold, 0.84, remedy_ui::core::Color::WHITE);
    for st in [States::HOVER, States::FOCUS_VISIBLE] {
        s.rule("p_details_btn", Some("first"), st, StylePatch::default().background(lift).border(k.gold));
    }
    s.rule(
        "p_details_art",
        None,
        none,
        StylePatch::default().border(k.line).border_width(1.0).radius(14.0).shadow(k.shadow),
    );
    s.rule("heading", Some("season"), none, StylePatch::default().heading(true).weight(400.0).text_size(24.0).foreground(k.text).line_height(remedy_ui::style::LineHeight::Relative(1.25)));
    s.rule(
        "p_episode",
        None,
        none,
        StylePatch::default().background(k.raised).border(k.line).border_width(1.0).radius(14.0),
    );
    s.rule("p_episode", None, States::HOVER, StylePatch::default().border(mix(k.gold, 0.48, k.line)));
    s.rule("p_episode_thumb", None, none, StylePatch::default().radius(6.0));
    s.rule(
        "p_episode_code",
        None,
        none,
        StylePatch::default().foreground(k.muted).letter_spacing(remedy_ui::style::LetterSpacing::Em(0.03)),
    );
    s.rule("p_season_rule", None, none, StylePatch::default().background(k.line));
    s.rule("p_bar", Some("still_small"), none, StylePatch::default().radius(0.0));
    // `.episode button { background: transparent }`.
    s.rule("p_button", Some("episode"), none, StylePatch::default().background(remedy_ui::core::Color::TRANSPARENT));
}
