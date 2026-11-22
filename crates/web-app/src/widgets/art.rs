//! Pictures: where art is fetched from (api.ts `artUrl`), the poster an add-on put in a
//! title's address (main.ts `artFromId`), and the art block (main.ts `artBlock`): a 2:3
//! frame on the well under a soft gold glow, framed wide (16:9) or square (whole, inset) by
//! the picture's real shape, with the title's first letter when there is no picture or it
//! cannot be shown.

use remedy_ui::core::Insets;
use remedy_ui::image::{image_ext, Fit, ImageOptions, ImageState, Source};
use remedy_ui::layout::{Align, Justify, Length, Position};
use remedy_ui::reactive::Signal;
use remedy_ui::style::{BorderEdge, BorderSide, States, StylePatch, StyleSheet};
use remedy_ui::{El, Ui};

use crate::api::encode;
use crate::theme::{gradient, Look};

/// Kodi art (`image://`, `special://`) through the server's `/api/image`; plain `http(s)`
/// as is (the image loader retries it through `/api/image/web` when the site sends no CORS
/// headers); anything else has no picture.
pub fn art_url(path: Option<&str>) -> Option<String> {
    let path = path.filter(|p| !p.is_empty())?;
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("image://") || lower.starts_with("special://") {
        return Some(format!("/api/image?path={}", encode(path)));
    }
    (lower.starts_with("http://") || lower.starts_with("https://")).then(|| path.to_owned())
}

/// The poster an add-on put in a title's own address (`art={"poster": ...}` in its plugin
/// link), if any.
pub fn poster_from_id(id: &str) -> Option<String> {
    art_from_id(id, &["poster", "tvshow.poster"])
}

/// The first of `keys` an add-on put in the `art` of a title's address, when it is https.
pub fn art_from_id(id: &str, keys: &[&str]) -> Option<String> {
    let (_, query) = id.split_once('?')?;
    let raw = query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (decode_component(k) == "art").then(|| decode_component(v))
    })?;
    let art: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&raw).ok()?;
    keys.iter().find_map(|k| art.get(*k)?.as_str().filter(|v| v.starts_with("https://")).map(str::to_owned))
}

/// `URLSearchParams` decoding: `+` is a space, `%XX` a byte, bad escapes kept as written.
fn decode_component(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(v) => {
                        out.push(v);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The title's letter for the fallback tile: its first character, upper case, or `P`.
pub fn letter(name: &str) -> String {
    name.trim().chars().next().map_or_else(|| "P".to_owned(), |c| c.to_uppercase().collect())
}

/// How a picture sits in its frame, from its real shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    /// 2:3, cropped to fill.
    Poster,
    /// Wider than 1.2:1: a 16:9 frame, cropped to fill.
    Wide,
    /// 0.85 to 1.2: the poster frame, the picture whole and inset by 18% of the width.
    Square,
}

impl Frame {
    /// main.ts's `load` listener: `ratio > 1.2` wide, `0.85 <= ratio <= 1.2` square.
    pub fn of(width: u32, height: u32) -> Frame {
        let ratio = width as f32 / height.max(1) as f32;
        if ratio > 1.2 {
            Frame::Wide
        } else if (0.85..=1.2).contains(&ratio) {
            Frame::Square
        } else {
            Frame::Poster
        }
    }

    fn aspect(self) -> f32 {
        if self == Frame::Wide { 16.0 / 9.0 } else { 2.0 / 3.0 }
    }
}

/// The art block for `art` (a Kodi or web address) named `name`, filling its parent's width.
/// `variant` picks the frame's surroundings: "card" adds the card's hairline under it.
pub fn art(ui: &Ui, art: Option<&str>, name: &str, variant: &'static str) -> El {
    let rule = variant == "card";
    let frame = Signal::new(Frame::Poster);
    let width = Signal::new(0.0f32);
    let inner = ui.el("p_art").column().align(Align::Center).justify(Justify::Center).grow(1.0).fill_width();
    let inner = match art_url(art) {
        None => inner.child(letter_tile(ui, name)),
        Some(url) => {
            let (u2, n) = (ui.clone(), name.to_owned());
            let fitted = move |fit: Fit| {
                let u = u2.clone();
                let n = n.clone();
                let options = ImageOptions::new()
                    // The frame's own glow shows while the picture loads, as an <img> does.
                    .placeholder(u2.el("p_art_wait"))
                    .fallback(move || letter_tile(&u, &n))
                    .on_state(move |s| {
                        if let ImageState::Sized((w, h)) | ImageState::Ready(_, (w, h)) = s {
                            let f = Frame::of(*w, *h);
                            if frame.get_untracked().ok() != Some(f) {
                                frame.set(f);
                            }
                        }
                    });
                image_ext(&u2, Source::Url(url.clone()), "", fit, options).layout(|l| {
                    l.width = Length::Percent(1.0);
                    l.height = Length::Percent(1.0);
                })
            };
            let holder = ui.el("p_art_holder").column().grow(1.0).fill_width();
            let h2 = holder.clone();
            // A square picture shows whole (contain); the others fill the frame (cover).
            let shown = Signal::new(None::<bool>);
            holder.bind(move || {
                let square = frame.get() == Frame::Square;
                if shown.get_untracked().ok().flatten() != Some(square) {
                    shown.set(Some(square));
                    h2.clear();
                    let _ = h2.clone().child(fitted(if square { Fit::Contain } else { Fit::Cover }));
                }
            })
        }
    };
    // The frame keeps its aspect ratio (CSS `aspect-ratio` on the border box, the hairline
    // included); the picture box and the hairline are laid over it, so nothing inside feeds
    // back into its size.
    let line = if rule { 1.0 } else { 0.0 };
    let inner = inner.layout(move |l| {
        l.position = Position::Absolute;
        l.inset = Insets { top: 0.0, start: 0.0, end: 0.0, bottom: line };
        l.grow = 0.0;
    });
    // A square picture is inset by 18% of the width (`padding: 18%`).
    let i2 = inner.clone();
    let inner = inner.bind(move || {
        let pad = if frame.get() == Frame::Square { width.get() * 0.18 } else { 0.0 };
        let _ = i2.clone().padding(Insets::all(pad));
    });
    let outer = ui.el("p_art_frame").variant(if rule { "card" } else { "" }).fill_width().layout(|l| l.position = Position::Relative).child(inner);
    let o2 = outer.clone();
    let outer = outer.bind(move || {
        let a = frame.get().aspect();
        let _ = o2.clone().layout(move |l| l.aspect_ratio = Some(a));
    });
    outer.on_resize(move |s| {
        // The inset of a square picture follows the width; it does not change the frame's size.
        if width.get_untracked().is_ok_and(|w| (w - s.width).abs() > 0.5) {
            width.set(s.width);
        }
    })
}

/// The letter frame for a title with no picture (`div.art.empty`).
pub fn letter_tile(ui: &Ui, name: &str) -> El {
    ui.el("p_art_letter")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        .grow(1.0)
        .fill_width()
        .child(ui.el("p_art_letter_text").text(letter(name)).nowrap())
}

/// `.art`'s glow over the well, as styles.css writes it.
pub const GLOW: &str = "radial-gradient(circle at 50% 42%, color-mix(in srgb, var(--gold) 16%, transparent), transparent 62%)";

pub fn rules(s: &mut StyleSheet, k: &Look) {
    // .art: the well under a gold glow around 50% 42%.
    s.rule("p_art", None, States::NONE, StylePatch::default().background(k.well).background_layers(gradient(GLOW)));
    s.rule("p_art_frame", Some("card"), States::NONE, StylePatch::default().border_side(BorderEdge::Bottom, BorderSide::new(1.0, k.line)));
    s.rule("p_art_letter_text", None, States::NONE, StylePatch::default().foreground(k.gold).heading(true).text_size(32.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_route_as_art_url_does() {
        assert_eq!(art_url(Some("image://C%3a%5cx.jpg/")).as_deref(), Some("/api/image?path=image%3A%2F%2FC%253a%255cx.jpg%2F"));
        assert_eq!(art_url(Some("HTTPS://example.com/p.jpg")).as_deref(), Some("HTTPS://example.com/p.jpg"));
        assert_eq!(art_url(Some("C:\\x.jpg")), None);
        assert_eq!(art_url(Some("")), None);
        assert_eq!(art_url(None), None);
    }

    #[test]
    fn reads_the_poster_from_a_plugin_address() {
        let art = encode(r#"{"poster":"https://img/p.jpg","fanart":"https://img/f.jpg"}"#);
        let id = format!("plugin://plugin.video.x/?mode=play&art={art}&x=1");
        assert_eq!(poster_from_id(&id).as_deref(), Some("https://img/p.jpg"));
        assert_eq!(art_from_id(&id, &["fanart"]).as_deref(), Some("https://img/f.jpg"));
        assert_eq!(poster_from_id("plugin://x/?art=%7B%22poster%22%3A%22http%3A%2F%2Fa%22%7D"), None);
        assert_eq!(poster_from_id("tt123"), None);
        assert_eq!(decode_component("a+b%2"), "a b%2");
    }

    #[test]
    fn the_glow_parses() {
        assert_eq!(gradient(GLOW).len(), 1);
    }

    #[test]
    fn frames_by_shape_and_letters() {
        assert_eq!(Frame::of(1920, 1080), Frame::Wide);
        assert_eq!(Frame::of(500, 500), Frame::Square);
        assert_eq!(Frame::of(680, 1000), Frame::Poster);
        assert_eq!(letter("  éclair"), "É");
        assert_eq!(letter(" "), "P");
    }
}
