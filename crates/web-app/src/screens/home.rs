//! Home: the source link and the Continue watching, Watchlist and Bookmarks shelves (main.ts:397 `renderHome`; plan P3).

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::{api, Ctx};

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    // Until P3 builds the shelves: proof that the typed API answers.
    let home = ctx.api.resource(|| (), |_| api::home());
    page::placeholder(ui, "Home").child(w::text(ui, move || match (home.get(), home.error()) {
        (Some(h), _) => format!(
            "Continue watching: {} · Watchlist: {} · Bookmarks: {}",
            h.continue_watching.len(),
            h.watchlist.len(),
            h.bookmarks.len()
        ),
        (None, Some(e)) => format!("{e:?}"),
        (None, None) => "Loading…".to_owned(),
    }))
}
