//! The window around every page: the header (brand, nav, theme menu) over the routed page,
//! the player dock, and the pairing screen when the server asks this device to pair.

use remedy_ui::reactive::{Cancel, Signal};
use remedy_ui::router::Router;
use remedy_ui::widgets::{self as w, MenuItem};
use remedy_ui::{show, App, El, Ui, WindowSpec};

use crate::fonts::Fonts;
use crate::pages::{Nav, Page};
use crate::{api, screens, theme, Ctx};

/// Where the image loader retries art from sites that send no CORS headers.
pub const IMAGE_PROXY: &str = "/api/image/web?url=";

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
        };
        ctx.router.set_title(|r| Page::from_route(r).title());
        let me = ctx.me;
        ctx.api.run(api::pair_me(), &Cancel::new(), move |out| me.set(out.ok()));
        let root = ui.root().child(page(ui, &ctx));
        let (app, t, f) = (a.clone(), ctx.theme, ctx.fonts);
        let _ = root.bind(move || {
            let mut look = theme::resolve(&t.get(), prefers_dark.get());
            f.get().apply(&mut look);
            app.set_theme(look);
        });
    });
}

/// The page: header and routed content, or the pairing screen while pairing is needed.
fn page(ui: &Ui, ctx: &Ctx) -> El {
    let pair = ctx.api.pair_needed();
    let (u1, u2, c1, c2) = (ui.clone(), ui.clone(), ctx.clone(), ctx.clone());
    ui.el("portal_page")
        .column()
        .grow(1.0)
        .fill_width()
        .child(show(ui, move || pair.get(), move || screens::pairing::view(&u1, &c1)).el())
        .child(show(ui, move || !pair.get(), move || routed(&u2, &c2)).el())
}

fn routed(ui: &Ui, ctx: &Ctx) -> El {
    let (u, c) = (ui.clone(), ctx.clone());
    let content = ctx.router.view(ui, move |route| screens::page(&u, &c, &Page::from_route(route)));
    w::column(ui)
        .grow(1.0)
        .fill_width()
        .child(header(ui, ctx))
        .child(w::scroll(ui).child(content))
        .child(screens::player::view(ui, ctx))
}

fn header(ui: &Ui, ctx: &Ctx) -> El {
    let mut nav = w::row(ui).gap(16.0);
    for item in Nav::ALL {
        nav = nav.child(ctx.router.link_to(ui, &item.page().path(), item.label()));
    }
    let chosen = ctx.theme;
    let themes = theme::MENU
        .iter()
        .map(|c| MenuItem::radio_signal(c.name, chosen, c.id.to_owned()))
        .collect();
    w::row(ui)
        .gap(24.0)
        .padding(16.0)
        .fill_width()
        .child(ctx.router.link_to(ui, "/", "Portal"))
        .child(nav)
        .child(w::spacer(ui))
        .child(w::menu_button(ui, "Theme", themes))
}
