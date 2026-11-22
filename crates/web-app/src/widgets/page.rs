//! The frame of a page: its padding and heading.

use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

/// A page with only its heading, for a screen not yet ported; screens add children to it.
pub fn placeholder(ui: &Ui, heading: &str) -> El {
    w::column(ui).gap(12.0).padding(24.0).fill_width().child(w::heading(ui, 1, heading.to_owned()))
}
