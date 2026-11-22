//! Portal's dashboard on RemedyUI, built for the browser and served by Portal's server at
//! `/next/` beside the old page at `/` (see `docs/REMEDYUI-PORT.md`).
//!
//! Layout, so screens can be built side by side:
//! - [`api`]: the typed client, one function per route.
//! - [`pages`]: the hash routes and the [`pages::Page`] each one opens.
//! - [`theme`], [`fonts`]: per-browser look, in the old page's own localStorage items ([`stored`]).
//! - [`shell`]: the window: header, nav, theme menu and the routed page.
//! - [`screens`]: one file per screen, each `view(ui, ctx, ..) -> El`.
//! - [`widgets`]: pieces several screens share (art addresses, cards, page frames).

pub mod api;
pub mod fonts;
pub mod pages;
pub mod screens;
pub mod shell;
pub mod stored;
pub mod theme;
pub mod widgets;

use portal_api::PairMe;
use remedy_ui::reactive::Signal;
use remedy_ui::router::Router;
use remedy_ui::{App, AppConfig};

/// What every screen is handed: the API, the router, and the device's own choices.
#[derive(Clone)]
pub struct Ctx {
    pub app: App,
    pub api: api::Api,
    pub router: Router,
    /// The theme id (`theme::MENU`), per browser (`portal-theme`, shared with the old page).
    pub theme: Signal<String>,
    /// The text and heading faces, per browser (`portal-fonts`, shared with the old page).
    pub fonts: Signal<fonts::Fonts>,
    /// `GET /api/pair/me` once it answered: paired, and whether this is the PC itself
    /// (`this_pc` stands in for main.ts's `isRemote()`).
    pub me: Signal<Option<PairMe>>,
    /// The header's activity panel and the notice line under it.
    pub activity: widgets::activity::Activity,
    /// The window's width in CSS px, for the 720 px and 1600 px layouts.
    pub width: Signal<f32>,
    /// `GET /api/update` as last read (the header's pill; Settings' Updates card).
    pub update: Signal<Option<portal_api::UpdateInfo>>,
}

impl Ctx {
    /// Opens `page`, adding a history entry.
    pub fn go(&self, page: &pages::Page) {
        self.router.push(&page.path());
    }
}

/// The app's configuration: Portal's name, theme and sheet.
pub fn config() -> AppConfig {
    AppConfig::new("Portal").theme(theme::resolve(theme::DEFAULT, None), theme::sheet)
}

/// Starts the app (in a page: returns at once and the browser drives it).
pub fn start() {
    if let Err(e) = remedy_ui::run(config(), shell::app) {
        eprintln!("Portal could not start: {e}");
    }
}
