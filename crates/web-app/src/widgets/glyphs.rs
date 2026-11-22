//! Portal's own glyphs, path for path from `web/index.html` and `saved.ts` `ICONS`, drawn in
//! the element's text color. Their `.line` parts are strokes (styles.css: 1.8 wide, round
//! caps and joins), the rest fills.

use std::sync::Arc;

use remedy_ui::{El, Ui};

const LINE: &str = r#"fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round""#;

fn svg(body: &str) -> Arc<[u8]> {
    let body = body.replace("LINE", LINE);
    format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="currentColor">{body}</svg>"#)
        .into_bytes()
        .into()
}

/// Which glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// `PLAY_ICON` (main.ts).
    Play,
    /// `ICONS.info`.
    Info,
    /// `ICONS.watchlist`, `.when-off`: a list with a plus.
    WatchlistAdd,
    /// `ICONS.watchlist`, `.when-on`: a list with a tick.
    WatchlistOn,
    /// `ICONS.bookmarks`, `.when-off`: an outlined bookmark.
    BookmarkAdd,
    /// `ICONS.bookmarks`, `.when-on`: a filled bookmark.
    BookmarkOn,
    /// The Info dialog's close cross (info.ts).
    Close,
    /// The Info dialog's "opens elsewhere" arrow on its links (info.ts).
    External,
}

impl Glyph {
    fn source(self) -> &'static str {
        match self {
            Glyph::Play => r#"<path d="M8 5.5v13l10.5-6.5z"/>"#,
            Glyph::Info => r#"<circle LINE cx="12" cy="12" r="8.6"/><path LINE d="M12 11v5.2"/><circle cx="12" cy="7.9" r="1.15"/>"#,
            Glyph::WatchlistAdd => r#"<path LINE d="M4.5 7h11M4.5 12h11M4.5 17h6.5M17.5 14v7M14 17.5h7"/>"#,
            Glyph::WatchlistOn => r#"<path LINE d="M4.5 7h15M4.5 12h15M4.5 17h6.5M14 17.6l2.4 2.4 4.1-4.6"/>"#,
            Glyph::BookmarkAdd => r#"<path LINE d="M7 4.5h10v15.2l-5-3.7-5 3.7z"/>"#,
            Glyph::BookmarkOn => r#"<path d="M7 4.5h10v15.2l-5-3.7-5 3.7z"/>"#,
            Glyph::Close => r#"<path LINE d="M6 6l12 12M18 6L6 18"/>"#,
            Glyph::External => {
                r#"<path LINE d="M14 5h5v5M19 5l-8 8M17 14v4.5a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1-.5-.5v-11a.5.5 0 0 1 .5-.5H10"/>"#
            }
        }
    }

    /// The glyph as an SVG file.
    pub fn bytes(self) -> Arc<[u8]> {
        svg(self.source())
    }
}

/// `glyph` at `size` px square, decorative (the button around it carries the name).
#[track_caller]
pub fn glyph(ui: &Ui, glyph: Glyph, size: f32) -> El {
    remedy_ui::svg::icon(ui, glyph.bytes(), size, "").layout(|l| l.shrink = 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_is_a_drawable_svg() {
        for g in [
            Glyph::Play,
            Glyph::Info,
            Glyph::WatchlistAdd,
            Glyph::WatchlistOn,
            Glyph::BookmarkAdd,
            Glyph::BookmarkOn,
            Glyph::Close,
            Glyph::External,
        ] {
            assert!(remedy_ui::svg::render(&g.bytes(), Some((24, 24)), None).is_ok(), "{g:?}");
        }
    }
}
