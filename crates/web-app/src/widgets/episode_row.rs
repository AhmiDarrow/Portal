//! A season's episodes as rows (main.ts `episodeItem`; styles.css `.ep-row`): a 16:9 still
//! to play from, with Watched / Up next / Continue on it and how far watching got, then the
//! code, aired date, runtime and rating, the title, three lines of plot, and Play (or Resume),
//! Start over and Info. Also the 16:9 still with a letter fallback both pages use.

use std::rc::Rc;

use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets};
use remedy_ui::image::{image_ext, Fit, ImageOptions, Source};
use remedy_ui::layout::{Align, Display, Justify, Length, Position, Track};
use remedy_ui::motion::{CubicBezier, Easing, Motion, Target};
use remedy_ui::reactive::Signal;
use remedy_ui::style::{Shadow, States, Style, StylePatch, StyleSheet};
use remedy_ui::view::{RichText, TextStylePatch};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::{mix, Look};
use crate::widgets::art::{art_url, letter_tile};
use crate::widgets::card::bar;
use crate::widgets::glyphs::{glyph, Glyph};

/// A 16:9 still (`.art` in a still frame: `aspect-ratio: 16 / 9; object-fit: cover`), the
/// letter tile when there is none or it cannot load. `letter` is the tile's size: 2rem in an
/// episode row, 1.2rem on the details page.
pub fn still(ui: &Ui, art: Option<&str>, name: &str, letter: f32) -> El {
    let tile = {
        let (u, n) = (ui.clone(), name.to_owned());
        move || letter_tile(&u, &n).variant(if letter < 30.0 { "small" } else { "" })
    };
    let inner = match art_url(art) {
        None => tile(),
        Some(url) => image_ext(ui, Source::Url(url), "", Fit::Cover, ImageOptions::new().placeholder(ui.el("p_art_wait")).fallback(tile))
            .layout(|l| {
                l.width = Length::Percent(1.0);
                l.height = Length::Percent(1.0);
            }),
    };
    ui.el("p_art").column().fill_width().clip().layout(|l| {
        l.aspect_ratio = Some(16.0 / 9.0);
        l.shrink = 0.0;
    })
    .child(inner)
}

/// What one episode row shows and does.
pub struct EpisodeRow {
    pub code: String,
    pub name: String,
    pub art: Option<String>,
    pub facts: Vec<String>,
    pub plot: String,
    pub finished: bool,
    /// This is the episode the show is up to.
    pub next: bool,
    /// Where watching stopped, when it can resume from there.
    pub resume: Option<f64>,
    /// How far watching got, for the bar (not once finished).
    pub fraction: Option<f64>,
    /// The words on the play buttons: "Resume 12:34", "Play again" or "Play".
    pub go_label: String,
    /// Plays (from `resume` when there is one).
    pub go: Rc<dyn Fn()>,
    /// Plays from the start.
    pub start_over: Rc<dyn Fn()>,
    /// Opens Info; gets the button.
    pub info: Rc<dyn Fn(&El)>,
}

fn flag(ui: &Ui, text: &str, next: bool) -> El {
    ui.el("p_ep_flag")
        .variant(if next { "next" } else { "" })
        .text(text.to_owned())
        .nowrap()
        .semantics(Semantics::new(Role::Label).hidden(true))
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: 7.2, bottom: remedy_ui::layout::AUTO, start: 7.2, end: remedy_ui::layout::AUTO };
        })
}

fn ease() -> Easing {
    Easing::Bezier(CubicBezier { x1: 0.25, y1: 0.1, x2: 0.25, y2: 1.0 })
}

/// One row (`phone`: the still above the words, at 720 px and under).
pub fn episode_row(ui: &Ui, r: EpisodeRow, phone: bool) -> El {
    let fine = {
        let caps = ui.pointer_caps();
        move || caps.with(|c| c.hover && c.fine)
    };
    let reduced = ui.reduced_motion();
    let picture = still(ui, r.art.as_deref(), &r.name, 32.0);
    let picture = if r.finished { picture.style(Style::new().saturate(0.55).brightness(0.8)) } else { picture };
    let lit = Signal::new(false);
    // .ep-play: a round play mark, shown on the episode the show is up to and under the
    // pointer or keyboard focus, gold then.
    let mark = ui
        .el("p_ep_play")
        .row()
        .align(Align::Center)
        .justify(Justify::Center)
        .width(46.4)
        .height(46.4)
        .child(glyph(ui, Glyph::Play, 20.8).layout(|l| l.margin.start = 2.4))
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: remedy_ui::layout::AUTO, bottom: remedy_ui::layout::AUTO, start: remedy_ui::layout::AUTO, end: remedy_ui::layout::AUTO };
        });
    let next = r.next;
    let m2 = mark.clone();
    let mark = mark.bind(move || {
        let on = lit.get();
        m2.state(States::HOVER, on);
        let shown = on || next;
        let d = if reduced { std::time::Duration::ZERO } else { std::time::Duration::from_millis(150) };
        m2.animate(Target::Opacity(if shown { 1.0 } else { 0.0 }), Motion::Tween { duration: d, easing: ease() });
        m2.animate(Target::Scale(if shown { 1.0 } else { 0.9 }), Motion::Tween { duration: d, easing: ease() });
    });
    let mut thumb = ui
        .el("p_ep_thumb")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        .clip()
        .fill_width()
        .layout(|l| {
            l.position = Position::Relative;
            l.shrink = 0.0;
        })
        .child(picture.clone())
        .child(mark);
    if r.finished {
        thumb = thumb.child(flag(ui, "Watched", false));
    } else if r.next {
        thumb = thumb.child(flag(ui, if r.resume.is_some() { "Continue" } else { "Up next" }, true));
    }
    if let Some(f) = r.fraction {
        thumb = thumb.child(bar(ui, f).variant("still").height(4.0).layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: remedy_ui::layout::AUTO, bottom: 0.0, start: 0.0, end: 0.0 };
        }));
    }
    let what: Vec<&str> = [r.code.as_str(), r.name.as_str()].into_iter().filter(|s| !s.is_empty()).collect();
    let thumb_label = format!("{}: {}", r.go_label, what.join(" · "));
    let go = r.go.clone();
    let p2 = picture.clone();
    let fine2 = fine.clone();
    let thumb = thumb
        .on_click(move || go())
        .on_hover(move |on| {
            lit.set(on);
            if fine2() && !reduced {
                p2.animate(Target::Scale(if on { 1.04 } else { 1.0 }), Motion::Tween { duration: std::time::Duration::from_millis(350), easing: ease() });
            }
        })
        .semantics(Semantics::new(Role::Button).label(thumb_label).action(Action::Click));
    let t2 = thumb.clone();
    let thumb = thumb.on_focus_change(move |on| lit.set(on && t2.states().contains(States::FOCUS_VISIBLE)));

    let mut facts_patch = TextStylePatch::default();
    facts_patch.features = vec![remedy_ui::text::Feature { tag: *b"tnum", value: 1 }];
    let mut body = ui
        .el("p_ep_body")
        .column()
        .gap(4.8)
        .layout(|l| {
            l.min_width = Length::Px(0.0);
            l.shrink = 1.0;
        })
        .child(ui.el("p_ep_facts").text(RichText::new().styled(&r.facts.join(" · "), facts_patch)))
        .child(w::heading(ui, 3, r.name.clone()).variant("ep"));
    if !r.plot.is_empty() {
        body = body.child(ui.el("p_ep_plot").text(RichText::plain(r.plot.clone())).max_lines(3));
    }
    let go = r.go.clone();
    let mut actions = ui
        .el("p_ep_actions")
        .row()
        .gap(6.4)
        .layout(|l| {
            l.wrap = true;
            l.margin.top = 5.6;
        })
        .child(
            ui.el("p_pill")
                .variant("go")
                .row()
                .align(Align::Center)
                .gap(6.4)
                .child(glyph(ui, Glyph::Play, 17.6))
                .child(ui.el("p_pill_text").text(r.go_label.clone()).nowrap())
                .on_click(move || go())
                .semantics(Semantics::new(Role::Button).label(r.go_label.clone()).action(Action::Click)),
        );
    if r.resume.is_some() {
        let s = r.start_over.clone();
        actions = actions.child(
            ui.el("p_pill")
                .variant("text")
                .row()
                .align(Align::Center)
                .child(ui.el("p_pill_text").text("Start over").nowrap())
                .on_click(move || s())
                .semantics(Semantics::new(Role::Button).label("Start over").action(Action::Click)),
        );
    }
    let info = r.info.clone();
    actions = actions.child(crate::widgets::saved::info_button(ui, &r.name, "pill", move |el| info(el)));
    let body = body.child(actions);

    let variant = match (r.next, r.finished) {
        (true, _) => "next",
        (false, true) => "watched",
        _ => "",
    };
    ui.el("p_ep_row")
        .variant(variant)
        .layout(move |l| {
            l.display = Display::Grid;
            l.columns = if phone {
                vec![Track::Fr(1.0)]
            } else {
                vec![Track::MinMax(Box::new(Track::Px(160.0)), Box::new(Track::Px(272.0))), Track::Fr(1.0)]
            };
            l.gap = if phone { (9.6, 9.6) } else { (16.0, 16.0) };
            l.align_items = Align::Start;
            l.shrink = 0.0;
        })
        .padding(Insets::all(11.2))
        .semantics(Semantics::new(Role::Article))
        .child(thumb)
        .child(body)
}

/// A row for an item of the season that is not an episode (`.ep-other .source-row`).
pub fn other_row(ui: &Ui, label: &str, on_press: impl Fn() + 'static) -> El {
    ui.el("p_source_row")
        .row()
        .align(Align::Center)
        .fill_width()
        .layout(|l| {
            l.margin = Insets { top: 1.6, bottom: 1.6, start: 0.0, end: 0.0 };
            l.shrink = 0.0;
        })
        .child(ui.el("p_source_row_text").text(RichText::plain(label.to_owned())).grow(1.0))
        .child(ui.el("p_source_row_chevron").text("›"))
        .on_click(on_press)
        .semantics(Semantics::new(Role::Button).label(label.to_owned()).action(Action::Click))
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let none = States::NONE;
    let clear = Color::TRANSPARENT;
    let row = StylePatch::default().background(k.raised).border(k.line).border_width(1.0).radius(14.0);
    s.rule("p_ep_row", None, none, row);
    let lit = mix(k.gold, 0.48, k.line);
    s.rule("p_ep_row", None, States::HOVER, StylePatch::default().border(lit));
    s.rule("p_ep_row", None, States::FOCUS_WITHIN, StylePatch::default().border(lit));
    let ring = Shadow::new(0.0, 0.0, 0.0, mix(k.gold, 0.35, clear)).spread(1.0);
    for st in [none, States::HOVER, States::FOCUS_WITHIN] {
        s.rule("p_ep_row", Some("next"), st, StylePatch::default().border(k.gold).shadow(ring));
    }
    s.rule("p_ep_thumb", None, none, StylePatch::default().background(k.well).radius(8.0).outline_offset(2.0));
    s.rule(
        "p_ep_play",
        None,
        none,
        StylePatch::default()
            .background(mix(k.bg, 0.70, clear))
            .border(mix(k.text, 0.35, clear))
            .border_width(1.0)
            .radius(999.0)
            .foreground(k.text),
    );
    s.rule("p_ep_play", None, States::HOVER, StylePatch::default().background(k.gold).border(k.gold).foreground(k.bg));
    s.rule(
        "p_ep_flag",
        None,
        none,
        StylePatch::default()
            .background(mix(k.bg, 0.78, clear))
            .foreground(k.text)
            .radius(999.0)
            .text_size(12.0)
            .weight(600.0)
            .letter_spacing(remedy_ui::style::LetterSpacing::Em(0.02))
            .padding(Insets { top: 1.92, bottom: 1.92, start: 8.0, end: 8.0 }),
    );
    s.rule("p_ep_flag", Some("next"), none, StylePatch::default().background(k.gold).foreground(k.bg));
    s.rule("p_bar", Some("still"), none, StylePatch::default().background(mix(k.bg, 0.60, clear)).radius(0.0));
    s.rule(
        "p_ep_facts",
        None,
        none,
        StylePatch::default().foreground(k.muted).text_size(13.6).letter_spacing(remedy_ui::style::LetterSpacing::Em(0.02)),
    );
    s.rule("heading", Some("ep"), none, StylePatch::default().heading(true).weight(400.0).text_size(19.2).foreground(k.text).line_height(remedy_ui::style::LineHeight::Relative(1.25)));
    s.rule("p_ep_plot", None, none, StylePatch::default().foreground(mix(k.text, 0.82, k.muted)).text_size(14.88).line_height(remedy_ui::style::LineHeight::Relative(1.45)));
    // .ep-go: the gold Play pill; solid gold under the pointer.
    let go_border = mix(k.gold, 0.45, k.line);
    s.rule("p_pill", Some("go"), none, StylePatch::default().foreground(k.gold).border(go_border));
    for st in [States::HOVER, States::FOCUS_VISIBLE] {
        s.rule("p_pill", Some("go"), st, StylePatch::default().background(k.gold).foreground(k.bg).border(k.gold));
    }
    // A pill with words only: `.pill-btn` padding with no glyph.
    s.rule(
        "p_source_row",
        None,
        none,
        StylePatch::default()
            .background(k.raised)
            .foreground(k.text)
            .border(k.line)
            .border_width(1.0)
            .radius(8.0)
            .padding(Insets { top: 11.2, bottom: 11.2, start: 13.6, end: 14.4 }),
    );
    for st in [States::HOVER, States::FOCUS_VISIBLE] {
        s.rule("p_source_row", None, st, StylePatch::default().border(k.gold).background(mix(k.gold, 0.08, k.raised)));
    }
    s.rule("p_source_row_chevron", None, none, StylePatch::default().foreground(k.muted).weight(700.0));
    s.rule("p_art_letter_text", Some("small"), none, StylePatch::default().text_size(19.2));
}
