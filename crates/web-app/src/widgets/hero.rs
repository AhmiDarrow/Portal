//! A show's hero on its page and its season pages (main.ts `showHero`, `heroPlay`;
//! styles.css `.show-hero`): the poster and the show over its backdrop, behind two scrims
//! of the raised surface, with the breadcrumbs, the facts and the actions.

use remedy_ui::a11y::{Action, Current, Role, Semantics};
use remedy_ui::core::Insets;
use remedy_ui::image::{image_ext, Fit, ImageOptions, ImageState, Source};
use remedy_ui::layout::{Align, Length, Position};
use remedy_ui::style::{Shadow, States, StylePatch, StyleSheet};
use remedy_ui::view::RichText;
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::{gradient, mix, Look};
use crate::widgets::art;
use crate::widgets::glyphs::{glyph, Glyph};

/// One breadcrumb: a link to a route, or the page shown (`aria-current`), or plain text.
pub enum Crumb {
    Link { path: String, label: String },
    Here(String),
    Text(String),
}

/// What the hero shows.
pub struct Hero {
    pub name: String,
    pub poster: Option<String>,
    /// The backdrop's address, already through `art_url`.
    pub fanart: Option<String>,
    pub crumbs: Vec<Crumb>,
    pub facts: Vec<String>,
    /// The actions row: the play pill first, then the toggles.
    pub actions: Vec<El>,
}

/// `.hero-play`: the gold pill that resumes or plays the episode the show is up to.
pub fn play_pill(ui: &Ui, label: &str, on_press: impl Fn() + 'static) -> El {
    ui.el("p_hero_play")
        .row()
        .align(Align::Center)
        .gap(7.2)
        .child(glyph(ui, Glyph::Play, 17.6))
        .child(ui.el("p_pill_text").text(label.to_owned()).nowrap())
        .on_click(on_press)
        .semantics(Semantics::new(Role::Button).label(label.to_owned()).action(Action::Click))
}

fn crumb(ui: &Ui, ctx: &crate::Ctx, c: &Crumb) -> El {
    match c {
        Crumb::Link { path, label } => crate::widgets::link(ui, &ctx.router, path, label.clone(), "crumb", None),
        Crumb::Here(t) => ui
            .el("p_crumb_here")
            .text(RichText::plain(t.clone()))
            .semantics(Semantics::new(Role::Label).label(t.clone()).current(Current::Page)),
        Crumb::Text(t) => ui.el("p_crumb_text").text(RichText::plain(t.clone())),
    }
}

/// The hero, laid out for a phone (720 px and under) when `phone`.
pub fn hero(ui: &Ui, ctx: &crate::Ctx, h: Hero, phone: bool) -> El {
    let mut nav = ui
        .el("p_crumbs")
        .row()
        .align(Align::Center)
        .layout(|l| {
            l.wrap = true;
            l.gap = (7.2, 2.4);
        })
        .semantics(Semantics::new(Role::Navigation).label("Breadcrumb"));
    for (i, c) in h.crumbs.iter().enumerate() {
        if i > 0 {
            nav = nav.child(ui.el("p_crumb_sep").text("›").semantics(Semantics::new(Role::Label).hidden(true)));
        }
        nav = nav.child(crumb(ui, ctx, c));
    }
    let mut text = ui
        .el("p_show_hero_text")
        .column()
        .grow(1.0)
        .layout(|l| {
            l.shrink = 1.0;
            l.min_width = Length::Px(0.0);
        })
        .child(nav)
        .child(w::heading(ui, 1, h.name.clone()).layout(|l| {
            l.margin = Insets { top: 2.4, bottom: 4.8, start: 0.0, end: 0.0 };
        }));
    if !h.facts.is_empty() {
        text = text.child(ui.el("p_show_facts").text(h.facts.join(" · ")).layout(|l| l.margin.bottom = 13.6));
    }
    let mut actions = ui.el("p_show_actions").row().align(Align::Center).gap(if phone { 6.4 } else { 7.2 }).layout(move |l| {
        l.wrap = true;
        if phone {
            l.width = Length::Percent(1.0);
        }
    });
    for a in h.actions {
        // On a phone every action shares the row, centered (`flex: 1 1 auto`).
        let a = if phone {
            a.variant("phone").justify(remedy_ui::layout::Justify::Center).layout(|l| {
                l.grow = 1.0;
                l.shrink = 1.0;
            })
        } else {
            a
        };
        actions = actions.child(a);
    }
    let text = text.child(actions);

    let poster = ui
        .el("p_show_poster")
        .clip()
        .width(if phone { 104.0 } else { 152.0 })
        .layout(|l| l.shrink = 0.0)
        .child(art::art(ui, h.poster.as_deref(), &h.name, ""));
    let mut root = ui
        .el("p_show_hero")
        .variant(if phone { "phone" } else { "" })
        .clip()
        .fill_width()
        .gap(if phone { 20.8 } else { 20.8 })
        .layout(move |l| {
            l.position = Position::Relative;
            l.margin.bottom = 17.6;
            l.shrink = 0.0;
        });
    root = if phone { root.column().align(Align::Start) } else { root.row().align(Align::End) };
    if let Some(url) = h.fanart {
        let backdrop = ui.el("p_show_backdrop").layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
        });
        let b2 = backdrop.clone();
        // ENGINE GAP: `object-position: center 25%`; RemedyUI's Fit::Cover always centers
        // (remedy-ui-media-image/src/lib.rs `place`).
        let img = image_ext(
            ui,
            Source::Url(url),
            "",
            Fit::Cover,
            ImageOptions::new().placeholder(ui.el("p_show_wait")).on_state(move |s| {
                if let ImageState::Failed(_) = s {
                    let _ = b2.clone().hidden(true);
                }
            }),
        )
        .layout(|l| {
            l.width = Length::Percent(1.0);
            l.height = Length::Percent(1.0);
        });
        let fill = |el: El| {
            el.layout(|l| {
                l.position = Position::Absolute;
                l.inset = Insets::ZERO;
            })
        };
        root = root.child(backdrop.child(fill(ui.el("p_show_backdrop_img").child(img))).child(fill(ui.el("p_show_scrim"))));
    }
    root.child(poster).child(text)
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let none = States::NONE;
    s.rule(
        "p_show_hero",
        None,
        none,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .shadow(k.shadow)
            .padding(Insets::all(18.4)),
    );
    s.rule("p_show_hero", Some("phone"), none, StylePatch::default().padding(Insets::all(15.2)));
    s.rule("p_show_backdrop_img", None, none, StylePatch::default().opacity(0.42));
    s.rule(
        "p_show_scrim",
        None,
        none,
        StylePatch::default().background_layers(gradient(
            "linear-gradient(90deg, var(--raised) 0%, color-mix(in srgb, var(--raised) 82%, transparent) 42%, color-mix(in srgb, var(--raised) 35%, transparent) 100%), \
             linear-gradient(0deg, var(--raised) 0%, transparent 55%)",
        )),
    );
    s.rule(
        "p_show_poster",
        None,
        none,
        StylePatch::default().border(k.line).border_width(1.0).radius(8.0).shadow(k.shadow),
    );
    s.rule("p_show_facts", None, none, StylePatch::default().foreground(k.muted));
    let small = StylePatch::default().text_size(14.08);
    s.rule("p_crumbs", None, none, small.clone().foreground(k.muted));
    s.rule("p_link", Some("crumb"), none, small.clone().foreground(k.muted));
    s.rule("p_link", Some("crumb"), States::HOVER, StylePatch::default().foreground(k.gold));
    s.rule("p_crumb_here", None, none, small.clone().foreground(k.text));
    s.rule("p_crumb_text", None, none, small.clone().foreground(k.muted));
    s.rule("p_crumb_sep", None, none, small.foreground(k.muted).opacity(0.6));
    // .hero-play: 0.45rem 1.05rem 0.45rem 0.8rem, gold with the background's color on it;
    // 84% gold into white under the pointer.
    s.rule(
        "p_hero_play",
        None,
        none,
        StylePatch::default()
            .background(k.gold)
            .border(k.gold)
            .border_width(1.0)
            .foreground(k.bg)
            .weight(600.0)
            .radius(999.0)
            .padding(Insets { top: 7.2, bottom: 7.2, start: 12.8, end: 16.8 }),
    );
    let lift = mix(k.gold, 0.84, remedy_ui::core::Color::WHITE);
    s.rule("p_hero_play", None, States::HOVER, StylePatch::default().background(lift));
    s.rule("p_hero_play", None, States::FOCUS_VISIBLE, StylePatch::default().background(lift));
    // A phone: `.show-hero-actions .pill-btn, .hero-play { padding: 0.4rem 0.6rem }`.
    let phone_pad = Insets { top: 6.4, bottom: 6.4, start: 9.6, end: 9.6 };
    s.rule("p_hero_play", Some("phone"), none, StylePatch::default().padding(phone_pad));
    s.rule("p_pill", Some("phone"), none, StylePatch::default().padding(phone_pad));
    let _ = Shadow::new(0.0, 0.0, 0.0, k.line);
}
