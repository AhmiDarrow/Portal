//! Portal's own settings ("Settings" in the nav, `#/server`): appearance, playback, cast, devices, subtitles, media info, Kodi, updates (main.ts:2744-3709; plan P11).

use remedy_ui::{El, Ui};

use crate::widgets::page;
use crate::Ctx;

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    let _ = ctx;
    page::placeholder(ui, "Settings")
}
