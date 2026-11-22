//! A show's seasons as tabs on a season page, so the next one is one press away (main.ts
//! `seasonTabs`; styles.css `.season-tabs`): "All seasons", then each season (specials
//! last), the one shown in gold, a ✓ on the ones watched. The row scrolls sideways and opens
//! with the current tab in the middle.

use std::cell::Cell;
use std::rc::Rc;

use remedy_ui::a11y::{Action, Current, Role, Semantics};
use remedy_ui::core::{Insets, Vec2};
use remedy_ui::layout::Align;
use remedy_ui::router::Route;
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::RichText;
use remedy_ui::{El, Ui};

use crate::pages::Page;
use crate::theme::{mix, Look};
use crate::widgets::shows::{season_name, season_watched, show_seasons, ShowView};
use crate::Ctx;

fn tab(ui: &Ui, ctx: &Ctx, path: &str, label: String, current: bool, done: bool) -> El {
    let href = Route::parse(&Page::Browse(path.to_owned()).path()).href();
    let route = Page::Browse(path.to_owned()).path();
    let r = ctx.router.clone();
    let mut el = ui
        .el("p_season_tab")
        .row()
        .align(Align::Center)
        .gap(5.6)
        .layout(|l| l.shrink = 0.0)
        .child(ui.el("p_season_tab_text").text(RichText::plain(label.clone())).nowrap());
    if done {
        el = el.child(ui.el("p_tab_done").variant(if current { "current" } else { "" }).text("✓").nowrap());
    }
    // The ✓ is read as ", watched" (the old page's `sr-only` text).
    let name = if done { format!("{label}, watched") } else { label };
    let el = el.on_click(move || r.push(&route)).semantics({
        let s = Semantics::new(Role::Link).label(name).url(href).action(Action::Click);
        if current { s.current(Current::Page) } else { s }
    });
    el.state(States::SELECTED, current);
    el
}

/// The tabs, or `None` off a season page or for a show with fewer than two seasons.
pub fn season_tabs(ui: &Ui, ctx: &Ctx, view: &ShowView, path: &str) -> Option<El> {
    let season = view.season?;
    let folders = show_seasons(&view.show.save.id);
    if folders.len() < 2 {
        return None;
    }
    let ordered = folders.iter().filter(|f| f.season > 0).chain(folders.iter().filter(|f| f.season == 0));
    let row = ui
        .el("p_season_tabs")
        .row()
        .gap(6.4)
        .scroll_x()
        .fill_width()
        .layout(|l| {
            l.shrink = 0.0;
            l.margin.bottom = 17.6;
        })
        .padding(Insets { top: 1.6, bottom: 8.8, start: 1.6, end: 1.6 })
        .semantics(Semantics::new(Role::Navigation).label("Seasons"));
    let mut row = row.child(tab(ui, ctx, &view.show.save.id, "All seasons".into(), false, false));
    let mut current_tab = None;
    for f in ordered {
        let current = f.path == path || f.season == season;
        let t = tab(ui, ctx, &f.path, season_name(f.season), current, season_watched(view.library.as_ref(), f.season));
        if current && current_tab.is_none() {
            current_tab = Some(t.clone());
        }
        row = row.child(t);
    }
    // Centers the current tab once both are laid out (main.ts `afterShowPaint`).
    if let Some(cur) = current_tab {
        let done = Rc::new(Cell::new(false));
        let r2 = row.clone();
        row = row.on_resize(move |size| {
            if done.get() {
                return;
            }
            let (Some(rb), Some(cb)) = (r2.bounds(), cur.bounds()) else { return };
            done.set(true);
            let left = cb.min.x - rb.min.x;
            let x = left - (size.width - (cb.max.x - cb.min.x)) / 2.0;
            r2.scroll_to_exact(Vec2::new(x.max(0.0), 0.0));
        });
    }
    Some(row)
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule(
        "p_season_tab",
        None,
        States::NONE,
        StylePatch::default()
            .foreground(k.muted)
            .background(k.well)
            .border(k.line)
            .border_width(1.0)
            .radius(999.0)
            .padding(Insets { top: 5.6, bottom: 5.6, start: 13.6, end: 13.6 }),
    );
    s.rule(
        "p_season_tab",
        None,
        States::HOVER,
        StylePatch::default().foreground(k.text).border(mix(k.gold, 0.5, k.line)),
    );
    s.rule(
        "p_season_tab",
        None,
        States::SELECTED,
        StylePatch::default().foreground(k.bg).background(k.gold).border(k.gold).weight(600.0),
    );
    s.rule(
        "p_season_tab",
        None,
        States::SELECTED | States::HOVER,
        StylePatch::default().foreground(k.bg).background(k.gold).border(k.gold),
    );
    s.rule("p_tab_done", None, States::NONE, StylePatch::default().foreground(k.gold).text_size(12.8));
    s.rule("p_tab_done", Some("current"), States::NONE, StylePatch::default().foreground(k.bg));
}
