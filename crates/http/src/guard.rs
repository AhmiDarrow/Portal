//! Requests from other websites.
//!
//! Portal has no login, so a web page elsewhere must not be able to use it.
//! Two checks run before every route:
//!
//! - The `Host` must be this PC by address or by a local name. A hostile page
//!   can point its own domain at 127.0.0.1 (DNS rebinding) and so become
//!   "same-origin" with Portal, but its requests still carry that domain as
//!   the Host. A request from this PC itself (which may change everything)
//!   is answered only on `localhost`, this PC's own addresses, the
//!   computer's own name (`mypc`, `mypc.local`, `mypc.<domain>`) and
//!   `PORTAL_ALLOWED_HOSTS`: anyone on the network can answer a lookup of
//!   another single-word or `.local` name. Other devices, which pair first
//!   and never get the PC's rights, may also use any local name.
//!   `PORTAL_ALLOWED_HOSTS` (names separated by commas) adds more, for
//!   Portal behind a proxy or on a name of your own.
//! - A change (anything but GET, HEAD or OPTIONS) sent by a browser must come
//!   from Portal's own pages: `Sec-Fetch-Site` same-origin or none, and an
//!   `Origin`, when there is one, naming the same host. The tray launcher and
//!   the cast helper send neither header.
//! - Some reads change things too: a browse runs the add-on and notes titles
//!   in the library, and a prompt check starts Kodi. A browser read of the
//!   API from another site is refused, except what a Cast TV loads from its
//!   own origin (subtitles; video is outside `/api`). A browser too old to
//!   send `Sec-Fetch-Site` must show the read is Portal's own by `Origin` or
//!   `Referer`; a client that is no browser at all sends none of them.

use std::net::{IpAddr, SocketAddr};
use std::sync::OnceLock;

use axum::extract::{ConnectInfo, Request};
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
const REFUSED_PC_HOST: &str = "On this PC, open Portal at http://localhost, by this PC's IP address or its computer name, or add this name to PORTAL_ALLOWED_HOSTS.";
const REFUSED_SITE: &str = "Portal does not accept changes from other websites.";
const REFUSED_READ: &str = "Portal does not answer other websites.";

pub(crate) async fn same_site(request: Request, next: Next) -> Response {
    let path = request.uri().path();
    // No address means the request did not come over the network listener.
    let pc = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_none_or(|ConnectInfo(peer)| crate::pairing::this_pc(*peer, request.headers()));
    if let Err(detail) = check(request.method(), path, request.headers(), extra_hosts(), pc) {
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
        // This PC's own names: bare, mDNS, and on a work domain.
        let computer = std::env::var("COMPUTERNAME")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let domain = std::env::var("USERDNSDOMAIN")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !computer.is_empty() {
            names.push(format!("{computer}.local"));
            if !domain.is_empty() {
                names.push(format!("{computer}.{domain}"));
            }
            names.push(computer);
        }
        names
    })
}

fn check(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    extra: &[String],
    pc: bool,
) -> Result<(), &'static str> {
    let host = headers
        .get(header::HOST)
        .map(|value| value.to_str().unwrap_or("\u{0}"));
    // HTTP/1.0 clients may leave Host out; a browser never does.
    if let Some(host) = host {
        if pc && !pc_host(host, extra) {
            return Err(REFUSED_PC_HOST);
        }
        if !local_host(host, extra) {
            return Err(REFUSED_HOST);
        }
    }
    let site = headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok());
    let foreign = site.is_some_and(|site| site != "same-origin" && site != "none");
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        if open_read(path) {
            return Ok(());
        }
        if foreign || (site.is_none() && !own_read(headers, host)) {
            return Err(REFUSED_READ);
        }
        return Ok(());
    }
    if foreign {
        return Err(REFUSED_SITE);
    }
    if let Some(origin) = headers.get(header::ORIGIN) {
        if !names_host(origin.to_str().unwrap_or(""), host) {
            return Err(REFUSED_SITE);
        }
    }
    Ok(())
}

/// `url` (an Origin or a Referer) is on `host`, port included.
fn names_host(url: &str, host: Option<&str>) -> bool {
    let authority = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .map(|rest| rest.split(['/', '?', '#']).next().unwrap_or(""));
    matches!((authority, host), (Some(named), Some(host)) if named.eq_ignore_ascii_case(host))
}

/// An API read without `Sec-Fetch-Site` (older browsers and TVs) must name
/// Portal's own page in `Origin` or `Referer`. One with neither is only
/// taken from a client that is no browser (the tray launcher, scripts): a
/// page can drop its Referer but cannot change the browser's User-Agent.
fn own_read(headers: &HeaderMap, host: Option<&str>) -> bool {
    let text = |name: header::HeaderName| {
        headers
            .get(name)
            .map(|value| value.to_str().unwrap_or(""))
    };
    if let Some(origin) = text(header::ORIGIN) {
        return names_host(origin, host);
    }
    if let Some(referer) = text(header::REFERER) {
        return names_host(referer, host);
    }
    !text(header::USER_AGENT).is_some_and(|agent| agent.starts_with("Mozilla/"))
}

/// A Host a request from this PC may carry: `localhost`, one of this PC's
/// own addresses, or a name in `extra` (the computer's own names and
/// `PORTAL_ALLOWED_HOSTS`), never a name anyone on the network could answer.
fn pc_host(host: &str, extra: &[String]) -> bool {
    let host = host.trim();
    let name = match host.strip_prefix('[') {
        Some(inside) => inside.split_once(']').map_or("", |(address, _)| address),
        None => match host.rsplit_once(':') {
            Some((name, port)) if port.bytes().all(|byte| byte.is_ascii_digit()) => name,
            _ => host,
        },
    };
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if let Ok(ip) = name.parse::<IpAddr>() {
        return crate::from_this_pc(SocketAddr::new(ip, 0));
    }
    // Browsers answer every `*.localhost` with loopback themselves.
    name == "localhost" || name.ends_with(".localhost") || extra.contains(&name)
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
