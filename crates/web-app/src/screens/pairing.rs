//! Pairing this device with the PC (pair.ts:11-25; plan P2): shown by the shell instead of the pages after a 401 with `{"pair": true}`.

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    page::placeholder(ui, "Pair this device")
}
