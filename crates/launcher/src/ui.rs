//! What the launcher shows: Portal's look for its dialogs (the dashboard's palette and
//! faces), its icon for the tray, and a notice from a launch that has no tray.

use remedy_ui::host::{TrayImage, WindowIcon};
use remedy_ui::themes::{portal_sheet, portal_theme};
use remedy_ui::{App, AppConfig, ConfirmSpec};

/// The app's name: its window titles, notifications and single instance.
pub const NAME: &str = "Portal";

/// Portal's own gold palette and its faces, as the dashboard shows them by default.
pub fn config() -> AppConfig {
    let config = AppConfig::new(NAME).app_id(crate::autostart::ID).cpu_only();
    match portal_theme("portal") {
        Some(theme) => config.theme(theme, portal_sheet),
        None => config,
    }
}

/// A question the tray asks, in the words the message box used: the first paragraph is
/// the heading, the rest the text, answered Yes or No with No first in focus.
pub fn question(text: &str) -> ConfirmSpec {
    let (title, rest) = split(text);
    ConfirmSpec::new(title, rest)
        .ok("Yes")
        .cancel("No")
        .danger(true)
}

/// A message with one OK button, titled "Portal" as the message box was.
pub fn notice(text: &str) -> ConfirmSpec {
    ConfirmSpec::new(NAME, text).notice()
}

/// The heading and the text of a question: its first sentence when that is a question with
/// more after it ("Restart the Portal server? Anything playing will stop.", "Quit Portal?"
/// and its paragraphs), else "Portal" over all of it, as the message box's title was.
fn split(text: &str) -> (String, String) {
    let first = text.split("\n\n").next().unwrap_or(text);
    let more = first.len() < text.trim_end().len();
    let cut = first
        .find("? ")
        .map(|at| at + 1)
        .or_else(|| (first.ends_with('?') && more).then_some(first.len()));
    match cut {
        Some(at) => (text[..at].trim().to_string(), text[at..].trim().to_string()),
        None => (NAME.to_string(), text.to_string()),
    }
}

/// Shows `message` from a launch that is not the tray (`--stop` with nothing to stop, say)
/// and returns once it is dismissed.
pub fn alert_alone(message: &str) {
    let message = message.to_string();
    let _ = remedy_ui::run(config(), move |app: &App| {
        let quit = app.clone();
        app.confirm(notice(&message), move |_| quit.quit());
    });
}

/// Portal's icon (assets/portal.ico, every size it is drawn at) for the tray.
pub fn icon() -> (TrayImage, Option<WindowIcon>) {
    let drawn = ico_sizes(include_bytes!("../../../assets/portal.ico"));
    let artwork = drawn.into_iter().fold(None::<WindowIcon>, |all, icon| {
        Some(match all {
            Some(all) => all.with_size(icon),
            None => icon,
        })
    });
    let image = artwork.as_ref().map_or_else(
        // A blank square rather than no tray at all.
        || TrayImage {
            size: 16,
            rgba: vec![0; 16 * 16 * 4],
        },
        |a| {
            let small = a.scaled(32);
            TrayImage {
                size: small.size(),
                rgba: small.rgba().to_vec(),
            }
        },
    );
    (image, artwork)
}

/// The PNG images an ICO file holds, decoded.
fn ico_sizes(ico: &[u8]) -> Vec<WindowIcon> {
    let word = |at: usize| {
        ico.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let long = |at: usize| {
        ico.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let count = word(4).unwrap_or(0);
    (0..count)
        .filter_map(|i| {
            let entry = 6 + 16 * i;
            let (len, at) = (long(entry + 8)?, long(entry + 12)?);
            let decoded =
                remedy_ui::image::decode(ico.get(at..at.checked_add(len)?)?, None).ok()?;
            let image = decoded.image;
            if image.width != image.height {
                return None;
            }
            WindowIcon::new(image.width, image.pixels.to_vec())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn questions_keep_their_words() {
        let (title, text) = split("Restart the Portal server? Anything playing will stop.");
        assert_eq!(title, "Restart the Portal server?");
        assert_eq!(text, "Anything playing will stop.");
        let (title, text) =
            split("Quit Portal?\n\nThis stops the Portal server.\n\nOpen it again.");
        assert_eq!(title, "Quit Portal?");
        assert_eq!(text, "This stops the Portal server.\n\nOpen it again.");
        let lan =
            "Changing who can reach Portal can restart it, which stops anything playing. Continue?";
        assert_eq!(split(lan), ("Portal".to_string(), lan.to_string()));
        let (title, text) = split("Portal is not running.");
        assert_eq!(
            (title.as_str(), text.as_str()),
            ("Portal", "Portal is not running.")
        );
    }

    #[test]
    fn the_icon_has_every_drawn_size() {
        let (image, artwork) = icon();
        assert_eq!(image.size, 32);
        assert!(image.rgba.iter().any(|b| *b != 0), "drawn");
        let sizes: Vec<u32> = artwork
            .unwrap()
            .drawn()
            .iter()
            .map(WindowIcon::size)
            .collect();
        assert!(sizes.contains(&16) && sizes.contains(&256), "{sizes:?}");
    }
}
