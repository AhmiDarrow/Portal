//! The Info dialog: hero, poster, facts, rating, people, sources and the online refresh (info.ts:35-470; plan P4). A modal over any page; `view` builds its layer.

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    w::column(ui)
}
