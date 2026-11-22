//! Pieces of the long settings-like pages (Source, an add-on's settings, Settings): the
//! `.server-card` section, the `.jump-bar` that scrolls to one, the `.cats` pill buttons,
//! the On/Off switch (`input.toggle`), a drop-down in Portal's look, and [`keep_focus`] for
//! a repainted list.

use std::rc::Rc;

use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{CornerRadii, Insets, Rect};
use remedy_ui::input::Key;
use remedy_ui::layout::Length;
use remedy_ui::paint::Brush;
use remedy_ui::reactive::{untrack, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::{mix, Look};

/// A `section.server-card`: the raised surface, a hairline, 14 px corners.
pub fn card(ui: &Ui, label: &str) -> El {
    ui.el("p_server_card")
        .column()
        .fill_width()
        // `margin: 0.55rem 0`, collapsed between cards: 0.55rem apart.
        .layout(|l| l.margin.top = 8.8)
        .semantics(Semantics::new(Role::Region).label(label.to_owned()))
}

/// The card's `h2`.
pub fn h2(ui: &Ui, text: &str) -> El {
    w::heading(ui, 2, text.to_owned()).layout(|l| l.margin.bottom = 9.6)
}

/// A `p` in a card (`.source-card > p`: 0.35rem above and below); `variant` "muted" or "error".
pub fn para(ui: &Ui, text: impl Into<String>, variant: &'static str) -> El {
    w::text(ui, text.into()).variant(variant).fill_width().layout(|l| l.margin.bottom = 5.6)
}

/// `.cats`: pill buttons in a row that wraps (one scrolling row on a phone when `scroll`).
pub fn cats(ui: &Ui, label: &str, phone_scroll: bool, phone: bool) -> El {
    let row = ui.el("p_cats").row().gap(6.4).fill_width().semantics(Semantics::new(Role::Navigation).label(label.to_owned()));
    if phone_scroll && phone {
        row.scroll().scroll_x().layout(|l| {
            l.max_height = Length::Auto;
            l.padding.bottom = 3.2;
        })
    } else {
        row.layout(|l| l.wrap = true)
    }
}

/// One `.cats` button; `current` marks it (`aria-current`).
pub fn cat_button(ui: &Ui, label: &str, current: bool, on_press: impl Fn() + 'static) -> El {
    let b = crate::widgets::button(ui, label.to_owned(), on_press).variant("cat").layout(|l| l.shrink = 0.0);
    b.state(States::SELECTED, current);
    b
}

/// `.jump-bar`: buttons that scroll to the page's sections.
pub fn jump_bar(ui: &Ui, phone: bool, sections: &[(&str, El)]) -> El {
    // 0.6rem under it, collapsed with the first card's 0.55rem.
    let mut row = cats(ui, "Sections", true, phone).layout(|l| l.margin.bottom = 0.8);
    for (name, target) in sections {
        let t = target.clone();
        row = row.child(crate::widgets::button(ui, (*name).to_owned(), move || t.scroll_into_view()).variant("jump").layout(|l| l.shrink = 0.0));
    }
    row
}

/// The On/Off switch (`input.toggle`): 2.75rem by 1.5rem, its knob gold and to the right
/// when on. `label` is what a screen reader says; the page shows its own name beside it.
pub fn toggle(ui: &Ui, label: &str, on: Signal<bool>, enabled: bool, on_flip: impl Fn(bool) + 'static) -> El {
    let label = label.to_owned();
    let t = ui
        .el("p_toggle")
        .width(44.0)
        .height(24.0)
        .layout(|l| l.shrink = 0.0)
        .focusable(enabled)
        .paint(|cx| {
            let d = 16.8;
            let on = cx.states.contains(States::CHECKED);
            let x = if on { 3.2 + 19.2 } else { 3.2 };
            let y = (cx.size.height - d) / 2.0;
            cx.list.fill_rounded(Rect::new(x, y, x + d, y + d), CornerRadii::all(d / 2.0), Brush::Solid(cx.style.foreground));
        })
        .disabled(!enabled);
    let flip = Rc::new(move || {
        if !enabled {
            return;
        }
        let now = !on.get_untracked().unwrap_or(false);
        on_flip(now);
    });
    let f2 = flip.clone();
    let t2 = t.clone();
    t.on_click(move || flip())
        .on_key(move |e| {
            if e.pressed && matches!(e.key, Key::Space | Key::Enter) {
                f2();
                return true;
            }
            false
        })
        .bind(move || t2.state(States::CHECKED, on.get()))
        .semantics(move || Semantics::new(Role::Switch).label(label.clone()).checked(on.get()).action(Action::Click))
}

/// A drop-down in Portal's look (`select`): the well, a hairline, 8 px corners.
pub fn select(ui: &Ui, label: &str, options: Vec<String>, chosen: Signal<usize>) -> El {
    // The chosen option at the start, the arrow at the end, as a `select` draws them.
    let s = w::select(ui, label.to_owned(), options, chosen).variant("p_select");
    if let Some(value) = ui.tree().children(s.id()).first() {
        let _ = ui.element(*value).layout(|l| l.grow = 1.0);
    }
    s
}

/// Repaints `root` with `paint` and puts focus back on the control in the same place
/// (main.ts `keepFocus`), so a key press on a button the repaint replaces keeps it.
pub fn keep_focus(ui: &Ui, root: &El, paint: impl FnOnce()) {
    let tree = ui.tree();
    let controls = || tree.descendants(root.id()).into_iter().filter(|i| tree.is_focusable(*i)).collect::<Vec<_>>();
    let index = tree.focused().and_then(|f| controls().iter().position(|i| *i == f));
    untrack(paint);
    let Some(index) = index else { return };
    if tree.focused().is_some_and(|f| tree.contains(f)) {
        return;
    }
    if let Some(id) = controls().get(index) {
        ui.element(*id).focus();
    }
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let pad = |y: f32, x: f32| Insets { top: y, bottom: y, start: x, end: x };
    s.rule(
        "p_server_card",
        None,
        States::NONE,
        StylePatch::default().background(k.raised).border(k.line).border_width(1.0).radius(14.0).padding(pad(13.6, 14.4)),
    );
    s.rule("p_server_card", None, States::HOVER, StylePatch::default().border(mix(k.gold, 0.48, k.line)));
    // .cats button: pills; the current one gold on a gold wash.
    for v in ["cat", "jump"] {
        s.rule("p_button", Some(v), States::NONE, StylePatch::default().radius(999.0));
        s.rule(
            "p_button",
            Some(v),
            States::SELECTED,
            StylePatch::default().border(k.gold).foreground(k.gold).background(mix(k.gold, 0.14, k.bg)),
        );
    }
    s.rule("p_button", Some("jump"), States::NONE, StylePatch::default().padding(pad(4.48, 11.2)).text_size(14.72));
    // input.toggle
    s.rule(
        "p_toggle",
        None,
        States::NONE,
        StylePatch::default().background(k.well).border(k.line).border_width(1.0).radius(99.0).foreground(k.muted),
    );
    s.rule(
        "p_toggle",
        None,
        States::CHECKED,
        StylePatch::default().border(k.gold).background(mix(k.gold, 0.28, k.well)).foreground(k.gold),
    );
    s.rule("p_toggle", None, States::DISABLED, StylePatch::default().opacity(0.45));
    // select: the well, a hairline, 8 px corners, gold under the pointer.
    s.rule(
        "button",
        Some("p_select"),
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(pad(6.72, 12.0))
            .weight(400.0)
            .text_size(16.0),
    );
    s.rule("button", Some("p_select"), States::HOVER, StylePatch::default().border(k.gold));
    s.rule("button", Some("p_select"), States::DISABLED, StylePatch::default().opacity(0.45));
    // A password field looks like the text fields (`input[type=password]`).
    s.rule(
        "secret_input",
        Some("portal_search"),
        States::NONE,
        StylePatch::default()
            .background(k.well)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(pad(7.68, 10.4)),
    );
    s.rule("secret_input", Some("portal_search"), States::HOVER, StylePatch::default().border(k.gold));
    s.rule("text_input", Some("portal_search"), States::DISABLED, StylePatch::default().opacity(0.45));
    s.rule("secret_input", Some("portal_search"), States::DISABLED, StylePatch::default().opacity(0.45));
}
