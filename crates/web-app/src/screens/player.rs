//! The player dock: stage, controls, side panel, theater and mini player, casting (player.ts, stage.ts; plans P7-P10). Built once by the shell under every page; empty until something plays.

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    w::column(ui)
}
