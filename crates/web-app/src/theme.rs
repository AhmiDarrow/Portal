//! Portal's themes, chosen per device as in `web/src/themes.ts`: the same ids, the same
//! menu order, "Portal" (the untouched page) by default and "System" following the
//! browser between Light and Dark. The palettes themselves are RemedyUI's
//! `remedy_ui_themes::portal_theme`, token for token from themes.ts.

use remedy_ui::style::{States, StylePatch, StyleSheet, Theme};
use remedy_ui::themes::{portal_page_layers, portal_sheet, portal_theme};

/// The old page's localStorage key, shared with it (`/` and `/next/` are one origin): the
/// theme id as plain text.
pub const KEY: &str = "portal-theme";

/// The default: Portal's own gold palette.
pub const DEFAULT: &str = "portal";

/// One entry of the theme menu: its id, name and swatch (a color, or a CSS gradient).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub id: &'static str,
    pub name: &'static str,
    pub swatch: &'static str,
}

/// The menu as themes.ts lists it: Portal, System, Light, names that do not start with
/// Dark, then Dark with Neutral Dark directly under it.
pub const MENU: &[Choice] = &[
    Choice { id: "portal", name: "Portal", swatch: "#e2b15a" },
    Choice { id: "system", name: "System", swatch: "linear-gradient(90deg, #0c0c14 50%, #f4f4f8 50%)" },
    Choice { id: "light", name: "Light", swatch: "#7c3aed" },
    Choice { id: "orange", name: "Amber", swatch: "#fb923c" },
    Choice { id: "purple", name: "Amethyst", swatch: "#c084fc" },
    Choice { id: "deutan", name: "Azure", swatch: "#56b4e9" },
    Choice { id: "green", name: "Emerald", swatch: "#34d399" },
    Choice { id: "tritan", name: "Lagoon", swatch: "#2fd0c0" },
    Choice { id: "cyan", name: "Ocean", swatch: "#22d3ee" },
    Choice { id: "protan", name: "Skyline", swatch: "#6cb6ff" },
    Choice { id: "dark", name: "Dark", swatch: "#8b5cf6" },
    Choice { id: "neutral", name: "Neutral Dark", swatch: "#94a3b8" },
    Choice { id: "forest", name: "Dark Forest", swatch: "#4d7a5a" },
    Choice { id: "alien", name: "Dark Purple", swatch: "#b026ff" },
];

/// This browser's saved choice, as the old page saved it.
pub fn saved() -> &'static str {
    crate::stored::item(KEY).map_or(DEFAULT, |id| known(&id))
}

/// A saved id the menu knows, else the default (a stale or edited value).
pub fn known(id: &str) -> &'static str {
    MENU.iter().find(|c| c.id == id).map_or(DEFAULT, |c| c.id)
}

/// The palette `id` stands for: "system" is Light when the browser prefers light, else Dark.
pub fn palette_id(id: &str, prefers_dark: Option<bool>) -> &'static str {
    match known(id) {
        "system" if prefers_dark == Some(false) => "light",
        "system" => "dark",
        other => other,
    }
}

/// The theme for the choice `id`.
pub fn resolve(id: &str, prefers_dark: Option<bool>) -> Theme {
    portal_theme(palette_id(id, prefers_dark))
        .or_else(|| portal_theme(DEFAULT))
        .unwrap_or_else(Theme::dark_forest)
}

/// Portal's sheet, plus `portal_page`: the page's background color under its two gold glows
/// (styles.css `html, body`).
pub fn sheet(t: &Theme) -> StyleSheet {
    let mut s = portal_sheet(t);
    s.rule(
        "portal_page",
        None,
        States::NONE,
        StylePatch::default().background(t.colors.bg).background_layers(portal_page_layers()),
    );
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_entry_has_a_palette() {
        for c in MENU {
            assert!(portal_theme(palette_id(c.id, None)).is_some(), "{}", c.id);
        }
        assert_eq!(MENU.len(), 14);
    }

    #[test]
    fn system_follows_the_browser_and_unknown_ids_fall_back() {
        assert_eq!(palette_id("system", Some(false)), "light");
        assert_eq!(palette_id("system", Some(true)), "dark");
        assert_eq!(palette_id("system", None), "dark");
        assert_eq!(palette_id("no-such-theme", None), "portal");
        assert_eq!(resolve("deutan", None).name, "Azure");
    }
}
