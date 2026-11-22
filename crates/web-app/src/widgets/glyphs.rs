//! Portal's own glyphs, path for path from `web/index.html` and `saved.ts` `ICONS`, drawn in
//! the element's text color. Their `.line` parts are strokes (styles.css: 1.8 wide, round
//! caps and joins), the rest fills.

use std::sync::Arc;

use remedy_ui::{El, Ui};

const LINE: &str = r#"fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round""#;

fn svg(body: &str) -> Arc<[u8]> {
    // `.icon-btn svg text`: 700 6.5px var(--ui), centred.
    let body = body.replace("LINE", LINE).replace("TEN", r#"font-family="Remedy Sans UI" font-weight="700" font-size="6.5" text-anchor="middle""#);
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
    /// The player's (index.html `#dock-*`): pause, the 10 s skips, sound and muted, subtitles,
    /// picture in picture, theater mode, full screen and back, and the chevrons of Minimize,
    /// Expand and the episodes tab.
    Pause,
    Back10,
    Forward10,
    Volume,
    VolumeMuted,
    Captions,
    Pip,
    Theater,
    Fullscreen,
    FullscreenExit,
    ChevronDown,
    ChevronUp,
    ChevronLeft,
    ChevronRight,
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
            Glyph::Pause => r#"<path d="M7.5 5.5h3v13h-3zM13.5 5.5h3v13h-3z"/>"#,
            Glyph::Back10 => r#"<path LINE d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3.7h3.7"/><text TEN x="12" y="15.6">10</text>"#,
            Glyph::Forward10 => r#"<path LINE d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.7h-3.7"/><text TEN x="12" y="15.6">10</text>"#,
            Glyph::Volume => r#"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path LINE d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11"/>"#,
            Glyph::VolumeMuted => r#"<path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z"/><path LINE d="M16 9.5l5 5M21 9.5l-5 5"/>"#,
            Glyph::Captions => {
                r#"<rect LINE x="3" y="5.5" width="18" height="13" rx="2"/><path LINE d="M10.5 10.3a2.2 2.2 0 1 0 0 3.4M17 10.3a2.2 2.2 0 1 0 0 3.4"/>"#
            }
            Glyph::Pip => r#"<rect LINE x="3" y="5" width="18" height="14" rx="2"/><rect x="12" y="11.5" width="7" height="5.5" rx="1"/>"#,
            Glyph::Theater => r#"<rect LINE x="2" y="6.5" width="20" height="11" rx="2"/><path LINE d="M6 6.5v11M2 10h4"/>"#,
            Glyph::Fullscreen => r#"<path LINE d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5"/>"#,
            Glyph::FullscreenExit => r#"<path LINE d="M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5"/>"#,
            Glyph::ChevronDown => r#"<path LINE d="M6 9.5l6 6 6-6"/>"#,
            Glyph::ChevronUp => r#"<path LINE d="M6 14.5l6-6 6 6"/>"#,
            Glyph::ChevronLeft => r#"<path LINE d="M14 6l-6 6 6 6"/>"#,
            Glyph::ChevronRight => r#"<path LINE d="M10 6l6 6-6 6"/>"#,
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
            Glyph::Pause,
            Glyph::Back10,
            Glyph::Forward10,
            Glyph::Volume,
            Glyph::VolumeMuted,
            Glyph::Captions,
            Glyph::Pip,
            Glyph::Theater,
            Glyph::Fullscreen,
            Glyph::FullscreenExit,
            Glyph::ChevronDown,
            Glyph::ChevronUp,
            Glyph::ChevronLeft,
            Glyph::ChevronRight,
        ] {
            assert!(remedy_ui::svg::render(&g.bytes(), Some((24, 24)), None).is_ok(), "{g:?}");
        }
    }
}
