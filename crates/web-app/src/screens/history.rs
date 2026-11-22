//! History: rows with rename, remove (slot-checked), play and from start (main.ts:1665-1732; plan P4).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    page::placeholder(ui, "History")
}
