//! A yes/no question in Portal's theme, where the old page asked with `window.confirm`: the
//! add-on prompt's frosted layer and card, the question, the action and Cancel. Tab stays
//! inside, Escape cancels, and focus goes back where it was.

use std::cell::RefCell;
use std::rc::Rc;

use remedy_ui::a11y::{Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::input::Key;
use remedy_ui::layout::{Align, Justify, Length, Position};
use remedy_ui::{El, Ui};

use crate::widgets::button;

/// Asks `question`; `ok` labels the button that runs `on_ok`.
pub fn confirm(ui: &Ui, question: &str, ok: &str, on_ok: impl Fn() + 'static) {
    let back = ui.tree().focused().map(|f| ui.element(f));
    let layer: Rc<RefCell<Option<El>>> = Rc::default();
    let close = {
        let layer = layer.clone();
        Rc::new(move || {
            if let Some(l) = layer.borrow_mut().take() {
                l.remove();
            }
            if let Some(b) = &back {
                b.focus();
            }
        })
    };
    let (c1, c2, c3) = (close.clone(), close.clone(), close);
    let yes = button(ui, ok, move || {
        c1();
        on_ok();
    });
    let card = ui
        .el("p_prompt_card")
        .column()
        .semantics(Semantics::new(Role::Dialog).label(question.to_owned()).modal(true))
        .child(ui.el("p_prompt_copy").text(question.to_owned()))
        .child(
            ui.el("p_prompt_actions")
                .row()
                .layout(|l| {
                    l.margin.top = 12.8;
                    l.wrap = true;
                    l.gap = (8.0, 8.0);
                })
                .child(yes.clone())
                .child(button(ui, "Cancel", move || c2())),
        )
        .on_key(move |e| {
            if e.pressed && e.key == Key::Escape {
                c3();
                return true;
            }
            false
        });
    let c4 = card.clone();
    let el = ui
        .el("p_prompt")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        .focus_scope()
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
            l.width = Length::Percent(1.0);
            l.height = Length::Percent(1.0);
        })
        .padding(Insets::all(16.0))
        .child(card)
        .on_resize(move |size| {
            let _ = c4.clone().layout(move |l| l.width = Length::Px(448.0_f32.min(size.width - 32.0).max(0.0)));
        });
    *layer.borrow_mut() = Some(ui.overlay(el));
    yes.focus();
}
