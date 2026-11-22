//! Browse: a folder of the add-on and in-folder search (main.ts:552 `renderBrowse`,
//! `paintBrowse`, `runSearch`; plan P5). A folder that is a show's page or one of its
//! seasons leads with the show ([`super::show::folder_view`]); any other folder is a grid of
//! cards under its heading. A folder seen before paints from the cache at once and is
//! checked again ("Refreshing" after 400 ms).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use portal_api::{BrowseItem, BrowsePage};
use remedy_ui::layout::{Align, Length};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail};
use crate::pages::Page;
use crate::screens::show::{self, open_item, open_media_info, BrowseSource};
use crate::widgets::activity::{describe_browse, describe_search, BrowseMode, Owner};
use crate::widgets::browse_card;
use crate::widgets::page::h1;
use crate::widgets::shows::{folder_label, path_label, Media};
use crate::Ctx;

/// Folders kept for an instant repaint (main.ts `BROWSE_CACHE_LIMIT`). [`set_cache_limit`]
/// changes it.
pub const CACHE_LIMIT: usize = 60;

thread_local! {
    static CACHE: RefCell<VecDeque<BrowsePage>> = const { RefCell::new(VecDeque::new()) };
    static LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(CACHE_LIMIT) };
    static SOURCE: RefCell<Option<BrowseSource>> = const { RefCell::new(None) };
}

pub fn set_cache_limit(n: usize) {
    LIMIT.with(|l| l.set(n));
}

fn cached(path: &str) -> Option<BrowsePage> {
    CACHE.with(|c| c.borrow().iter().find(|p| p.path == path).cloned())
}

fn cache(page: &BrowsePage) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        c.retain(|p| p.path != page.path);
        c.push_back(page.clone());
        while c.len() > LIMIT.with(std::cell::Cell::get) {
            c.pop_front();
        }
    });
}

fn heading(path: &str) -> String {
    if let Some(s) = SOURCE.with(|s| s.borrow().clone()).filter(|s| s.path == path && !s.name.is_empty()) {
        return s.name;
    }
    let l = path_label(path);
    if !l.is_empty() {
        return l;
    }
    let f = folder_label(path);
    if f.is_empty() { "Folder".into() } else { f }
}

/// What the page shows: a folder, or search results for a query.
#[derive(Clone, PartialEq)]
enum Shown {
    Nothing,
    Folder(BrowsePage),
    Results(String, BrowsePage),
    Error(String),
}

pub fn view(ui: &Ui, ctx: &Ctx, path: &str) -> El {
    let root = w::column(ui).fill_width();
    if path.trim().is_empty() {
        return root.child(w::text(ui, "Missing a folder.").variant("error"));
    }
    let shown = Signal::new(match cached(path) {
        Some(p) => Shown::Folder(p),
        None => Shown::Nothing,
    });
    let source = Signal::new(SOURCE.with(|s| s.borrow().clone()));
    let path = path.to_owned();
    load(ctx, &path, shown, source);
    let (r2, u, c) = (root.clone(), ui.clone(), ctx.clone());
    root.bind(move || {
        let now = shown.get();
        let src = source.get();
        c.width.get();
        c.me.get();
        untrack(|| {
            r2.clear();
            let el = match now {
                Shown::Nothing => return,
                Shown::Error(e) => w::text(&u, e).variant("error"),
                Shown::Folder(p) => paint(&u, &c, &p, None, shown, src),
                Shown::Results(q, p) => paint(&u, &c, &p, Some(q), shown, src),
            };
            let _ = r2.clone().child(el);
        });
    })
}

fn load(ctx: &Ctx, path: &str, shown: Signal<Shown>, source: Signal<Option<BrowseSource>>) {
    let was_cached = cached(path).is_some();
    let name = heading(path);
    let ticket = ctx.activity.begin(Owner::Page, describe_browse(&name, if was_cached { BrowseMode::Refresh } else { BrowseMode::Open }), if was_cached { 400 } else { 0 });
    let start = move |c: &Ctx, p: String| {
        let c2 = c.clone();
        let open = Rc::new(std::cell::Cell::new(true));
        // Kodi down: say it is being started (main.ts `noteKodi`).
        let (c3, o2, n2) = (c.clone(), open.clone(), heading(&p));
        c.api.run(api::status(), &Cancel::new(), move |out| {
            if let Ok(s) = out {
                if o2.get() && s.state != portal_api::EngineState::Running {
                    let t = c3.activity.begin(Owner::Page, describe_browse(&n2, BrowseMode::Starting), 0);
                    let _ = t;
                }
            }
        });
        c.api.run(api::browse(&p), &Cancel::new(), move |out| {
            open.set(false);
            c2.activity.end(ticket);
            c2.activity.end_page();
            // A search started meanwhile owns the page; this folder's answer only refreshes the cache.
            let searching = shown.get_untracked().is_ok_and(|s| matches!(s, Shown::Results(..)));
            match out {
                Ok(page) => {
                    cache(&page);
                    if !searching {
                        shown.set(Shown::Folder(page));
                        c2.activity.notice("");
                    }
                }
                Err(e) if !searching => {
                    if was_cached {
                        c2.activity.notice(shown_detail(&e));
                    } else {
                        shown.set(Shown::Error(shown_detail(&e)));
                    }
                }
                Err(_) => {}
            }
        });
    };
    if SOURCE.with(|s| s.borrow().is_none()) {
        let (c, p) = (ctx.clone(), path.to_owned());
        ctx.api.run(api::source(), &Cancel::new(), move |out| {
            if let Some(s) = out.ok().as_ref().and_then(BrowseSource::of) {
                SOURCE.with(|x| *x.borrow_mut() = Some(s.clone()));
                source.set(Some(s));
            }
            start(&c, p);
        });
    } else {
        start(ctx, path.to_owned());
    }
}

/// The search form (`form.search`): the field, Search, and Clear while results show.
fn search_form(ui: &Ui, ctx: &Ctx, folder: &str, query: Option<&str>, shown: Signal<Shown>) -> El {
    let field = w::text_input(ui, "Search this add-on").placeholder("Search this add-on");
    if let Some(q) = query {
        field.set_value(q);
    }
    let (c, f2) = (ctx.clone(), field.clone());
    let run = Rc::new(move || run_search(&c, &f2.value(), shown, &f2));
    let r2 = run.clone();
    let input = field.clone().on_submit(move || r2()).el().variant("portal_search").layout(|l| {
        l.grow = 1.0;
        l.shrink = 1.0;
        l.basis = Length::Px(224.0);
        l.min_width = Length::Px(0.0);
        l.max_width = Length::Px(448.0);
    });
    let mut form = ui
        .el("p_search")
        .row()
        .align(Align::Center)
        .gap(8.0)
        .fill_width()
        .layout(|l| {
            l.wrap = true;
            l.margin.top = 9.6;
            l.margin.bottom = 16.0;
            l.shrink = 0.0;
        })
        .child(input)
        .child(crate::widgets::button(ui, "Search", move || run()));
    if query.is_some() {
        let (c, p) = (ctx.clone(), folder.to_owned());
        form = form.child(crate::widgets::button(ui, "Clear", move || {
            shown.set(cached(&p).map_or(Shown::Nothing, Shown::Folder));
            c.router.replace(&Page::Browse(p.clone()).path());
        }));
    }
    form
}

thread_local! {
    static SEARCH_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn run_search(ctx: &Ctx, query: &str, shown: Signal<Shown>, field: &remedy_ui::view::TextField) {
    let q = query.trim().to_owned();
    if q.is_empty() {
        ctx.activity.notice("Enter a search.");
        field.el().focus();
        field.select_all();
        return;
    }
    // A newer search on the same page wins, whichever answers first.
    let mine = SEARCH_SEQ.with(|s| {
        s.set(s.get() + 1);
        s.get()
    });
    let ticket = ctx.activity.begin(Owner::Page, describe_search(&q, false), 0);
    let c = ctx.clone();
    ctx.api.run(api::search(&q), &Cancel::new(), move |out| {
        c.activity.end(ticket);
        if SEARCH_SEQ.with(std::cell::Cell::get) != mine {
            return;
        }
        match out {
            Ok(page) => {
                shown.set(Shown::Results(q, page));
                c.activity.notice("");
            }
            Err(e) => c.activity.notice(shown_detail(&e)),
        }
    });
}

fn paint(ui: &Ui, ctx: &Ctx, page: &BrowsePage, query: Option<String>, shown: Signal<Shown>, source: Option<BrowseSource>) -> El {
    let phone = ctx.width.get_untracked().unwrap_or(1280.0) <= 720.0;
    let form = search_form(ui, ctx, &page.path, query.as_deref(), shown);
    if query.is_none() {
        if let Some(el) = show::folder_view(ui, ctx, page, &heading(&page.path), source.clone(), form.clone()) {
            return el;
        }
    }
    let ids: Vec<String> = page.items.iter().filter_map(|i| crate::widgets::shows::media_of(&page.path, i).map(|m| m.save.id)).collect();
    let (r, at) = (ctx.router.clone(), ctx.router.route_untracked());
    crate::widgets::saved::load(ctx, ids, move || r.route_untracked() == at);
    let title = match &query {
        Some(q) if !q.trim().is_empty() => format!("Results for “{}”", q.trim()),
        Some(_) => "Results".into(),
        None => heading(&page.path),
    };
    let mut kicker = w::row(ui).gap(0.0).child(w::text(ui, if query.is_some() { "Search" } else { "Browse" }).variant("muted"));
    if !ctx.is_remote() {
        kicker = kicker
            .child(w::text(ui, " · ").variant("muted"))
            .child(crate::widgets::link(ui, &ctx.router, &Page::Source.path(), "Source", "", None));
    }
    let head = w::column(ui).child(kicker.layout(|l| l.margin.bottom = 16.0)).child(h1(ui, &title).layout(|l| l.margin.bottom = 0.0));
    let items = if page.items.is_empty() {
        let text = match query.as_deref().map(str::trim) {
            Some(q) if !q.is_empty() => format!("No results for “{q}”."),
            Some(_) => "No results.".into(),
            None => "This folder is empty.".into(),
        };
        w::text(ui, text).variant("muted")
    } else {
        let (c1, c2, f1, f2) = (ctx.clone(), ctx.clone(), page.path.clone(), page.path.clone());
        let items = page.items.clone();
        let u2 = ui.clone();
        let open: browse_card::Open = Rc::new(move |item: &BrowseItem| open_item(&c1, &f1, item, None));
        let info: browse_card::OpenInfo = Rc::new(move |m: &Media, el: &El, label: String| {
            let found = items.iter().find(|i| crate::widgets::shows::media_of(&f2, i).as_ref() == Some(m)).cloned();
            let (c3, f3) = (c2.clone(), f2.clone());
            let run: Rc<dyn Fn()> = Rc::new(move || {
                if let Some(i) = &found {
                    open_item(&c3, &f3, i, None);
                }
            });
            open_media_info(&u2, &c2, m, el, Some((label, run)));
        });
        let cards = page.items.iter().map(|i| browse_card::card(ui, ctx, &page.path, i, None, open.clone(), info.clone())).collect();
        browse_card::grid(ui, cards, phone)
    };
    w::column(ui).fill_width().child(head).child(form).child(items)
}
