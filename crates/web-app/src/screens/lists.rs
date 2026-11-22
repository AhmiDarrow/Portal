//! The Watchlist and Bookmarks pages (main.ts:1638 `renderList`; plan P4): the heading with
//! how many there are, then a grid of cards with Rename and Remove, or the empty state with a
//! link to browse the add-on.

use std::rc::Rc;

use portal_api::CardList;
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::{self, shown_detail, List};
use crate::pages::Page;
use crate::screens::browse;
use crate::screens::home::{BOOKMARKS_EMPTY, WATCHLIST_EMPTY};
use crate::screens::show::BrowseSource;
use crate::widgets::activity::{describe_library, Library, Owner};
use crate::widgets::page::{self, KeepRow};
use crate::widgets::{browse_card, card};
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, list: List) -> El {
    let editing: card::Editing = Signal::new(None);
    let shown = Signal::new(None::<Result<(CardList, Option<BrowseSource>), String>>);
    let again = Signal::new(0u32);
    let keep = KeepRow::default();
    let root = w::column(ui).fill_width();
    let (c, at) = (ctx.clone(), ctx.router.route_untracked());
    // Every paint asks again, as `render` does: after a rename or a removal too.
    let load = root.clone().bind(move || {
        let _ = again.get();
        untrack(|| fetch(&c, list, at.clone(), shown));
    });
    let k2 = keep.clone();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        k2.note();
        again.update(|n| *n += 1);
    });
    let (r2, u, c) = (load.clone(), ui.clone(), ctx.clone());
    load.bind(move || {
        let Some(now) = shown.get() else { return };
        let narrow = c.width.get() <= 720.0;
        untrack(|| {
            r2.clear();
            let el = match now {
                Err(e) => w::text(&u, e).variant("error"),
                Ok((body, source)) => paint(&u, &c, list, &body, source, narrow, editing, refresh.clone(), &keep),
            };
            let _ = r2.clone().child(el);
            keep.restore(&u);
        });
    })
}

fn fetch(ctx: &Ctx, list: List, at: remedy_ui::router::Route, shown: Signal<Option<Result<(CardList, Option<BrowseSource>), String>>>) {
    let kind = if list == List::Watchlist { Library::Watchlist } else { Library::Bookmarks };
    let ticket = ctx.activity.begin(Owner::Page, describe_library(kind), 350);
    let c = ctx.clone();
    let here = move |c: &Ctx| c.router.route_untracked() == at;
    ctx.api.run(api::list(list), &Cancel::new(), move |out| {
        if !here(&c) {
            return;
        }
        let body = match out {
            Ok(b) => b,
            Err(e) => {
                c.activity.end(ticket);
                shown.set(Some(Err(shown_detail(&e))));
                return;
            }
        };
        // An empty list links to the add-on, so it is looked up if not known yet.
        let known = browse::known_source();
        if !body.items.is_empty() || known.is_some() {
            c.activity.end(ticket);
            shown.set(Some(Ok((body, known))));
            return;
        }
        let c2 = c.clone();
        c.api.run(api::source(), &Cancel::new(), move |out| {
            let found = out.ok().as_ref().and_then(BrowseSource::of);
            if let Some(s) = &found {
                browse::note_source(s.clone());
            }
            if here(&c2) {
                c2.activity.end(ticket);
                shown.set(Some(Ok((body, found))));
            }
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn paint(ui: &Ui, ctx: &Ctx, list: List, body: &CardList, source: Option<BrowseSource>, narrow: bool, editing: card::Editing, refresh: Rc<dyn Fn()>, keep: &KeepRow) -> El {
    let heading = if list == List::Watchlist { "Watchlist" } else { "Bookmarks" };
    let n = body.items.len();
    let head = page::h1_count(ui, heading, (n > 0).then_some(n));
    if n == 0 {
        let empty = if list == List::Watchlist { WATCHLIST_EMPTY } else { BOOKMARKS_EMPTY };
        // .empty-state p { margin: 0 0 0.5rem }
        let mut state = page::empty_state(ui).child(w::text(ui, empty).variant("muted").layout(|l| l.margin.bottom = 8.0));
        if let Some(s) = source {
            let name = if s.name.is_empty() { "the add-on".to_owned() } else { s.name.clone() };
            state = state.child(w::row(ui).child(crate::widgets::link(ui, &ctx.router, &Page::Browse(s.path).path(), format!("Browse {name}"), "", None)).layout(|l| l.margin.bottom = 8.0));
        }
        keep.set_rows(Vec::new());
        return w::column(ui).fill_width().child(head).child(state);
    }
    let on_play = card::play_card(ctx);
    let cards: Vec<El> = body
        .items
        .iter()
        .map(|it| {
            let tools = card::list_tools(ui, ctx, list, it, editing, refresh.clone());
            card::card(ui, ctx, it, Some(tools), on_play.clone())
        })
        .collect();
    keep.set_rows(cards.clone());
    w::column(ui).fill_width().child(head).child(browse_card::grid(ui, cards, narrow))
}
