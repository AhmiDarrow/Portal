//! The page routes, the same hash routes as `web/src/main.ts` (`#/browse/<path>`,
//! `#/title/<id>`, `#/show/<id>/<season>`, `#/settings/<addon>`), so old bookmarks open the
//! same pages. Ids are written with `encodeURIComponent`, so an id is always one segment.

use remedy_ui::router::Route;

use crate::api::encode;

/// One page of the dashboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    /// A folder of the add-on, by its plugin path (empty: the add-on's own top folder).
    Browse(String),
    /// Details of a movie or show.
    Title(String),
    /// A show's seasons and episodes, opened at a season or where watching left off.
    Show { id: String, season: Option<u32> },
    Watchlist,
    Bookmarks,
    History,
    /// The add-on source, repositories and updates.
    Source,
    /// One add-on's settings.
    AddonSettings(String),
    /// Portal's own settings ("Settings" in the nav).
    Server,
    /// The TV-browser receiver; it stays on the old UI.
    Cast,
    NotFound,
}

impl Page {
    /// The page a route names, as `main.ts`'s `render` picks it.
    pub fn from_route(route: &Route) -> Page {
        let segs = route.segments();
        let rest = || segs.get(1..).unwrap_or_default().join("/");
        match segs.first().map(String::as_str) {
            None | Some("home") => Page::Home,
            Some("browse") => Page::Browse(rest()),
            Some("title") => Page::Title(rest()),
            Some("show") => Page::Show {
                id: segs.get(1).cloned().unwrap_or_default(),
                // Only `\d{1,3}` is a season, as showRoute reads it.
                season: segs
                    .get(2)
                    .filter(|s| (1..=3).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|s| s.parse().ok()),
            },
            Some("watchlist") => Page::Watchlist,
            Some("bookmarks") => Page::Bookmarks,
            Some("history") => Page::History,
            Some("source") => Page::Source,
            Some("settings") => Page::AddonSettings(rest()),
            Some("server") => Page::Server,
            Some("cast") => Page::Cast,
            Some(_) => Page::NotFound,
        }
    }

    /// The route to push for this page (`router.push(&page.path())`).
    pub fn path(&self) -> String {
        match self {
            Page::Home => "/".into(),
            Page::Browse(p) => format!("/browse/{}", encode(p)),
            Page::Title(id) => format!("/title/{}", encode(id)),
            Page::Show { id, season: Some(s) } => format!("/show/{}/{s}", encode(id)),
            Page::Show { id, season: None } => format!("/show/{}", encode(id)),
            Page::Watchlist => "/watchlist".into(),
            Page::Bookmarks => "/bookmarks".into(),
            Page::History => "/history".into(),
            Page::Source => "/source".into(),
            Page::AddonSettings(id) => format!("/settings/{}", encode(id)),
            Page::Server => "/server".into(),
            Page::Cast => "/cast".into(),
            Page::NotFound => "/".into(),
        }
    }

    /// The page's name in the title, as `PAGE_TITLES` has it.
    pub fn name(&self) -> &'static str {
        match self {
            Page::Home => "Home",
            Page::Browse(_) => "Browse",
            Page::Title(_) => "Details",
            Page::Show { .. } => "Show",
            Page::Watchlist => "Watchlist",
            Page::Bookmarks => "Bookmarks",
            Page::History => "History",
            Page::Source => "Source",
            Page::AddonSettings(_) => "Add-on settings",
            Page::Server => "Settings",
            Page::Cast => "TV",
            Page::NotFound => "Portal",
        }
    }

    /// The window title: "Watchlist · Portal", or "Portal".
    pub fn title(&self) -> String {
        match self.name() {
            "Portal" => "Portal".into(),
            name => format!("{name} · Portal"),
        }
    }

    /// The nav entry marked current: an add-on's settings are reached from Source.
    pub fn nav(&self) -> Option<Nav> {
        match self {
            Page::Home => Some(Nav::Home),
            Page::Watchlist => Some(Nav::Watchlist),
            Page::Bookmarks => Some(Nav::Bookmarks),
            Page::History => Some(Nav::History),
            Page::Source | Page::AddonSettings(_) => Some(Nav::Source),
            Page::Server => Some(Nav::Settings),
            _ => None,
        }
    }
}

/// The header's nav, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Home,
    Watchlist,
    Bookmarks,
    History,
    Source,
    Settings,
}

impl Nav {
    pub const ALL: [Nav; 6] = [Nav::Home, Nav::Watchlist, Nav::Bookmarks, Nav::History, Nav::Source, Nav::Settings];

    pub fn label(self) -> &'static str {
        match self {
            Nav::Home => "Home",
            Nav::Watchlist => "Watchlist",
            Nav::Bookmarks => "Bookmarks",
            Nav::History => "History",
            Nav::Source => "Source",
            Nav::Settings => "Settings",
        }
    }

    pub fn page(self) -> Page {
        match self {
            Nav::Home => Page::Home,
            Nav::Watchlist => Page::Watchlist,
            Nav::Bookmarks => Page::Bookmarks,
            Nav::History => Page::History,
            Nav::Source => Page::Source,
            Nav::Settings => Page::Server,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(hash: &str) -> Page {
        Page::from_route(&Route::parse(hash))
    }

    #[test]
    fn old_bookmarks_open_their_pages() {
        assert_eq!(page("#/"), Page::Home);
        assert_eq!(page(""), Page::Home);
        assert_eq!(page("#/home"), Page::Home);
        assert_eq!(page("#/browse/plugin%3A%2F%2Fplugin.video.x%2F%3Fmode%3D1"), Page::Browse("plugin://plugin.video.x/?mode=1".into()));
        assert_eq!(page("#/browse/"), Page::Browse(String::new()));
        assert_eq!(page("#/title/tt0000001"), Page::Title("tt0000001".into()));
        assert_eq!(page("#/show/abc%2Fdef/2"), Page::Show { id: "abc/def".into(), season: Some(2) });
        assert_eq!(page("#/show/abc/x"), Page::Show { id: "abc".into(), season: None });
        assert_eq!(page("#/show/abc/1234"), Page::Show { id: "abc".into(), season: None });
        assert_eq!(page("#/settings/plugin.video.x"), Page::AddonSettings("plugin.video.x".into()));
        assert_eq!(page("#/server"), Page::Server);
        assert_eq!(page("#/cast"), Page::Cast);
        assert_eq!(page("#/nowhere"), Page::NotFound);
    }

    #[test]
    fn paths_round_trip() {
        for p in [
            Page::Home,
            Page::Browse("plugin://plugin.video.x/?a=1&b=two words".into()),
            Page::Title("t/1".into()),
            Page::Show { id: "s 1".into(), season: Some(3) },
            Page::Show { id: "s".into(), season: None },
            Page::AddonSettings("plugin.video.x".into()),
            Page::Server,
        ] {
            assert_eq!(Page::from_route(&Route::parse(&p.path())), p);
        }
    }

    #[test]
    fn titles_and_nav_follow_main_ts() {
        assert_eq!(Page::Home.title(), "Home · Portal");
        assert_eq!(Page::NotFound.title(), "Portal");
        assert_eq!(Page::AddonSettings("x".into()).nav(), Some(Nav::Source));
        assert_eq!(Page::Server.nav(), Some(Nav::Settings));
    }
}
