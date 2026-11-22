//! Add-on prompts: text (with a file picker), select, yes/no, ok and progress, while playing and in settings (player.ts:2438-2685; plans P7, P12). A modal layer.

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    w::column(ui)
}
