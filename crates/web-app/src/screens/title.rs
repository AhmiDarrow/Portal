//! Details of a movie or show: Play, Resume, From start, Episodes (main.ts:1350-1468; plan P6).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, id: &str) -> El {
    let _ = ctx;
    let _ = id;
    page::placeholder(ui, "Details")
}
