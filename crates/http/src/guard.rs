//! Requests from other websites.
//!
//! Portal has no login, so a web page elsewhere must not be able to use it.
//! Two checks run before every route:
//!
//! - The `Host` must be this PC by address or by a local name. A hostile page
//!   can point its own domain at 127.0.0.1 (DNS rebinding) and so become
//!   "same-origin" with Portal, but its requests still carry that domain as
//!   the Host. `PORTAL_ALLOWED_HOSTS` (names separated by commas) adds more,
//!   for Portal behind a proxy or on a name of your own.
//! - A change (anything but GET, HEAD or OPTIONS) sent by a browser must come
//!   from Portal's own pages: `Sec-Fetch-Site` same-origin or none, and an
//!   `Origin`, when there is one, naming the same host. The tray launcher and
//!   the cast helper send neither header.
//! - Some reads change things too: a browse runs the add-on and notes titles
//!   in the library, and a prompt check starts Kodi. A browser read of the
//!   API from another site is refused, except what a Cast TV loads from its
//!   own origin (subtitles; video is outside `/api`).

use std::net::IpAddr;
use std::sync::OnceLock;

use axum::extract::Request;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

/// Local name endings a page on the internet cannot point at this PC,
/// including the names home routers hand out.
const LOCAL_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".lan",
    ".home",
    ".home.arpa",
    ".internal",
    ".intranet",
    ".localdomain",
    ".fritz.box",
    ".router",
    ".gateway",
    ".ts.net",
];

const REFUSED_HOST: &str = "Portal only answers requests addressed to this PC. Open it by its IP address, or add this name to PORTAL_ALLOWED_HOSTS.";
const REFUSED_SITE: &str = "Portal does not accept changes from other websites.";
const REFUSED_READ: &str = "Portal does not answer other websites.";

pub(crate) async fn same_site(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if let Err(detail) = check(request.method(), path, request.headers(), extra_hosts()) {
        return crate::fail(StatusCode::FORBIDDEN, detail).into_response();
    }
    let mut response = next.run(request).await;
    // Pages are open to other sites' reads, so another site must not put the
    // dashboard in a frame and trick a click on this PC's controls.
    let headers = response.headers_mut();
    headers.insert(
        axum::http::header::X_FRAME_OPTIONS,
        axum::http::HeaderValue::from_static("DENY"),
    );
    headers.insert(
        axum::http::header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
}

fn extra_hosts() -> &'static [String] {
    static HOSTS: OnceLock<Vec<String>> = OnceLock::new();
    HOSTS.get_or_init(|| {
        let mut names: Vec<String> = std::env::var("PORTAL_ALLOWED_HOSTS")
            .unwrap_or_default()
            .split(',')
            .map(|name| name.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        // This PC's own full name, e.g. on a work domain.
        let computer = std::env::var("COMPUTERNAME")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let domain = std::env::var("USERDNSDOMAIN")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !computer.is_empty() && !domain.is_empty() {
            names.push(format!("{computer}.{domain}"));
        }
        names
    })
}

fn check(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    extra: &[String],
) -> Result<(), &'static str> {
    let host = headers
        .get(header::HOST)
        .map(|value| value.to_str().unwrap_or("\u{0}"));
    // HTTP/1.0 clients may leave Host out; a browser never does.
    if let Some(host) = host {
        if !local_host(host, extra) {
            return Err(REFUSED_HOST);
        }
    }
    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    let foreign = site.is_some_and(|site| site != "same-origin" && site != "none");
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        if foreign && !open_read(path) {
            return Err(REFUSED_READ);
        }
        return Ok(());
    }
    if foreign {
        return Err(REFUSED_SITE);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        let origin = origin.to_str().unwrap_or("");
        let named = origin
            .strip_prefix("http://")
            .or_else(|| origin.strip_prefix("https://"));
        let same =
            matches!((named, host), (Some(named), Some(host)) if named.eq_ignore_ascii_case(host));
        if !same {
            return Err(REFUSED_SITE);
        }
    }
    Ok(())
}

/// A read another site's page may make: the dashboard's files, the video,
/// and the poster and subtitles a Cast TV loads.
fn open_read(path: &str) -> bool {
    !path.starts_with("/api/") || path.starts_with("/api/subtitles/file/")
}

/// `host` (with or without a port) is an IP address, `localhost`, a
/// single-label LAN name, a local suffix, or one of `extra`.
fn local_host(host: &str, extra: &[String]) -> bool {
    let host = host.trim();
    if let Some(inside) = host.strip_prefix('[') {
        let Some((address, rest)) = inside.split_once(']') else {
            return false;
        };
        return address.parse::<IpAddr>().is_ok() && port_only(rest);
    }
    let name = match host.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
        _ => host,
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return false;
    }
    name.parse::<IpAddr>().is_ok()
        || name == "localhost"
        || !name.contains('.')
        || LOCAL_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        || extra.contains(&name)
}

fn port_only(rest: &str) -> bool {
    rest.is_empty()
        || rest
            .strip_prefix(':')
            .is_some_and(|port| !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()))
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/guard.rs"]
mod tests;
