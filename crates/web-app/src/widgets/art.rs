//! Where a picture is fetched from, as api.ts's `artUrl` decides it.

use crate::api::encode;

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
}
