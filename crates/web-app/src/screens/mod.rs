//! One module per screen. Each has `pub fn view(ui, ctx, ..) -> El`; [`page`] picks the one
//! the route names. Screens talk to the server only through [`crate::api`] and move between
//! pages with [`Ctx::go`](crate::Ctx::go).

pub mod addon_settings;
pub mod browse;
pub mod history;
pub mod home;
pub mod info;
pub mod lists;
pub mod pairing;
pub mod player;
pub mod prompts;
pub mod settings;
pub mod show;
pub mod source;
pub mod title;

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::api::List;
use crate::pages::Page;
use crate::widgets::page;
use crate::Ctx;

/// The screen for `p`.
pub fn page(ui: &Ui, ctx: &Ctx, p: &Page) -> El {
    match p {
        Page::Home => home::view(ui, ctx),
        Page::Browse(path) => browse::view(ui, ctx, path),
        Page::Title(id) => title::view(ui, ctx, id),
        Page::Show { id, season } => show::view(ui, ctx, id, *season),
        Page::Watchlist => lists::view(ui, ctx, List::Watchlist),
        Page::Bookmarks => lists::view(ui, ctx, List::Bookmarks),
        Page::History => history::view(ui, ctx),
        Page::Source => source::view(ui, ctx),
        Page::AddonSettings(addon) => addon_settings::view(ui, ctx, addon),
        Page::Server => settings::view(ui, ctx),
        Page::Cast => cast_elsewhere(ui, ctx),
        Page::NotFound => not_found(ui, ctx),
    }
}

/// main.ts:371, word for word.
fn not_found(ui: &Ui, ctx: &Ctx) -> El {
    page::placeholder(ui, "Page not found")
        .child(w::text(ui, "Portal has no page at this address.").variant("muted"))
        .child(ctx.router.link_to(ui, "/", "Back to Home"))
}

/// The TV-browser receiver stays on the old page (port plan, Decisions).
fn cast_elsewhere(ui: &Ui, ctx: &Ctx) -> El {
    let app = ctx.app.clone();
    page::placeholder(ui, "TV").child(w::link(ui, "Open the TV receiver", move || {
        app.open_url("/#/cast", |_| {});
    }))
}
