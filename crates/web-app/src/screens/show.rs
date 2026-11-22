//! Show: a show's hero, season tabs and episodes (main.ts:887-1123; plan P6).
//!
//! `#/show/<id>/<season>` looks the show up in the library, picks the season asked for (else
//! the one being watched), and replaces the address with that season's folder, as main.ts
//! `renderShow` does, so Back skips it. The folder's page ([`folder_view`], from Browse)
//! leads with the show: [`hero`](crate::widgets::hero), [`season tabs`](crate::widgets::season_tabs),
//! then the episodes as rows (or the seasons as cards on the show's own page), then the
//! add-on search under them. Opened at the episode being watched, that row is scrolled into
//! view. The library is asked again after each paint and the page repaints when it changed.

use std::cell::RefCell;
use std::rc::Rc;

use portal_api::{BrowseItem, BrowsePage, ItemKind, MediaKind, PlayKind, Source, TitleKind};
use remedy_ui::core::Insets;
use remedy_ui::layout::{Align, Length};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail};
use crate::pages::Page;
use crate::theme::Look;
use crate::widgets::activity::{describe_library, Library, Owner};
use crate::widgets::art::{art_from_id, art_url, poster_from_id};
use crate::widgets::browse_card;
use crate::widgets::episode_row::{episode_row, other_row, EpisodeRow};
use crate::widgets::hero::{self, Crumb, Hero};
use crate::widgets::play::{play, PlayTarget};
use crate::widgets::saved::{self, info_button, save_button_for};
use crate::widgets::season_tabs::season_tabs;
use crate::widgets::shows::{self, episode_show, library_episode, media_of, page_show, season_name, show_seasons, show_view, where_watching, Media, ShowRef, ShowView};
use crate::widgets::titles::{clean_episode_title, episode_code, format_clock, resume_of};
use crate::screens::info::{self, aired_on, runtime, InfoRequest};
use crate::Ctx;

use crate::api::List;

thread_local! {
    /// The episode a show address meant, scrolled into view once its season paints.
    static LANDING: RefCell<Option<(String, u32, u32)>> = const { RefCell::new(None) };
}

/// `#/show/<id>/<season>`: finds the folder and replaces the address with it.
pub fn view(ui: &Ui, ctx: &Ctx, id: &str, season: Option<u32>) -> El {
    let root = w::column(ui).fill_width();
    if id.is_empty() {
        return root.child(w::text(ui, "Missing a show.").variant("error"));
    }
    let ticket = ctx.activity.begin(Owner::Page, describe_library(Library::Title), 350);
    let (c, r2, u, id) = (ctx.clone(), root.clone(), ui.clone(), id.to_owned());
    ctx.api.run(api::title(&id), &Cancel::new(), move |out| {
        c.activity.end(ticket);
        let title = match out {
            Ok(t) => Some(t),
            // A show the library has not filed yet still opens as the add-on lists it.
            Err(e) if e.status == Some(404) => None,
            Err(e) => {
                let _ = r2.clone().child(w::text(&u, shown_detail(&e)).variant("error"));
                return;
            }
        };
        let mut folder = id.clone();
        LANDING.with(|l| *l.borrow_mut() = None);
        if let Some(t) = title {
            shows::note_library_show(&t);
            let at = where_watching(&t);
            let wanted = season.or(at.map(|a| a.0));
            let found = wanted.and_then(|s| show_seasons(&t.id).into_iter().find(|f| f.season == s));
            if let Some(f) = &found {
                folder = f.path.clone();
            }
            if let Some((s, e)) = at {
                let lands = match &found {
                    Some(f) => f.season == s,
                    None => season.is_none(),
                };
                if lands {
                    LANDING.with(|l| *l.borrow_mut() = Some((folder.clone(), s, e)));
                }
            }
        } else {
            shows::note_library_missing(&id);
        }
        c.router.replace(&Page::Browse(folder).path());
    });
    root
}

/// The browse source, for the crumbs' first link (main.ts `browseSource`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BrowseSource {
    pub path: String,
    pub name: String,
}

impl BrowseSource {
    pub fn of(s: &Source) -> Option<BrowseSource> {
        let path = s.path.clone().filter(|p| !p.is_empty())?;
        Some(BrowseSource { path, name: s.name.clone().unwrap_or_default() })
    }
}

/// The folder's page as its show's page, or `None` when the folder is not a show's.
/// `heading` is the folder's own heading (main.ts `browseHeading`).
pub fn folder_view(ui: &Ui, ctx: &Ctx, page: &BrowsePage, heading: &str, source: Option<BrowseSource>, search: El) -> Option<El> {
    shows::remember_folder(&page.path, &page.items);
    let show = page_show(page)?;
    let show_id = show.save.id.clone();
    let version = Signal::new(0u32);
    let root = w::column(ui).fill_width();
    let (r2, u, c, p, h) = (root.clone(), ui.clone(), ctx.clone(), page.clone(), heading.to_owned());
    let search = Rc::new(search);
    let first = Rc::new(std::cell::Cell::new(true));
    let last = Rc::new(std::cell::Cell::new(None::<(u32, bool, bool)>));
    let root = root.bind(move || {
        let v = version.get();
        let phone = c.width.get() <= 720.0;
        let remote = c.is_remote();
        // Only a change repaints: a rebuild empties the page for a moment, which would lose
        // the scroll position (the old page's innerHTML swap keeps it).
        if last.replace(Some((v, phone, remote))) == Some((v, phone, remote)) {
            return;
        }
        untrack(|| {
            r2.clear();
            let view = show_view(&p, show.clone());
            let el = paint(&u, &c, &p, &view, &h, source.as_ref(), remote, phone);
            let _ = r2.clone().child(el).child((*search).clone());
            after_paint(&u, &c, &p, &view, version, first.replace(false));
        });
    });
    // A progress save for this show refreshes what its episodes say (main.ts
    // `portal:progress`, 500 ms after the last one).
    let (c, id, u) = (ctx.clone(), show_id, ui.clone());
    let seen = Rc::new(std::cell::Cell::new(ctx.progress.get_untracked().map(|p| p.1).unwrap_or(0)));
    let wait = Rc::new(std::cell::Cell::new(0u64));
    let root = root.bind(move || {
        let (pid, n) = c.progress.get();
        if seen.replace(n) == n || pid != id {
            return;
        }
        let mine = wait.get() + 1;
        wait.set(mine);
        let (c2, id2, w2) = (c.clone(), id.clone(), wait.clone());
        u.after(std::time::Duration::from_millis(500), move || {
            if w2.get() == mine {
                refresh_library(&c2, id2, version);
            }
        });
    });
    Some(root)
}

#[allow(clippy::too_many_arguments)]
fn paint(ui: &Ui, ctx: &Ctx, page: &BrowsePage, view: &ShowView, heading: &str, source: Option<&BrowseSource>, remote: bool, phone: bool) -> El {
    let col = w::column(ui).fill_width();
    let mut ids: Vec<String> = page.items.iter().filter_map(|i| media_of(&page.path, i).map(|m| m.save.id)).collect();
    ids.push(view.show.save.id.clone());
    let wanted = {
        let (r, at) = (ctx.router.clone(), ctx.router.route_untracked());
        move || r.route_untracked() == at
    };
    saved::load(ctx, ids, wanted);
    let col = col.child(show_hero(ui, ctx, view, page, heading, source, remote, phone));
    let col = match season_tabs(ui, ctx, view, &page.path) {
        Some(t) => col.child(t),
        None => col,
    };
    let items = if page.items.is_empty() {
        w::text(ui, "This folder is empty.").variant("muted")
    } else if view.episodes {
        let rows: Vec<El> = page.items.iter().map(|i| episode_item(ui, ctx, &page.path, i, view, phone)).collect();
        ui.el("p_ep_list").column().gap(11.2).fill_width().children(rows).layout(|l| l.shrink = 0.0)
    } else {
        let (c1, c2, folder) = (ctx.clone(), ctx.clone(), page.path.clone());
        let u2 = ui.clone();
        let open: browse_card::Open = Rc::new(move |item: &BrowseItem| open_item(&c1, &folder, item, None));
        let folder2 = page.path.clone();
        let info: browse_card::OpenInfo = Rc::new(move |m: &Media, el: &El, label: String| {
            let (c3, f3) = (c2.clone(), folder2.clone());
            let item = m.clone();
            let run: Rc<dyn Fn()> = Rc::new(move || {
                // Info's first action opens or plays it as the card does.
                if let Some(i) = find_item(&c3, &f3, &item) {
                    open_item(&c3, &f3, &i, None);
                }
            });
            open_media_info(&u2, &c2, m, el, Some((label, run)));
        });
        let cards = page.items.iter().map(|i| browse_card::card(ui, ctx, &page.path, i, Some(view), open.clone(), info.clone())).collect();
        browse_card::grid(ui, cards, phone)
    };
    col.child(items)
}

thread_local! {
    static PAGE_ITEMS: RefCell<Vec<(String, Vec<BrowseItem>)>> = const { RefCell::new(Vec::new()) };
}

fn find_item(_ctx: &Ctx, folder: &str, m: &Media) -> Option<BrowseItem> {
    PAGE_ITEMS.with(|p| {
        p.borrow().iter().find(|(f, _)| f == folder).and_then(|(_, items)| {
            items.iter().find(|i| media_of(folder, i).is_some_and(|x| x == *m)).cloned()
        })
    })
}

/// Info for something a page lists (main.ts `openMediaInfo`).
pub fn open_media_info(ui: &Ui, ctx: &Ctx, m: &Media, opener: &El, run: Option<(String, Rc<dyn Fn()>)>) {
    info::open(ui, ctx, InfoRequest { seed: m.seed.clone(), art: m.art.clone(), save: Some(m.save.clone()), play: run, opener: Some(opener.clone()) });
}

/// main.ts `mediaButtons`: Watchlist, Bookmark and Info.
pub fn media_buttons(ui: &Ui, ctx: &Ctx, m: &Media, variant: &'static str, info: impl Fn(&El) + 'static) -> Vec<El> {
    let name = if m.kind == MediaKind::Episode { m.seed.title.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| m.save.name.clone()) } else { m.save.name.clone() };
    vec![
        save_button_for(ui, ctx, List::Watchlist, m.save.clone(), variant),
        save_button_for(ui, ctx, List::Bookmarks, m.save.clone(), variant),
        info_button(ui, &name, variant, info),
    ]
}

#[allow(clippy::too_many_arguments)]
fn show_hero(ui: &Ui, ctx: &Ctx, view: &ShowView, page: &BrowsePage, heading: &str, source: Option<&BrowseSource>, remote: bool, phone: bool) -> El {
    let show = &view.show;
    let library = view.library.as_ref();
    let name = show.save.name.clone();
    let poster = poster_from_id(&show.save.id).or(show.art.clone()).or(library.and_then(|l| l.art.clone()));
    let fanart = art_url(art_from_id(&show.save.id, &["fanart", "tvshow.fanart", "landscape"]).or(show.seed.fanart.clone()).as_deref());
    let here = match view.season {
        Some(s) => season_name(s),
        None if page.path != show.save.id => heading.to_owned(),
        None => String::new(),
    };
    let root = match source {
        Some(s) => Crumb::Link { path: Page::Browse(s.path.clone()).path(), label: if s.name.is_empty() { "Browse".into() } else { s.name.clone() } },
        None if remote => Crumb::Text("Browse".into()),
        None => Crumb::Link { path: Page::Source.path(), label: "Source".into() },
    };
    let mut crumbs = vec![root];
    if here.is_empty() {
        crumbs.push(Crumb::Here(name.clone()));
    } else {
        crumbs.push(Crumb::Link { path: Page::Browse(show.save.id.clone()).path(), label: name.clone() });
        crumbs.push(Crumb::Here(here.clone()));
    }
    let mut facts = Vec::new();
    if !here.is_empty() {
        facts.push(here.clone());
    }
    if let Some(y) = show.seed.year.filter(|_| here.is_empty()) {
        facts.push(y.to_string());
    }
    if view.episodes {
        let rows: Vec<&BrowseItem> = page.items.iter().filter(|i| i.kind == ItemKind::Play && episode_show(&page.path, i).is_some()).collect();
        facts.push(format!("{} {}", rows.len(), if rows.len() == 1 { "episode" } else { "episodes" }));
        if let Some(lib) = library {
            let watched = rows
                .iter()
                .filter(|i| {
                    episode_show(&page.path, i).is_some_and(|n| {
                        library_episode(lib, &i.path, n.season.or(view.season), n.episode).is_some_and(|e| e.finished)
                    })
                })
                .count();
            if watched > 0 {
                facts.push(if watched == rows.len() { "All watched".into() } else { format!("{watched} watched") });
            }
        }
    } else if let Some(lib) = library {
        let watched: usize = lib.seasons.iter().map(|s| s.episodes.iter().filter(|e| e.finished).count()).sum();
        if watched > 0 {
            facts.push(format!("{watched} {} watched", if watched == 1 { "episode" } else { "episodes" }));
        }
    }
    let mut actions = Vec::new();
    if let Some(p) = hero_play(ui, ctx, view) {
        actions.push(p);
    }
    let (u, c, m) = (ui.clone(), ctx.clone(), show.clone());
    actions.extend(media_buttons(ui, ctx, show, "pill", move |el| open_media_info(&u, &c, &m, el, None)));
    hero::hero(ui, ctx, Hero { name, poster, fanart, crumbs, facts, actions }, phone)
}

/// Resume or play the episode the show is up to, from anywhere in the show (main.ts `heroPlay`).
fn hero_play(ui: &Ui, ctx: &Ctx, view: &ShowView) -> Option<El> {
    let library = view.library.clone()?;
    let (s, e) = view.next?;
    let code = episode_code(Some(s), Some(e));
    let next_path = library.next.as_ref().map(|n| n.path.clone()).unwrap_or_default();
    let known = library_episode(&library, &next_path, Some(s), e).cloned();
    let resume = known.as_ref().filter(|k| k.resumable).and_then(|k| resume_of(Some(k.position)));
    let label = match resume {
        Some(r) => format!("Resume {code} · {}", format_clock(r)),
        None => format!("Play {code}"),
    };
    let c = ctx.clone();
    Some(hero::play_pill(ui, &label, move || {
        let target = match &known {
            Some(k) => Some(crate::widgets::play::episode_target(&library, k, resume.is_none())),
            None => crate::widgets::play::title_target(&library, true),
        };
        if let Some(t) = target {
            play(&c, t);
        }
    }))
}

/// One episode as a row (main.ts `episodeItem`).
fn episode_item(ui: &Ui, ctx: &Ctx, folder: &str, item: &BrowseItem, view: &ShowView, phone: bool) -> El {
    let media = media_of(folder, item);
    let numbers = if item.kind == ItemKind::Play { episode_show(folder, item) } else { None };
    let (Some(numbers), Some(media)) = (numbers, media.filter(|m| m.kind == MediaKind::Episode)) else {
        let label = if item.label.is_empty() { "Untitled".to_owned() } else { item.label.clone() };
        let (c, f, i) = (ctx.clone(), folder.to_owned(), item.clone());
        return ui.el("p_ep_other").child(other_row(ui, &label, move || open_item(&c, &f, &i, None)));
    };
    let info = item.info.as_ref();
    let season = numbers.season.or(view.season);
    let code = episode_code(season, Some(numbers.episode));
    let raw = info.and_then(|i| i.title.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| item.label.clone());
    let name = Some(clean_episode_title(&raw, season, Some(numbers.episode), item.show_title.as_deref()))
        .filter(|n| !n.is_empty())
        .or(Some(item.label.clone()).filter(|l| !l.is_empty()))
        .unwrap_or_else(|| code.clone());
    let known = view.library.as_ref().and_then(|l| library_episode(l, &item.path, season, numbers.episode)).cloned();
    let resume = known.as_ref().filter(|k| k.resumable).and_then(|k| resume_of(Some(k.position)));
    let finished = known.as_ref().is_some_and(|k| k.finished);
    let fraction = known.as_ref().filter(|k| !finished && k.duration > 0.0 && k.position > 0.0).map(|k| k.position / k.duration);
    let next = view.next.is_some_and(|(s, e)| Some(s) == season && e == numbers.episode);
    let mut facts = vec![code.clone(), aired_on(ui, info.and_then(|i| i.premiered.as_deref())), runtime(info.and_then(|i| i.runtime_minutes))];
    if let Some(r) = info.and_then(|i| i.rating).filter(|r| r.is_finite() && *r > 0.0) {
        facts.push(format!("★ {:.1}", r.min(10.0)));
    }
    facts.retain(|f| !f.is_empty());
    let go_label = match (resume, finished) {
        (Some(r), _) => format!("Resume {}", format_clock(r)),
        (None, true) => "Play again".into(),
        _ => "Play".into(),
    };
    let play_from = {
        let (c, f, i) = (ctx.clone(), folder.to_owned(), item.clone());
        move |at: Option<f64>| -> Rc<dyn Fn()> {
            let (c, f, i) = (c.clone(), f.clone(), i.clone());
            Rc::new(move || open_item(&c, &f, &i, at))
        }
    };
    let go = play_from(resume);
    let start_over = play_from(None);
    let (u, c, m) = (ui.clone(), ctx.clone(), media.clone());
    let info_run = (if resume.is_some() { "Resume" } else { "Play" }.to_owned(), go.clone());
    let info_open: Rc<dyn Fn(&El)> = Rc::new(move |el: &El| open_media_info(&u, &c, &m, el, Some(info_run.clone())));
    let row = episode_row(
        ui,
        EpisodeRow {
            code,
            name,
            art: item.art.clone(),
            facts,
            plot: info.and_then(|i| i.plot.clone()).map(|p| p.trim().to_owned()).unwrap_or_default(),
            finished,
            next,
            resume,
            fraction,
            go_label,
            go,
            start_over,
            info: info_open,
        },
        phone,
    );
    let key = (folder.to_owned(), season.unwrap_or(0), numbers.episode);
    ROWS.with(|r| r.borrow_mut().push((key, row.clone())));
    row
}

thread_local! {
    static ROWS: RefCell<Vec<((String, u32, u32), El)>> = const { RefCell::new(Vec::new()) };
}

/// Scrolls to the episode a show address meant, and asks the library again (main.ts
/// `afterShowPaint`, `refreshLibraryShow`).
fn after_paint(ui: &Ui, ctx: &Ctx, page: &BrowsePage, view: &ShowView, version: Signal<u32>, check_library: bool) {
    PAGE_ITEMS.with(|p| {
        let mut p = p.borrow_mut();
        p.retain(|(f, _)| *f != page.path);
        p.push((page.path.clone(), page.items.clone()));
    });
    let rows = ROWS.with(|r| std::mem::take(&mut *r.borrow_mut()));
    let raw = LANDING.with(|l| l.borrow().clone());
    let landing = raw.filter(|l| l.0 == page.path);
    if let Some((path, s, e)) = landing {
        if let Some((_, row)) = rows.iter().find(|(k, _)| *k == (path.clone(), s, e)) {
            LANDING.with(|l| *l.borrow_mut() = None);
            // `scrollIntoView({ block: "center" })`: a window-tall box around the row's middle,
            // once the row is laid out.
            center_when_laid_out(ui.clone(), row.clone(), 40);
        }
    }
    if check_library {
        refresh_library(ctx, view.show.save.id.clone(), version);
    }
}

/// main.ts `refreshLibraryShow`: looks the show up in the library again and bumps `version`
/// (a repaint) if anything changed, unless the page moved on meanwhile.
fn refresh_library(ctx: &Ctx, id: String, version: Signal<u32>) {
    let before = shows::library_show(&id);
    let (r, at) = (ctx.router.clone(), ctx.router.route_untracked());
    ctx.api.run(api::title(&id), &Cancel::new(), move |out| {
        if r.route_untracked() != at {
            return;
        }
        let fresh = match out {
            Ok(t) => Some(t),
            Err(e) if e.status == Some(404) => None,
            Err(_) => return,
        };
        match &fresh {
            Some(t) => shows::note_library_show(t),
            None => shows::note_library_missing(&id),
        }
        if before != fresh {
            version.update(|n| *n += 1);
        }
    });
}

fn center_when_laid_out(ui: Ui, row: El, tries: u32) {
    let u2 = ui.clone();
    ui.after(std::time::Duration::from_millis(50), move || {
        match row.bounds().filter(|b| b.size().height > 0.0) {
            Some(b) => {
                let (size, h) = (b.size(), u2.window_size().height);
                // The middle of the window below `html { scroll-padding-top: 4.75rem }`: a box
                // that tall above and below the row's middle lands it there.
                let reach = h + 76.0;
                let top = remedy_ui::core::Point::new(0.0, size.height / 2.0 - reach / 2.0);
                row.scroll_rect_into_view(remedy_ui::core::Rect::from_origin_size(top, remedy_ui::core::Size::new(size.width, reach)));
            }
            None if tries > 0 => center_when_laid_out(u2, row, tries - 1),
            None => {}
        }
    });
}

/// main.ts `openItem`: settings rows open their add-on's settings, folders open, actions
/// run, and anything else plays (an episode as its show's, with the show's name).
pub fn open_item(ctx: &Ctx, folder: &str, item: &BrowseItem, resume: Option<f64>) {
    let settings = item.settings_addon.clone().filter(|s| !s.is_empty()).or_else(|| shows::settings_addon(&item.path));
    if let Some(id) = settings {
        ctx.go(&Page::AddonSettings(id));
        return;
    }
    if item.path.is_empty() {
        return;
    }
    match item.kind {
        ItemKind::Open => {
            shows::note_path_label(&item.path, &item.label);
            if let Some(m) = media_of(folder, item).filter(|m| m.kind == MediaKind::Tv) {
                shows::note_opened_show(&item.path, ShowRef { id: m.save.id, name: m.save.name, art: item.art.clone(), info: item.info.clone() });
            }
            ctx.go(&Page::Browse(item.path.clone()));
        }
        ItemKind::Run => {
            if !item.path.to_ascii_lowercase().contains("plugin://") || ctx.is_remote() {
                if ctx.is_remote() {
                    ctx.activity.notice("Add-on tools run only on the PC running Portal.");
                }
                return;
            }
            let c = ctx.clone();
            ctx.api.run(api::run_plugin(&item.path), &Cancel::new(), move |out| {
                if let Err(e) = out {
                    c.activity.notice(shown_detail(&e));
                }
            });
        }
        ItemKind::Play => {
            let info = media_of(folder, item).map(|m| m.seed).or(item.info.clone());
            match episode_show(folder, item) {
                Some(show) => {
                    let (c, it) = (ctx.clone(), item.clone());
                    let sid = show.id.clone();
                    progress_name(ctx, &sid, item.show_title.as_deref(), move |name| {
                        play(
                            &c,
                            PlayTarget {
                                kind: PlayKind::Episode,
                                id: show.id.clone(),
                                file: it.path.clone(),
                                title: it.label.clone(),
                                season: show.season,
                                episode: Some(show.episode),
                                art: it.art.clone(),
                                show_title: Some(name.clone()),
                                show_id: Some(show.id.clone()),
                                info,
                                resume_seconds: resume,
                                manual_links: false,
                                progress_kind: TitleKind::Tv,
                                progress_id: show.id.clone(),
                                progress_name: name,
                                track_progress: true,
                            },
                        );
                    });
                }
                None => {
                    let mut t = crate::widgets::play::movie_target(&item.path, &item.path, &item.label, item.art.clone(), resume);
                    if item.title_kind == Some(TitleKind::Tv) {
                        // main.ts `fileTarget`: played as a file, not kept in history.
                        let id = if item.path.is_empty() { item.id.clone() } else { item.path.clone() };
                        t = PlayTarget { kind: PlayKind::File, id: id.clone(), file: id.clone(), resume_seconds: None, progress_id: id, track_progress: false, ..t };
                    }
                    t.info = info;
                    play(ctx, t);
                }
            }
        }
    }
}

/// The show's name for history (main.ts `progressName`): the name this page knows, else
/// the item's show title, else the library's, else what the folder is called.
fn progress_name(ctx: &Ctx, show_id: &str, show_title: Option<&str>, done: impl FnOnce(String) + 'static) {
    let known = shows::remembered_name(show_id);
    if !known.is_empty() && !shows::is_season_label(&known) {
        done(known);
        return;
    }
    if let Some(t) = show_title.filter(|t| !t.is_empty()) {
        done(t.to_owned());
        return;
    }
    let id = show_id.to_owned();
    ctx.api.run(api::title(show_id), &Cancel::new(), move |out| {
        if let Ok(t) = out {
            if !t.name.is_empty() && t.name != id && !shows::is_season_label(&t.name) {
                done(t.name);
                return;
            }
        }
        let label = shows::path_label(&id);
        done(if !label.is_empty() && !shows::is_season_label(&label) { label } else { shows::folder_label(&id) });
    });
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule(
        "text_input",
        Some("portal_search"),
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(Insets { top: 7.68, bottom: 7.68, start: 10.4, end: 10.4 }),
    );
    s.rule("text_input", Some("portal_search"), States::HOVER, StylePatch::default().border(k.gold));
    let _ = (Align::Center, Length::Auto);
}
