//! The window around every page (index.html, main.ts:172-335, 3532, 3782-3810): the skip
//! link; the sticky frosted header with the brand, the nav (the page shown marked current),
//! the Theme menu, the update pill, the Kodi status chip and the activity panel; the notice
//! line; the routed page in `main`; the player dock; and the pairing screen when the server
//! asks this device to pair.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use portal_api::{EngineState, UpdateInfo, UpdateState};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets, Point, Vec2};
use remedy_ui::input::{Key, PointerPhase};
use remedy_ui::layout::{Align, Length, Position, AUTO};
use remedy_ui::motion::{Motion, Target};
use remedy_ui::paint::Brush;
use remedy_ui::reactive::{Cancel, Signal};
use remedy_ui::router::Router;
use remedy_ui::style::{StickyInsets, States, StylePatch, StyleSheet, TextTransform};
use remedy_ui::view::Style;
use remedy_ui::widgets as w;
use remedy_ui::{show, App, El, Ui, WindowSpec};

use crate::fonts::Fonts;
use crate::pages::{Nav, Page};
use crate::theme::{gradient, mix, Look};
use crate::widgets::activity::{self, Activity};
use crate::widgets::card::is_narrow;
use crate::{api, screens, theme, Ctx};

/// Where the image loader retries art from sites that send no CORS headers.
pub const IMAGE_PROXY: &str = "/api/image/web?url=";
/// How often the status chip asks (main.ts: every 4 s while the page is seen).
pub const CHIP_POLL: Duration = Duration::from_secs(4);
/// How often the update pill asks (main.ts `UPDATE_POLL_MS`).
pub const UPDATE_POLL: Duration = Duration::from_secs(30 * 60);
/// While an update downloads or installs, how often it is followed, and for how long.
pub const UPDATE_WATCH: Duration = Duration::from_millis(1500);
pub const UPDATE_WATCH_LIMIT: Duration = Duration::from_secs(15 * 60);

/// Opens Portal's window.
pub fn app(app: &App) {
    remedy_ui::image::set_web_proxy(Some(IMAGE_PROXY.into()));
    let prefers_dark = Signal::new(None::<bool>);
    app.on_appearance(move |dark, _| prefers_dark.set(dark));
    let a = app.clone();
    app.open(WindowSpec::new("Portal"), move |ui, window| {
        let ctx = Ctx {
            app: a.clone(),
            api: api::Api::new(&a),
            router: Router::for_window(window, "/"),
            // ENGINE GAP: read from the old page's keys, but a change here is not saved yet:
            // the engine cannot write a page's own localStorage item (see stored.rs).
            theme: Signal::new(theme::saved().to_owned()),
            fonts: Signal::new(Fonts::saved()),
            me: Signal::new(None),
            activity: Activity::new(ui),
            width: Signal::new(ui.window_size().width),
            update: Signal::new(None),
        };
        ctx.router.set_title(|r| Page::from_route(r).title());
        ask_pairing(&ctx, 0);
        let width = ctx.width;
        let root = ui.root().child(page(ui, &ctx)).on_resize(move |s| {
            if width.get_untracked().ok() != Some(s.width) {
                width.set(s.width);
            }
        });
        let (app, t, f) = (a.clone(), ctx.theme, ctx.fonts);
        let _ = root.bind(move || {
            let mut look = theme::resolve(&t.get(), prefers_dark.get());
            f.get().apply(&mut look);
            app.set_theme(look);
        });
    });
}

/// `GET /api/pair/me`, asked up to five times a second longer apart each, as a Portal that
/// is still starting may not answer yet; then a device that only watches (main.ts:3765).
fn ask_pairing(ctx: &Ctx, attempt: u32) {
    let c = ctx.clone();
    ctx.api.run(api::pair_me(), &Cancel::new(), move |out| match out {
        Ok(me) => c.me.set(Some(me)),
        Err(_) if attempt + 1 < 5 => {
            let c2 = c.clone();
            let ui = c.activity_ui();
            ui.after(Duration::from_secs(u64::from(attempt) + 1), move || ask_pairing(&c2, attempt + 1));
        }
        Err(_) => c.me.set(Some(portal_api::PairMe { paired: true, this_pc: false })),
    });
}

impl Ctx {
    /// The window's UI (for timers), from the activity panel that lives in it.
    fn activity_ui(&self) -> Ui {
        self.activity.ui()
    }

    /// True on a phone, tablet or TV: anything but the PC running Portal (main.ts `isRemote`).
    pub fn is_remote(&self) -> bool {
        self.me.get().is_some_and(|m| !m.this_pc)
    }
}

/// The page: header and routed content, or the pairing screen while pairing is needed.
fn page(ui: &Ui, ctx: &Ctx) -> El {
    let pair = ctx.api.pair_needed();
    let me = ctx.me;
    let (u1, u2, c1, c2) = (ui.clone(), ui.clone(), ctx.clone(), ctx.clone());
    ui.el("portal_page")
        .column()
        .grow(1.0)
        .fill_width()
        // Exactly the window's height, so the page's own scroll is what scrolls (and can be
        // scrolled from code); no wrapper may grow with its content.
        .layout(|l| {
            l.height = Length::Percent(1.0);
            l.min_height = Length::Px(0.0);
        })
        .child(show(ui, move || pair.get() || me.with(|m| m.as_ref().is_some_and(|m| !m.paired)), move || screens::pairing::view(&u1, &c1)).el().fill_width())
        .child(show(ui, move || !pair.get() && me.with(|m| m.as_ref().is_some_and(|m| m.paired)), move || routed(&u2, &c2)).el().grow(1.0).fill_width().layout(|l| l.min_height = Length::Px(0.0)))
}

fn routed(ui: &Ui, ctx: &Ctx) -> El {
    let chip: Signal<Chip> = Signal::new(("", "…".to_owned(), String::new()));
    start_polls(ui, ctx, chip);
    let (u, c) = (ui.clone(), ctx.clone());
    let content = ctx.router.view(ui, move |route| {
        // main.ts `render`: a new page hides the last one's activity and notice.
        c.activity.end_page();
        c.activity.notice("");
        screens::page(&u, &c, &Page::from_route(route))
    });
    let narrow = {
        let wd = ctx.width;
        move || is_narrow(wd.get())
    };
    let n2 = narrow.clone();
    let main = ui
        .el("p_main")
        .column()
        .fill_width()
        .focusable(true)
        .semantics(Semantics::new(Role::Main))
        .layout(|l| {
            l.max_width = Length::Px(1240.0);
            l.align_self = Some(Align::Center);
        })
        .child(content);
    let m2 = main.clone();
    let main = main.bind(move || {
        let pad = if n2() { Insets::all(12.8) } else { Insets { top: 22.4, bottom: 38.4, start: 21.6, end: 21.6 } };
        let _ = m2.clone().padding(pad);
    });
    let n3 = narrow.clone();
    let note = activity::notice(ui, &ctx.activity).layout(|l| {
        l.max_width = Length::Px(1240.0);
        l.align_self = Some(Align::Center);
    });
    let n4 = note.clone();
    let note = note.bind(move || {
        let side = if n3() { 12.8 } else { 0.0 };
        let _ = n4.clone().layout(move |l| {
            l.width = Length::Percent(1.0);
            l.margin = Insets { top: 14.4, bottom: 0.0, start: side, end: side };
        });
    });
    let scroll = w::scroll(ui)
        .fill_width()
        .child(skip_link(ui, &main))
        .child(header(ui, ctx, chip))
        .child(note)
        .child(main)
        // body { padding-bottom: 2rem }
        .child(w::column(ui).height(32.0).layout(|l| l.shrink = 0.0));
    w::column(ui)
        .grow(1.0)
        .fill_width()
        .layout(|l| l.min_height = Length::Px(0.0))
        .child(scroll)
        .child(screens::player::view(ui, ctx))
}

/// "Skip to content": off the top until it has focus; it moves focus to `main`.
fn skip_link(ui: &Ui, main: &El) -> El {
    let m = main.clone();
    let el = ui
        .el("p_skip")
        .text("Skip to content")
        .nowrap()
        .on_click(move || m.focus())
        .semantics(Semantics::new(Role::Link).label("Skip to content").action(Action::Click))
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets { top: 11.2, start: 11.2, end: AUTO, bottom: AUTO };
        });
    let e2 = el.clone();
    let el = el.on_focus_change(move |on| {
        let h = e2.bounds().map_or(40.0, |b| b.height());
        // transform: translateY(-180%) until focused.
        e2.animate(Target::Translate(Vec2::new(0.0, if on { 0.0 } else { -1.8 * h - 11.2 })), Motion::Instant);
    });
    el.animate(Target::Translate(Vec2::new(0.0, -80.0)), Motion::Instant);
    el
}

/// The header (`header.top`): sticky at the top, frosted, with its hairline under it.
fn header(ui: &Ui, ctx: &Ctx, chip: Signal<Chip>) -> El {
    let brand = crate::widgets::link(ui, &ctx.router, "/", "Portal", "brand", None);
    let brand = ui
        .el("p_brand")
        .row()
        .align(Align::Center)
        .gap(8.8)
        .child(ui.el("p_brand_bar").width(3.0).height(18.4).layout(|l| l.shrink = 0.0))
        .child(brand);
    let tools = ui
        .el("p_tools")
        .row()
        .align(Align::Center)
        .gap(9.6)
        .layout(|l| l.margin = Insets { top: 0.0, bottom: 0.0, start: AUTO, end: 0.0 })
        .child(theme_picker(ui, ctx))
        .child(update_pill(ui, ctx))
        .child(status_chip(ui, chip));
    // Wide: brand, nav, tools. Under 720 px brand and tools share the first row and the nav
    // takes the second, scrolling sideways. One nav for each place, the other hidden.
    let wide_nav = nav(ui, ctx);
    let narrow_nav = nav(ui, ctx)
        .layout(|l| {
            l.wrap = false;
            l.width = Length::Percent(1.0);
        })
        // .top nav { padding-bottom: 0.15rem }
        .padding(Insets { top: 0.0, bottom: 2.4, start: 0.0, end: 0.0 });
    let panel = activity::panel(ui, &ctx.activity)
        .layout(|l| l.margin = Insets { top: 4.0, bottom: 0.0, start: 0.0, end: 0.0 })
        .layout(|l| l.width = Length::Percent(1.0));
    let top = ui
        .el("p_top")
        .row()
        .align(Align::Center)
        .fill_width()
        .layout(|l| l.wrap = true)
        .child(brand)
        .child(wide_nav.clone().layout(|l| l.wrap = true))
        .child(tools)
        .child(narrow_nav.clone())
        .child(panel);
    let (t2, wd) = (top.clone(), ctx.width);
    let top = top.bind(move || {
        let narrow = is_narrow(wd.get());
        let (gap, pad) = if narrow {
            ((12.8, 8.0), Insets { top: 11.2, bottom: 11.2, start: 11.2, end: 11.2 })
        } else {
            ((17.6, 12.0), Insets { top: 12.0, bottom: 12.0, start: 20.0, end: 20.0 })
        };
        let _ = t2.clone().layout(move |l| l.gap = gap).padding(pad);
        let _ = wide_nav.clone().hidden(narrow);
        let _ = narrow_nav.clone().hidden(!narrow);
    });
    ui.el("p_header")
        .column()
        .fill_width()
        .sticky(StickyInsets::top(0.0))
        .semantics(Semantics::new(Role::Banner))
        .child(top)
        .child(ui.el("p_header_rule").fill_width().height(1.0).layout(|l| l.shrink = 0.0))
}

/// The nav, the page shown marked current; no Source on a remote device.
fn nav(ui: &Ui, ctx: &Ctx) -> El {
    let shown = Signal::new(None::<Nav>);
    let router = ctx.router.clone();
    let row = ui
        .el("p_nav")
        .row()
        .gap(3.2)
        .semantics(Semantics::new(Role::Navigation).label("Main"))
        .bind(move || shown.set(Page::from_route(&router.route()).nav()));
    let (r2, u, c) = (row.clone(), ui.clone(), ctx.clone());
    let row = row.bind(move || {
        let remote = c.is_remote();
        remedy_ui::reactive::untrack(|| {
            r2.clear();
            for item in Nav::ALL {
                if remote && item == Nav::Source {
                    continue;
                }
                let current = Signal::new(false);
                let link = crate::widgets::link(&u, &c.router, &item.page().path(), item.label(), "nav", Some(current))
                    .bind(move || current.set(shown.get() == Some(item)));
                let _ = r2.clone().child(link);
            }
        });
    });
    // Under 720 px the row scrolls sideways (no bar), so all of it stays reachable.
    row.scroll().scroll_x().layout(|l| {
        l.max_height = Length::Auto;
        l.shrink = 1.0;
        l.min_width = Length::Px(0.0);
    })
}

/// The Theme button and its list of themes with swatches (themes.ts `mountThemes`).
fn theme_picker(ui: &Ui, ctx: &Ctx) -> El {
    let chosen = ctx.theme;
    let open: Rc<Cell<Option<El>>> = Rc::default();
    let expanded = Signal::new(false);
    let name = move || theme::MENU.iter().find(|c| c.id == chosen.get()).map_or("Portal", |c| c.name);
    let button = crate::widgets::button(ui, "Theme", || {}).layout(|l| l.max_width = Length::Px(192.0));
    let (b2, u, o2) = (button.clone(), ui.clone(), open.clone());
    let toggle = Rc::new(move || {
        if let Some(m) = o2.take() {
            m.remove();
            expanded.set(false);
            return;
        }
        let menu = theme_menu(&u, &b2, chosen, o2.clone(), expanded);
        o2.set(Some(menu));
        expanded.set(true);
    });
    let t2 = toggle.clone();
    button.on_click(move || t2()).semantics(move || {
        Semantics::new(Role::Button).label(format!("Theme, {}", name())).expanded(expanded.get()).action(Action::Click)
    })
}

fn theme_menu(ui: &Ui, button: &El, chosen: Signal<String>, slot: Rc<Cell<Option<El>>>, expanded: Signal<bool>) -> El {
    let at = button.bounds().unwrap_or_default();
    let window = ui.window_size();
    let width = 264.0;
    let x = (at.max.x - width).clamp(4.0, (window.width - width - 4.0).max(4.0));
    let y = at.max.y + 6.4;
    let focus = Signal::new(theme::MENU.iter().position(|c| c.id == chosen.get_untracked().unwrap_or_default()).unwrap_or(0));
    let (s1, b1) = (slot.clone(), button.clone());
    let close = Rc::new(move |refocus: bool| {
        if let Some(m) = s1.take() {
            m.remove();
        }
        expanded.set(false);
        if refocus {
            b1.focus();
        }
    });
    let mut list = ui
        .el("p_theme_menu")
        .column()
        .layout(move |l| {
            l.width = Length::Px(width);
            l.max_height = Length::Px((window.height * 0.7).min(448.0));
        })
        .scroll()
        .semantics(Semantics::new(Role::Menu).label("Themes"));
    let mut options = Vec::new();
    for (i, c) in theme::MENU.iter().enumerate() {
        let id = c.id.to_owned();
        let label = if c.id == "system" { format!("{} · OS", c.name) } else { c.name.to_owned() };
        let swatch_layers = if c.swatch.starts_with("linear-gradient") { gradient(c.swatch) } else { Vec::new() };
        let fill = Color::from_hex(c.swatch).unwrap_or(Color::TRANSPARENT);
        let swatch = ui
            .el("p_swatch")
            .width(13.6)
            .height(13.6)
            .layout(|l| l.shrink = 0.0)
            .style(Style::new().background(fill).background_layers(swatch_layers));
        let cl = close.clone();
        let id2 = id.clone();
        let l2 = label.clone();
        let opt = ui
            .el("p_theme_option")
            .row()
            .align(Align::Center)
            .gap(8.0)
            .fill_width()
            .child(swatch)
            .child(ui.el("p_theme_option_text").text(label))
            .on_click(move || {
                chosen.set(id2.clone());
                cl(true);
            })
            .on_hover(move |on| {
                if on && focus.get_untracked().ok() != Some(i) {
                    focus.set(i);
                }
            })
            .semantics(move || {
                Semantics::new(Role::MenuItemRadio).label(l2.clone()).checked(chosen.get() == id).action(Action::Click)
            });
        let (o2, id3) = (opt.clone(), c.id);
        let opt = opt.bind(move || {
            o2.state(States::SELECTED, chosen.get() == id3);
            let _ = o2.clone().variant(if focus.get() == i { "focus" } else { "" });
        });
        options.push(opt.clone());
        list = list.child(opt);
    }
    let opts = options.clone();
    let move_to = move |i: usize| {
        focus.set(i);
        opts[i].focus();
    };
    let n = options.len();
    let c2 = close.clone();
    let list = list.on_key(move |e| {
        if !e.pressed {
            return false;
        }
        let at = focus.get_untracked().unwrap_or(0);
        match e.key {
            Key::Escape => c2(true),
            Key::ArrowDown => move_to((at + 1) % n),
            Key::ArrowUp => move_to((at + n - 1) % n),
            Key::Home => move_to(0),
            Key::End => move_to(n - 1),
            _ => return false,
        }
        true
    });
    let c3 = close.clone();
    let list = list.on_focus_within(move |inside| {
        // Tab out of the menu closes it.
        if !inside {
            c3(false);
        }
    });
    let list = w::place_at(list, Point::new(x, y));
    let c4 = close.clone();
    // A press anywhere else closes it (and goes on to what is under it).
    let catcher = ui
        .el("p_catch")
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
        })
        .pass_through()
        .on_pointer(move |e| {
            if e.phase == PointerPhase::Down {
                c4(false);
            }
            false
        });
    let layer = ui.overlay(ui.el("p_theme_layer").layout(|l| {
        l.position = Position::Absolute;
        l.inset = Insets::ZERO;
    }).pass_through().child(catcher).child(list));
    let start = focus.get_untracked().unwrap_or(0);
    options[start].focus();
    layer
}

/// The Kodi status chip (main.ts `refreshChip`): `running` in gold with a lit dot,
/// `starting` in the text color, `stopped` or `offline` muted, all in lower case.
fn status_chip(ui: &Ui, state: Signal<Chip>) -> El {
    let u0 = ui.clone();
    let dot = move || u0.el("p_chip_dot").width(7.2).height(7.2).layout(|l| l.shrink = 0.0).paint(|cx| {
        let c = Point::new(cx.size.width / 2.0, cx.size.height / 2.0);
        let r = cx.size.width / 2.0;
        let gold = cx.style.foreground;
        // box-shadow: 0 0 0 3px gold at 22%.
        cx.list.push(remedy_ui::paint::Item::FillPath {
            path: remedy_ui::paint::Path::circle(c, r + 3.0),
            brush: Brush::Solid(mix(gold, 0.22, Color::TRANSPARENT)),
            even_odd: false,
        });
        cx.list.push(remedy_ui::paint::Item::FillPath { path: remedy_ui::paint::Path::circle(c, r), brush: Brush::Solid(gold), even_odd: false });
    });
    let u = ui.clone();
    let chip = ui
        .el("p_chip")
        .row()
        .align(Align::Center)
        .gap(6.4)
        .variant(move || state.with(|s| s.0))
        .child(show(ui, move || state.with(|s| s.0 == "running"), dot).el())
        .child(u.el("p_chip_text").text(move || state.with(|s| s.1.clone())).nowrap())
        .semantics(move || Semantics::new(Role::Status).label(state.with(|s| s.1.clone())).description(state.with(|s| s.2.clone())));
    w::tooltip(&chip, move || state.with(|s| s.2.clone()));
    chip
}

/// The chip's (variant, text, title); `…` until the first answer.
type Chip = (&'static str, String, String);

fn refresh_chip(ctx: &Ctx, busy: Rc<Cell<bool>>, state: Signal<Chip>) {
    if busy.replace(true) {
        return;
    }
    ctx.api.run(api::status(), &Cancel::new(), move |out| {
        busy.set(false);
        let next = match out {
            Ok(s) => {
                let word = match s.state {
                    EngineState::Running => "running",
                    EngineState::Starting => "starting",
                    EngineState::Stopped => "stopped",
                };
                (word, word.to_owned(), format!("Kodi engine: {word}"))
            }
            Err(_) => ("stopped", "offline".to_owned(), "Portal is not answering".to_owned()),
        };
        let _ = state.try_set(next);
    });
}

/// The update pill (main.ts `paintUpdatePill`): to Settings, while an update is available
/// or installing. Not on a remote device.
fn update_pill(ui: &Ui, ctx: &Ctx) -> El {
    let info = ctx.update;
    let me = ctx.me;
    let (u, c) = (ui.clone(), ctx.clone());
    show(
        ui,
        move || {
            me.with(|m| m.as_ref().is_some_and(|m| m.this_pc))
                && info.with(|i| i.as_ref().is_some_and(|i| i.available || i.state == UpdateState::Installing))
        },
        move || {
            let text = info.with(|i| pill_text(i.as_ref()));
            let pill = crate::widgets::link(&u, &c.router, &Page::Server.path(), text, "pill", None);
            let latest = info.with(|i| i.as_ref().and_then(|i| i.latest.clone()));
            if let Some(v) = latest {
                w::tooltip(&pill, format!("Portal {v}"));
            }
            pill
        },
    )
    .el()
}

/// main.ts `updatePillText`.
pub fn pill_text(info: Option<&UpdateInfo>) -> String {
    match info.map(|i| (i.state, i.progress)) {
        Some((UpdateState::Downloading, p)) => match p.filter(|p| p.is_finite()) {
            Some(p) => format!("Downloading {}%", (p.clamp(0.0, 1.0) * 100.0).round()),
            None => "Downloading…".into(),
        },
        Some((UpdateState::Ready, _)) => "Update downloaded".into(),
        Some((UpdateState::Installing, _)) => "Updating…".into(),
        _ => "Update available".into(),
    }
}

/// The chip every 4 s and the update every 30 minutes, while the page is seen, and both
/// when it is seen again (main.ts:3797-3810).
fn start_polls(ui: &Ui, ctx: &Ctx, chip: Signal<Chip>) {
    let busy: Rc<Cell<bool>> = Rc::default();
    refresh_chip(ctx, busy.clone(), chip);
    let fetched = Rc::new(Cell::new(None::<f64>));
    refresh_update(ctx, fetched.clone(), Rc::default());
    fn chip_loop(ui: Ui, ctx: Ctx, busy: Rc<Cell<bool>>, chip: Signal<Chip>) {
        let u = ui.clone();
        ui.after(CHIP_POLL, move || {
            // The page is gone (this device was asked to pair): the loop ends with it.
            if chip.get_untracked().is_err() {
                return;
            }
            if !u.is_occluded() {
                refresh_chip(&ctx, busy.clone(), chip);
            }
            chip_loop(u, ctx, busy, chip);
        });
    }
    chip_loop(ui.clone(), ctx.clone(), busy.clone(), chip);
    fn update_loop(ui: Ui, ctx: Ctx, fetched: Rc<Cell<Option<f64>>>) {
        let u = ui.clone();
        ui.after(UPDATE_POLL, move || {
            if !u.is_occluded() {
                refresh_update(&ctx, fetched.clone(), Rc::default());
            }
            update_loop(u, ctx, fetched);
        });
    }
    update_loop(ui.clone(), ctx.clone(), fetched.clone());
    let (c, u) = (ctx.clone(), ui.clone());
    // `visibilitychange` to visible: only after the page was hidden, not when it first shows.
    let hidden = Rc::new(Cell::new(false));
    let _ = ui.root().on_visibility(move |seen| {
        if !seen {
            hidden.set(true);
            return;
        }
        if !hidden.replace(false) {
            return;
        }
        refresh_chip(&c, busy.clone(), chip);
        let stale = fetched.get().is_none_or(|t| now_secs(&u) - t > UPDATE_POLL.as_secs_f64());
        if stale {
            refresh_update(&c, fetched.clone(), Rc::default());
        }
    });
}

fn now_secs(ui: &Ui) -> f64 {
    ui.frame_time().as_secs_f64()
}

/// main.ts `refreshUpdate`/`noteUpdate`: only on the PC; a download or install is followed
/// every 1.5 s (15 minutes at most), and the page reloads once the new version answers.
fn refresh_update(ctx: &Ctx, fetched: Rc<Cell<Option<f64>>>, watching: Rc<Cell<bool>>) {
    if ctx.me.get_untracked().ok().flatten().is_some_and(|m| !m.this_pc) {
        return;
    }
    let c = ctx.clone();
    ctx.api.run(api::update(), &Cancel::new(), move |out| {
        // Portal is restarting, or it predates updates.
        let Ok(info) = out else { return };
        let ui = c.activity_ui();
        fetched.set(Some(now_secs(&ui)));
        let busy = matches!(info.state, UpdateState::Downloading | UpdateState::Installing);
        if busy && !watching.replace(true) {
            watch_update(&c, info.current.clone(), now_secs(&ui));
        }
        c.update.set(Some(info));
    });
}

fn watch_update(ctx: &Ctx, from: String, started: f64) {
    let (c, ui) = (ctx.clone(), ctx.activity_ui());
    let u = ui.clone();
    ui.after(UPDATE_WATCH, move || {
        if now_secs(&u) - started > UPDATE_WATCH_LIMIT.as_secs_f64() {
            return;
        }
        let c2 = c.clone();
        c.api.run(api::update(), &Cancel::new(), move |out| {
            let Ok(info) = out else {
                // Portal is down while the installer runs.
                watch_update(&c2, from, started);
                return;
            };
            if info.current != from {
                c2.app.reload();
                return;
            }
            let busy = matches!(info.state, UpdateState::Downloading | UpdateState::Installing);
            c2.update.set(Some(info));
            if busy {
                watch_update(&c2, from, started);
            }
        });
    });
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    let pad = |y: f32, x: f32| Insets { top: y, bottom: y, start: x, end: x };
    // .top: the background at 78% over a 16 px blur of what scrolls under it.
    s.rule(
        "p_header",
        None,
        States::NONE,
        StylePatch::default().background(mix(k.bg, 0.78, clear)).backdrop_blur(16.0),
    );
    s.rule("p_header_rule", None, States::NONE, StylePatch::default().background(mix(k.line, 0.85, clear)));
    s.rule("p_brand_bar", None, States::NONE, StylePatch::default().background(k.gold).radius(99.0));
    s.rule(
        "p_skip",
        None,
        States::NONE,
        StylePatch::default().background(k.gold).foreground(k.bg).weight(600.0).radius(999.0).padding(pad(7.2, 12.8)),
    );
    // .chip
    s.rule(
        "p_chip",
        None,
        States::NONE,
        StylePatch::default()
            .border(k.line)
            .border_width(1.0)
            .radius(999.0)
            .padding(pad(2.88, 11.52))
            .foreground(k.muted)
            .text_transform(TextTransform::Lower)
            .background(mix(k.raised, 0.70, clear)),
    );
    s.rule("p_chip", Some("running"), States::NONE, StylePatch::default().foreground(k.gold).border(k.gold));
    s.rule("p_chip", Some("starting"), States::NONE, StylePatch::default().foreground(k.text));
    s.rule("p_chip_dot", None, States::NONE, StylePatch::default().foreground(k.gold));
    // .theme-menu
    s.rule(
        "p_theme_menu",
        None,
        States::NONE,
        StylePatch::default()
            .background(mix(k.raised, 0.94, k.bg))
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .padding(Insets::all(6.4))
            .shadow(k.shadow_lift)
            .backdrop_blur(12.0),
    );
    s.rule(
        "p_theme_option",
        None,
        States::NONE,
        StylePatch::default().border(clear).border_width(1.0).radius(8.0).padding(pad(6.72, 12.0)).foreground(k.text).outline_offset(-2.0),
    );
    s.rule("p_theme_option", None, States::SELECTED, StylePatch::default().border(k.gold));
    s.rule("p_theme_option", Some("focus"), States::NONE, StylePatch::default().background(mix(k.gold, 0.12, k.raised)));
    s.rule("p_swatch", None, States::NONE, StylePatch::default().radius(99.0).border(k.line).border_width(1.0));
}
