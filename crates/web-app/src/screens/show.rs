//! Show: a show's hero, season tabs and episodes, opened at a season or where watching left off (main.ts:887-1123; plan P6).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, id: &str, season: Option<u32>) -> El {
    let _ = ctx;
    let _ = id;
    let _ = season;
    page::placeholder(ui, "Show")
}
