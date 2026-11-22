//! The frame of a page: its heading. The shell lays every page in `main`'s column (at most
//! 1240 px, centered, with its padding), so a page adds only its content.

use remedy_ui::core::Insets;
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

/// The page's `h1`: 32 px in the heading face, 0.6rem under it.
pub fn h1(ui: &Ui, text: &str) -> El {
    w::heading(ui, 1, text.to_owned()).layout(|l| l.margin = Insets { top: 0.0, bottom: 9.6, start: 0.0, end: 0.0 })
}

/// A page with only its heading, for a screen not yet ported; screens add children to it.
pub fn placeholder(ui: &Ui, heading: &str) -> El {
    w::column(ui).gap(12.0).fill_width().child(h1(ui, heading))
}
