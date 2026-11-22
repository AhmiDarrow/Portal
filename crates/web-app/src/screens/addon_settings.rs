//! One add-on's settings, saved per field (main.ts:2377-2661; plan P12).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx, addon: &str) -> El {
    let _ = ctx;
    let _ = addon;
    page::placeholder(ui, "Add-on settings")
}
