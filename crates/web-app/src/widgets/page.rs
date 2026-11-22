//! The frame of a page: its heading. The shell lays every page in `main`'s column (at most
//! 1240 px, centered, with its padding), so a page adds only its content.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use remedy_ui::core::Insets;
use remedy_ui::layout::{Align, Length};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::{mix, Look};

/// The page's `h1`: 32 px in the heading face, 0.6rem under it.
pub fn h1(ui: &Ui, text: &str) -> El {
    w::heading(ui, 1, text.to_owned()).layout(|l| l.margin = Insets { top: 0.0, bottom: 9.6, start: 0.0, end: 0.0 })
}

/// An `h1` with a count after it (`h1 .count`: 0.55em in the text face, muted, middle-aligned).
pub fn h1_count(ui: &Ui, text: &str, count: Option<usize>) -> El {
    let Some(n) = count else { return h1(ui, text) };
    // The heading's own space before the count: a 32 px space in the heading face.
    w::row(ui)
        .align(Align::Center)
        .gap(0.0)
        .layout(|l| {
            l.wrap = true;
            l.margin = Insets { top: 0.0, bottom: 9.6, start: 0.0, end: 0.0 };
        })
        .child(w::heading(ui, 1, format!("{text} ")).layout(|l| l.margin = Insets::ZERO))
        .child(ui.el("p_count").text(n.to_string()).nowrap())
}

/// A page with only its heading, for a screen not yet ported; screens add children to it.
pub fn placeholder(ui: &Ui, heading: &str) -> El {
    w::column(ui).gap(12.0).fill_width().child(h1(ui, heading))
}

/// main.ts `renderKeepingRow`: a library page that paints again after a rename or a removal
/// puts keyboard focus back on the row that had it, or the one now in its place.
#[derive(Clone, Default)]
pub struct KeepRow {
    rows: Rc<RefCell<Vec<El>>>,
    at: Rc<Cell<Option<usize>>>,
}

impl KeepRow {
    /// The rows just painted, in order.
    pub fn set_rows(&self, rows: Vec<El>) {
        *self.rows.borrow_mut() = rows;
    }

    /// Before a repaint: notes which row has focus.
    pub fn note(&self) {
        let at = self.rows.borrow().iter().position(|r| r.states().contains(States::FOCUS_WITHIN));
        self.at.set(at);
    }

    /// After a repaint: focus goes to the first control of that row, or of the last one.
    // shortcut: with no rows left focus stays where it fell (the old page focuses the h1 with
    // tabIndex -1); upgrade when an El can take focus without joining the Tab order.
    pub fn restore(&self, ui: &Ui) {
        let Some(i) = self.at.take() else { return };
        let rows = self.rows.clone();
        ui.after(std::time::Duration::ZERO, move || {
            let rows = rows.borrow();
            if let Some(r) = rows.get(i.min(rows.len().saturating_sub(1))) {
                r.focus_first();
            }
        });
    }
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule("p_count", None, States::NONE, StylePatch::default().text_size(17.6).foreground(k.muted));
    // .empty-state; its border is dashed in the old page (ENGINE GAP: no dashed borders).
    s.rule(
        "p_empty_state",
        None,
        States::NONE,
        StylePatch::default()
            .padding(Insets { top: 24.0, bottom: 24.0, start: 22.4, end: 22.4 })
            .border(mix(k.gold, 0.35, k.line))
            .border_width(1.0)
            .radius(14.0)
            .background(mix(k.gold, 0.06, k.raised)),
    );
}

/// `.empty-state`'s width: at most 36rem.
pub fn empty_state(ui: &Ui) -> El {
    ui.el("p_empty_state").column().layout(|l| l.max_width = Length::Px(576.0))
}
