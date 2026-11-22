//! Portal's buttons and links, as styles.css draws `button` and `a`: a bordered button on
//! the page's background with a gold border under the pointer (`quiet`: no border until
//! then), and gold links underlined only under the pointer.

use std::rc::Rc;

use remedy_ui::a11y::{Action, Current, Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::input::{Buttons, PointerPhase};
use remedy_ui::layout::Align;
use remedy_ui::reactive::Signal;
use remedy_ui::router::{Route, Router};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::{IntoProp, RichText, TextStylePatch};
use remedy_ui::{El, Ui};

use crate::theme::{mix, Look};

/// A text button (`button`); `.variant("quiet")` for the borderless one.
#[track_caller]
pub fn button(ui: &Ui, label: impl IntoProp<String>, on_press: impl Fn() + 'static) -> El {
    let label = label.into_prop();
    ui.el("p_button")
        .row()
        .align(Align::Center)
        .text(label.clone().map(RichText::plain))
        .nowrap()
        .on_click(on_press)
        .semantics(label.map(|l| Semantics::new(Role::Button).label(l).action(Action::Click)))
}

/// A link to the route `path` (an `<a href="#/path">` in the accessibility mirror): a press
/// pushes the route; a middle or Ctrl/Cmd press opens it in a new tab. `variant` picks the
/// look: "" (gold, underlined under the pointer), `nav`, `meta` (a card's muted links),
/// `brand`, `pill`. `current` marks it as the page shown (`aria-current="page"`).
#[track_caller]
pub fn link(
    ui: &Ui,
    router: &Router,
    path: &str,
    label: impl Into<String>,
    variant: &'static str,
    current: Option<Signal<bool>>,
) -> El {
    let label: String = label.into();
    let href = Route::parse(path).href();
    let hovered = Signal::new(false);
    let underline = variant.is_empty();
    let text_label = label.clone();
    let (r1, r2, p1, p2) = (router.clone(), router.clone(), path.to_owned(), path.to_owned());
    let elsewhere = Rc::new(std::cell::Cell::new(false));
    let e2 = elsewhere.clone();
    let el = ui
        .el("p_link")
        .variant(variant)
        .text(move || {
            let mut p = TextStylePatch::default();
            p.underline = Some(underline && hovered.get());
            RichText::new().styled(&text_label, p)
        })
        .nowrap()
        .on_hover(move |on| hovered.set(on))
        .on_click(move || {
            if e2.replace(false) {
                r1.open_elsewhere(&p1);
            } else {
                r1.push(&p1);
            }
        })
        .on_pointer(move |e| {
            if e.phase != PointerPhase::Down {
                return false;
            }
            if e.buttons.contains(Buttons::MIDDLE) {
                r2.open_elsewhere(&p2);
                return true;
            }
            elsewhere.set(e.modifiers.ctrl || e.modifiers.meta);
            false
        });
    let el2 = el.clone();
    el.bind(move || {
        let on = current.is_some_and(|c| c.get());
        el2.state(States::SELECTED, on);
    })
    .semantics(move || {
        let s = Semantics::new(Role::Link).label(label.clone()).action(Action::Click).url(href.clone());
        if current.is_some_and(|c| c.get()) { s.current(Current::Page) } else { s }
    })
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let pad = |y: f32, x: f32| Insets { top: y, bottom: y, start: x, end: x };
    // button: 0.42rem 0.75rem, a hairline of the line color, 8 px corners, the page's background.
    s.rule(
        "p_button",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.bg)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(pad(6.72, 12.0)),
    );
    s.rule("p_button", None, States::HOVER, StylePatch::default().border(k.gold));
    s.rule("p_button", None, States::DISABLED, StylePatch::default().opacity(0.45).border(k.line));
    // button.quiet: no surface or border, muted, 0.4rem at the sides.
    s.rule(
        "p_button",
        Some("quiet"),
        States::NONE,
        StylePatch::default().background(remedy_ui::core::Color::TRANSPARENT).border(remedy_ui::core::Color::TRANSPARENT).foreground(k.muted).padding(pad(6.72, 6.4)),
    );
    s.rule("p_button", Some("quiet"), States::HOVER, StylePatch::default().foreground(k.text).border(k.line));
    // a: gold.
    s.rule("p_link", None, States::NONE, StylePatch::default().foreground(k.gold));
    // nav a: muted pills; text color and a 7% wash under the pointer; the current page on a
    // gold-tinted surface with a gold ring (inset 1px: drawn as a border inside the padding).
    let clear = remedy_ui::core::Color::TRANSPARENT;
    s.rule(
        "p_link",
        Some("nav"),
        States::NONE,
        StylePatch::default().foreground(k.muted).radius(999.0).padding(pad(5.12, 11.52)).border(clear).border_width(0.0),
    );
    s.rule("p_link", Some("nav"), States::HOVER, StylePatch::default().foreground(k.text).background(mix(k.text, 0.07, clear)));
    s.rule(
        "p_link",
        Some("nav"),
        States::SELECTED,
        StylePatch::default()
            .foreground(k.text)
            .background(mix(k.gold, 0.18, k.raised))
            .border(mix(k.gold, 0.55, k.line))
            .border_width(1.0)
            .padding(pad(4.12, 10.52)),
    );
    // .card .meta a: muted, 0.88rem, gold under the pointer.
    s.rule("p_link", Some("meta"), States::NONE, StylePatch::default().foreground(k.muted).text_size(14.08));
    s.rule("p_link", Some("meta"), States::HOVER, StylePatch::default().foreground(k.gold));
    // .brand: the heading face at 24 px in the text color, gold under the pointer.
    s.rule("p_link", Some("brand"), States::NONE, StylePatch::default().foreground(k.text).heading(true).text_size(24.0).line_height(remedy_ui::style::LineHeight::Relative(1.0)));
    s.rule("p_link", Some("brand"), States::HOVER, StylePatch::default().foreground(k.gold));
    // .update-pill: a gold ring on a gold-tinted background, 0.9em.
    s.rule(
        "p_link",
        Some("pill"),
        States::NONE,
        StylePatch::default()
            .foreground(k.gold)
            .border(k.gold)
            .border_width(1.0)
            .radius(999.0)
            .padding(pad(2.88, 11.52))
            .background(mix(k.gold, 0.12, k.bg))
            .text_size(14.4),
    );
    s.rule("p_link", Some("pill"), States::HOVER, StylePatch::default().background(mix(k.gold, 0.14, clear)));
}
