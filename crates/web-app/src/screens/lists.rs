//! The Watchlist and Bookmarks pages (main.ts:1638; plan P4).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, list: crate::api::List) -> El {
    let heading = match list {
        crate::api::List::Watchlist => "Watchlist",
        crate::api::List::Bookmarks => "Bookmarks",
    };
    let _ = ctx;
    page::placeholder(ui, heading)
}
