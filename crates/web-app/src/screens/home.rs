//! Home: the source link and the Continue watching, Watchlist and Bookmarks shelves
//! (main.ts:397 `renderHome`, 420 `shelf`; plan P3).

use std::rc::Rc;

use portal_api::{Card, Home, Source};
use remedy_ui::core::Insets;
use remedy_ui::reactive::Signal;
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, List};
use crate::pages::Page;
use crate::widgets::activity::{describe_library, Library, Owner};
use crate::widgets::{card, page, shelf};
use crate::Ctx;

/// main.ts:140-141, word for word.
pub const WATCHLIST_EMPTY: &str = "Nothing on your watchlist yet. Use the Watchlist button on any movie or show to add it.";
pub const BOOKMARKS_EMPTY: &str = "No bookmarks yet. Use the Bookmark button on any movie, show or episode to keep it here.";
pub const CONTINUE_EMPTY: &str = "Nothing is in progress.";

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    // "Home / Loading your library." only when the load takes over 350 ms.
    let ticket = ctx.activity.begin(Owner::Page, describe_library(Library::Home), 350);
    let again = Signal::new(0u32);
    let home = ctx.api.resource(move || again.get(), |_| api::home());
    let source = ctx.api.resource(move || again.get(), |_| api::source());
    let editing: card::Editing = Signal::new(None);
    let refresh: Rc<dyn Fn()> = Rc::new(move || again.update(|n| *n += 1));

    let root = w::column(ui).fill_width();
    let (r2, u, c) = (root.clone(), ui.clone(), ctx.clone());
    // Drawn once both answered (a failed source only loses the link), as Promise.all does.
    let shown = Signal::new(false);
    root.bind(move || {
        let (h, err) = (home.get(), home.error());
        let done = !home.loading() && !source.loading() && (h.is_some() || err.is_some());
        if !done {
            return;
        }
        let src = source.get();
        let me = c.me.get();
        remedy_ui::reactive::untrack(|| {
            r2.clear();
            let body = match (h, err) {
                (Some(h), _) => page_body(&u, &c, &h, src.as_ref(), me.is_some_and(|m| !m.this_pc), editing, refresh.clone()),
                (None, Some(e)) => {
                    let detail = match e {
                        remedy_ui::reactive::FetchError::Failed(e) => api::shown_detail(&e),
                        _ => "Portal could not load this page.".to_owned(),
                    };
                    w::text(&u, detail).variant("error")
                }
                (None, None) => return,
            };
            let _ = r2.clone().child(body);
            if !shown.get_untracked().unwrap_or(true) {
                shown.set(true);
                c.activity.end(ticket);
            }
        });
    })
}

fn page_body(
    ui: &Ui,
    ctx: &Ctx,
    home: &Home,
    source: Option<&Source>,
    remote: bool,
    editing: card::Editing,
    refresh: Rc<dyn Fn()>,
) -> El {
    let p = |el: El, top: f32| el.layout(move |l| l.margin = Insets { top, bottom: 0.0, start: 0.0, end: 0.0 });
    let root = match source.and_then(|s| s.path.clone().filter(|p| !p.is_empty())) {
        Some(path) => {
            let name = source.and_then(|s| s.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| "Browse the source".into());
            w::row(ui).child(crate::widgets::link(ui, &ctx.router, &Page::Browse(path).path(), name, "", None))
        }
        None if remote => {
            w::text(ui, "Choose a video add-on on the PC running Portal to fill the shelves.").variant("muted")
        }
        None => w::row(ui)
            .child(crate::widgets::link(ui, &ctx.router, &Page::Source.path(), "Choose a video add-on", "", None))
            .child(w::text(ui, " to fill the shelves.").variant("muted")),
    };
    let narrow = card::is_narrow(ctx.width.get_untracked().unwrap_or(1280.0));
    let width = card::shelf_width(narrow);
    let on_play = card::play_card(ctx);
    let cards = |items: &[Card], list: Option<List>| -> Vec<El> {
        items
            .iter()
            .map(|it| {
                let tools = list.map(|l| card::list_tools(ui, ctx, l, it, editing, refresh.clone()));
                card::card(ui, ctx, it, tools, on_play.clone())
            })
            .collect()
    };
    let one = |title: &str, items: &[Card], empty: &str, list: Option<List>, top: f32| {
        p(shelf::shelf(ui, title, cards(items, list), empty, width), top)
    };
    w::column(ui)
        .fill_width()
        .child(page::h1(ui, "Home").layout(|l| l.margin = Insets::ZERO))
        // p { margin: 1em 0 } after the h1's 0.6rem: 16 px; the shelves' 1.6rem and 2.15rem
        // margins collapse with what is before them.
        .child(p(root, 16.0))
        .child(one("Continue watching", &home.continue_watching, CONTINUE_EMPTY, None, 25.6))
        .child(one("Watchlist", &home.watchlist, WATCHLIST_EMPTY, Some(List::Watchlist), 34.4))
        .child(one("Bookmarks", &home.bookmarks, BOOKMARKS_EMPTY, Some(List::Bookmarks), 34.4))
        .child(w::column(ui).height(34.4))
}
