//! Browse: a folder of the add-on, in-folder search, run items and settings links (main.ts:552 `renderBrowse`; plan P5).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, path: &str) -> El {
    let _ = ctx;
    let _ = path;
    page::placeholder(ui, "Browse")
}
