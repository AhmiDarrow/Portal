//! The typefaces the page is set in, one for text and one for headings, chosen per device as
//! in `web/src/fonts.ts`: the same ids and names, Remedy Sans for text and Ahmi Pixel for
//! headings by default. Every face is built into the app (`remedy-ui-fonts`).

use remedy_ui::style::Theme;
use serde::{Deserialize, Serialize};

/// The old page's localStorage key, shared with it: `{"text": .., "headings": ..}`.
pub const KEY: &str = "portal-fonts";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontId {
    RemedySans,
    RemedySansUi,
    AhmiPixel,
    AhmiPrint,
    System,
}

impl FontId {
    /// The menu, in fonts.ts order.
    pub const ALL: [FontId; 5] = [FontId::RemedySans, FontId::RemedySansUi, FontId::AhmiPixel, FontId::AhmiPrint, FontId::System];

    pub fn name(self) -> &'static str {
        match self {
            FontId::RemedySans => "Remedy Sans",
            FontId::RemedySansUi => "Remedy Sans UI",
            FontId::AhmiPixel => "Ahmi Pixel",
            FontId::AhmiPrint => "AhmiPrint",
            FontId::System => "System default",
        }
    }

    /// The families tried in order, as fonts.ts's CSS stacks name them. The engine's own
    /// faces stand in after them for anything they lack.
    pub fn families(self) -> Vec<String> {
        let f: &[&str] = match self {
            FontId::RemedySans => &["Remedy Sans"],
            FontId::RemedySansUi => &["Remedy Sans UI"],
            FontId::AhmiPixel => &["Ahmi Pixel", "Remedy Sans"],
            FontId::AhmiPrint => &["AhmiPrint"],
            // The browser's own `system-ui` face once [`load_system`] has read it; until
            // then, or where the browser cannot give it, the engine's UI face after it.
            FontId::System => &["system-ui"],
        };
        f.iter().map(|s| (*s).to_owned()).chain(["Remedy Sans UI".to_owned()]).collect()
    }
}

/// The two choices. A saved value that is not a known font keeps that field's default.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fonts {
    #[serde(default = "text_default", deserialize_with = "or_text")]
    pub text: FontId,
    #[serde(default = "headings_default", deserialize_with = "or_headings")]
    pub headings: FontId,
}

fn text_default() -> FontId {
    FontId::RemedySans
}

fn headings_default() -> FontId {
    FontId::AhmiPixel
}

fn or_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<FontId, D::Error> {
    Ok(FontId::deserialize(d).unwrap_or(text_default()))
}

fn or_headings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<FontId, D::Error> {
    Ok(FontId::deserialize(d).unwrap_or(headings_default()))
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts { text: text_default(), headings: headings_default() }
    }
}

impl Fonts {
    /// Either face is "System default".
    pub fn uses_system(self) -> bool {
        self.text == FontId::System || self.headings == FontId::System
    }

    /// Sets the theme's text and heading families to these choices.
    pub fn apply(self, theme: &mut Theme) {
        theme.font_families = self.text.families();
        theme.mono_families = theme.font_families.clone();
        theme.heading_families = self.headings.families();
    }
}

/// Reads the face the browser draws `system-ui` in, for "System default" (call it from the
/// press that picks it, and at start when it is saved). Firefox, Safari and phones cannot
/// give it, and the person may decline: then text stays in Remedy Sans UI, as `families` says.
pub fn load_system(app: &remedy_ui::App) {
    app.load_system_ui_font(|out| {
        if let Err(why) = out {
            eprintln!("System font not available: {why}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fonts_ts_values_and_keeps_defaults_for_unknown_ones() {
        let f: Fonts = serde_json::from_str(r#"{"text":"ahmi-print","headings":"system"}"#).unwrap();
        assert_eq!(f, Fonts { text: FontId::AhmiPrint, headings: FontId::System });
        let f: Fonts = serde_json::from_str(r#"{"text":"comic-sans"}"#).unwrap();
        assert_eq!(f, Fonts::default());
        assert_eq!(serde_json::to_string(&Fonts::default()).unwrap(), r#"{"text":"remedy-sans","headings":"ahmi-pixel"}"#);
    }

    #[test]
    fn the_default_matches_portal_theme() {
        let mut t = crate::theme::resolve("portal", None);
        let before = (t.font_families[0].clone(), t.heading_families[0].clone());
        Fonts::default().apply(&mut t);
        assert_eq!((t.font_families[0].clone(), t.heading_families[0].clone()), before);
    }
}
