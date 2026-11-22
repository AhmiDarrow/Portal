//! Portal's themes, chosen per device as in `web/src/themes.ts`: the same ids, the same
//! menu order, "Portal" (the untouched page) by default and "System" following the
//! browser between Light and Dark. The palettes themselves are RemedyUI's
//! `remedy_ui_themes::portal_theme`, token for token from themes.ts.

use remedy_ui::core::Color;
use remedy_ui::style::{Gradient, LineHeight, Shadow, States, StylePatch, StyleSheet, Theme};
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
/// (styles.css `html, body`), and the shell's and the shared widgets' own rules.
pub fn sheet(t: &Theme) -> StyleSheet {
    let mut s = portal_sheet(t);
    s.rule(
        "portal_page",
        None,
        States::NONE,
        StylePatch::default()
            .background(t.colors.bg)
            .background_layers(portal_page_layers())
            .line_height(LineHeight::Relative(1.5)),
    );
    let k = Look::of(t);
    // h1, h2, h3 (styles.css): the heading face at 400, 32 / 24 / 16 px.
    for (v, px, lh) in [("1", 32.0, 1.2), ("2", 24.0, 1.25), ("3", 16.0, 1.4)] {
        let p = StylePatch::default()
            .heading(true)
            .weight(400.0)
            .text_size(px)
            .foreground(k.text)
            .line_height(LineHeight::Relative(lh));
        s.rule("heading", Some(v), States::NONE, p);
    }
    s.rule("label", Some("muted"), States::NONE, StylePatch::default().foreground(k.muted));
    s.rule("label", Some("error"), States::NONE, StylePatch::default().foreground(k.error));
    crate::shell::rules(&mut s, &k);
    crate::widgets::rules(&mut s, &k);
    s
}

/// Portal's CSS variables (styles.css `:root`, themes.ts), read off the theme.
#[derive(Clone, Debug)]
pub struct Look {
    pub bg: Color,
    pub raised: Color,
    pub well: Color,
    pub line: Color,
    pub text: Color,
    pub muted: Color,
    pub gold: Color,
    pub error: Color,
    pub track: Color,
    /// `--shadow`: 0 12px 32px black at 28%.
    pub shadow: Shadow,
    /// `--shadow-lift`: 0 18px 40px black at 36%.
    pub shadow_lift: Shadow,
}

impl Look {
    pub fn of(t: &Theme) -> Look {
        let c = &t.colors;
        let custom = |k: &str, or: Color| c.custom.get(k).copied().unwrap_or(or);
        let line = c.line.unwrap_or(c.muted);
        Look {
            bg: c.bg,
            raised: custom("raised", c.panel),
            well: custom("well", c.card),
            line,
            text: c.text,
            muted: c.muted,
            gold: custom("gold", c.accent),
            error: c.danger,
            track: custom("track", line),
            shadow: Shadow::new(0.0, 12.0, 32.0, mix(Color::BLACK, 0.28, Color::TRANSPARENT)),
            shadow_lift: Shadow::new(0.0, 18.0, 40.0, mix(Color::BLACK, 0.36, Color::TRANSPARENT)),
        }
    }
}

/// CSS `color-mix(in srgb, a p, b)`: premultiplied, channel by channel in sRGB.
pub fn mix(a: Color, p: f32, b: Color) -> Color {
    let q = 1.0 - p;
    let alpha = a.a * p + b.a * q;
    if alpha <= 0.0 {
        return Color::TRANSPARENT;
    }
    let ch = |x: f32, y: f32| (x * a.a * p + y * b.a * q) / alpha;
    Color::new(ch(a.r, b.r), ch(a.g, b.g), ch(a.b, b.b), alpha)
}

/// One of Portal's CSS backgrounds as written (it may name `var(--gold)` and the like).
pub fn gradient(css: &str) -> Vec<Gradient> {
    Gradient::parse_layers(css).unwrap_or_default()
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
    fn mixes_like_css_color_mix() {
        let gold = Color::from_hex("#e2b15a").unwrap();
        let m = mix(gold, 0.22, Color::TRANSPARENT);
        assert!((m.r - gold.r).abs() < 1e-6 && (m.a - 0.22).abs() < 1e-6);
        let m = mix(Color::WHITE, 0.5, Color::BLACK);
        assert!((m.r - 0.5).abs() < 1e-6 && m.a == 1.0);
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
