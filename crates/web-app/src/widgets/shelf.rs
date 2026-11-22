//! A shelf: a titled row of cards that scrolls sideways and snaps to a card's start
//! (main.ts `shelf`, styles.css `.shelf`, `.scroller`), or a muted line when it is empty.

use remedy_ui::core::{Insets, Point, Rect};
use remedy_ui::layout::{Align, Length};
use remedy_ui::paint::{Affine, Brush, Item};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::{SnapAlign, SnapAxis, SnapStrictness};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::Look;

/// A section heading with the gold diamond before it (`.shelf h2`).
pub fn heading(ui: &Ui, title: &str) -> El {
    ui.el("p_shelf_head")
        .row()
        .align(Align::Center)
        .gap(8.8)
        .layout(|l| l.margin = Insets { top: 0.0, bottom: 12.0, start: 0.0, end: 0.0 })
        .child(diamond(ui))
        .child(w::heading(ui, 2, title.to_owned()))
}

/// `.shelf h2::before`: a 0.42rem gold square, 2 px corners, turned 45 degrees.
fn diamond(ui: &Ui) -> El {
    let side = 6.72;
    ui.el("p_diamond").width(side).height(side).layout(|l| l.shrink = 0.0).paint(move |cx| {
        let (w, h) = (cx.size.width, cx.size.height);
        let c = Point::new(w / 2.0, h / 2.0);
        let t = Affine::translate(remedy_ui::core::Vec2::new(-c.x, -c.y))
            .then(Affine::rotate(std::f32::consts::FRAC_PI_4))
            .then(Affine::translate(remedy_ui::core::Vec2::new(c.x, c.y)));
        cx.list.push(Item::PushTransform(t));
        cx.list.fill_rounded(Rect::new(0.0, 0.0, w, h), remedy_ui::core::CornerRadii::all(2.0), Brush::Solid(cx.style.foreground));
        cx.list.push(Item::PopTransform);
    })
}

/// The sideways row of `cards`, each `card_width` wide, as one focus group: arrow keys move
/// along it, Tab leaves it.
pub fn scroller(ui: &Ui, cards: Vec<El>, card_width: f32) -> El {
    let row = ui
        .el("p_scroller")
        .row()
        .gap(14.4)
        .align(Align::Stretch)
        .padding(Insets { top: 8.0, bottom: 18.4, start: 3.2, end: 3.2 });
    let row = cards.into_iter().fold(row, |row, c| {
        row.child(c.snap_align(SnapAlign::Start).layout(move |l| {
            l.width = Length::Px(card_width);
            l.shrink = 0.0;
        }))
    });
    let reduced = ui.reduced_motion();
    let s = ui
        .el("p_shelf_scroll")
        .scroll()
        .scroll_x()
        .fill_width()
        .layout(|l| {
            l.max_height = Length::Auto;
        })
        .focus_group()
        .child(row);
    // Reduced motion: no snapping (styles.css), so a scroll stays where it was put.
    if reduced {
        s
    } else {
        s.scroll_snap(SnapAxis::X, SnapStrictness::Proximity).scroll_padding(Insets { top: 0.0, bottom: 0.0, start: 2.4, end: 2.4 })
    }
}

/// A whole shelf: its heading, then the cards, or `empty` in muted text when there are none.
pub fn shelf(ui: &Ui, title: &str, cards: Vec<El>, empty: &str, card_width: f32) -> El {
    let body = if cards.is_empty() {
        w::text(ui, empty.to_owned()).variant("muted")
    } else {
        scroller(ui, cards, card_width)
    };
    ui.el("p_shelf").column().fill_width().child(heading(ui, title)).child(body)
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule("p_diamond", None, States::NONE, StylePatch::default().foreground(k.gold));
}
