//! `GET /api/image/web?url=<encoded>`: a picture from another site, fetched here and handed
//! back from Portal's own origin, for pages that draw images on a canvas (the RemedyUI web
//! app) when that site sends no CORS headers.
//!
//! Only public `http(s)` addresses: every hop of a redirect is checked the way Kodi art is
//! (`portal_engine::public_web_host`, and every address the name resolves to must be
//! `public_ip`), and the connection goes to the address that was checked, so a name cannot
//! point somewhere else between the check and the fetch. Only `image/*` answers come back,
//! up to a size limit, within a time limit. Answers are kept in a small in-memory cache,
//! the least recently used going first. The route is under `/api/`, so pairing and the
//! same-site guard apply to it.

use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::Response;
use serde::Deserialize;

use crate::{fail, ApiError, AppState};

const NOT_ALLOWED: &str = "That picture's address is not on the public web.";
const NOT_FETCHED: &str = "That picture could not be fetched.";
const NOT_A_PICTURE: &str = "That address is not a picture.";
const TOO_LARGE: &str = "That picture is too large.";
/// What `/api/image` sends when the site says nothing about caching.
const DEFAULT_CACHE: &str = "private, max-age=3600";

/// Every bound the proxy works within. Each has a default, an environment variable for the
/// server, and a setter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest picture fetched (`PORTAL_IMAGE_PROXY_MAX_BYTES`, default 16 MiB).
    pub max_bytes: usize,
    /// Time for the whole fetch, redirects included (`PORTAL_IMAGE_PROXY_TIMEOUT_SECS`, 20 s).
    pub timeout: Duration,
    /// Redirects followed (`PORTAL_IMAGE_PROXY_REDIRECTS`, 5).
    pub redirects: usize,
    /// Pictures kept in memory (`PORTAL_IMAGE_PROXY_CACHE_ENTRIES`, 128).
    pub cache_entries: usize,
    /// Bytes kept in memory (`PORTAL_IMAGE_PROXY_CACHE_BYTES`, 32 MiB).
    pub cache_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 16 << 20,
            timeout: Duration::from_secs(20),
            redirects: 5,
            cache_entries: 128,
            cache_bytes: 32 << 20,
        }
    }
}

impl Limits {
    /// The defaults, with any the environment sets.
    pub fn from_env() -> Self {
        let read = |key: &str| std::env::var(key).ok().and_then(|v| v.trim().parse::<u64>().ok());
        let d = Limits::default();
        Limits {
            max_bytes: read("PORTAL_IMAGE_PROXY_MAX_BYTES").map_or(d.max_bytes, |v| v as usize),
            timeout: read("PORTAL_IMAGE_PROXY_TIMEOUT_SECS").map_or(d.timeout, Duration::from_secs),
            redirects: read("PORTAL_IMAGE_PROXY_REDIRECTS").map_or(d.redirects, |v| v as usize),
            cache_entries: read("PORTAL_IMAGE_PROXY_CACHE_ENTRIES").map_or(d.cache_entries, |v| v as usize),
            cache_bytes: read("PORTAL_IMAGE_PROXY_CACHE_BYTES").map_or(d.cache_bytes, |v| v as usize),
        }
    }

    pub fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_bytes = bytes;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn redirects(mut self, hops: usize) -> Self {
        self.redirects = hops;
        self
    }

    pub fn cache(mut self, entries: usize, bytes: usize) -> Self {
        self.cache_entries = entries;
        self.cache_bytes = bytes;
        self
    }
}

/// A fetched picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picture {
    pub bytes: Vec<u8>,
    pub content_type: String,
    /// The site's own `Cache-Control`, passed on.
    pub cache_control: Option<String>,
}

pub struct ImageProxy {
    limits: Limits,
    /// Which addresses count as public; tests widen it to reach a local server.
    public: fn(IpAddr) -> bool,
    cache: Mutex<VecDeque<(String, Picture)>>,
}

impl ImageProxy {
    pub fn new(limits: Limits) -> Self {
        ImageProxy { limits, public: portal_engine::public_ip, cache: Mutex::new(VecDeque::new()) }
    }

    #[cfg(test)]
    fn with_public(mut self, public: fn(IpAddr) -> bool) -> Self {
        self.public = public;
        self
    }

    /// The picture at `url`, from the cache or fetched.
    pub async fn get(&self, url: &str) -> Result<Picture, ApiError> {
        if let Some(hit) = self.cached(url) {
            return Ok(hit);
        }
        let picture = tokio::time::timeout(self.limits.timeout, self.fetch(url))
            .await
            .map_err(|_| fail(StatusCode::GATEWAY_TIMEOUT, NOT_FETCHED))??;
        self.store(url, &picture);
        Ok(picture)
    }

    async fn fetch(&self, url: &str) -> Result<Picture, ApiError> {
        let mut at = reqwest::Url::parse(url).map_err(|_| fail(StatusCode::BAD_REQUEST, NOT_ALLOWED))?;
        for _ in 0..=self.limits.redirects {
            let client = self.client_for(&at).await?;
            let response = client
                .get(at.clone())
                .header(header::ACCEPT, "image/*")
                .send()
                .await
                .map_err(|_| fail(StatusCode::BAD_GATEWAY, NOT_FETCHED))?;
            if response.status().is_redirection() {
                let next = response
                    .headers()
                    .get(header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| at.join(v).ok())
                    .ok_or_else(|| fail(StatusCode::BAD_GATEWAY, NOT_FETCHED))?;
                at = next;
                continue;
            }
            return self.read(response).await;
        }
        Err(fail(StatusCode::BAD_GATEWAY, NOT_FETCHED))
    }

    /// A client that may connect only to the checked, public address of `url`'s host.
    async fn client_for(&self, url: &reqwest::Url) -> Result<reqwest::Client, ApiError> {
        let refuse = || fail(StatusCode::BAD_REQUEST, NOT_ALLOWED);
        if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
            return Err(refuse());
        }
        let port = url.port_or_known_default().ok_or_else(refuse)?;
        let builder = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .timeout(self.limits.timeout);
        let host = url.host_str().ok_or_else(refuse)?;
        let builder = match host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
            Ok(ip) if (self.public)(ip) => builder,
            Ok(_) => return Err(refuse()),
            Err(_) if portal_engine::public_web_host(url.as_str()) => {
                let address = self.resolve(host, port).await.ok_or_else(refuse)?;
                builder.resolve(host, address)
            }
            Err(_) => return Err(refuse()),
        };
        builder.build().map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, NOT_FETCHED))
    }

    /// The address to connect to, when every address `name` resolves to is public.
    async fn resolve(&self, name: &str, port: u16) -> Option<SocketAddr> {
        let found: Vec<SocketAddr> = tokio::net::lookup_host((name, port)).await.ok()?.collect();
        (!found.is_empty() && found.iter().all(|a| (self.public)(a.ip()))).then(|| found[0])
    }

    async fn read(&self, mut response: reqwest::Response) -> Result<Picture, ApiError> {
        if !response.status().is_success() {
            return Err(fail(StatusCode::BAD_GATEWAY, NOT_FETCHED));
        }
        let headers = response.headers();
        let content_type = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim()
            .to_string();
        if !content_type.to_ascii_lowercase().starts_with("image/") {
            return Err(fail(StatusCode::UNSUPPORTED_MEDIA_TYPE, NOT_A_PICTURE));
        }
        let cache_control = headers.get(header::CACHE_CONTROL).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let limit = self.limits.max_bytes;
        if response.content_length().is_some_and(|n| n > limit as u64) {
            return Err(fail(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| fail(StatusCode::BAD_GATEWAY, NOT_FETCHED))? {
            if bytes.len() + chunk.len() > limit {
                return Err(fail(StatusCode::PAYLOAD_TOO_LARGE, TOO_LARGE));
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(Picture { bytes, content_type, cache_control })
    }

    fn cached(&self, url: &str) -> Option<Picture> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let index = cache.iter().position(|(key, _)| key == url)?;
        let entry = cache.remove(index)?;
        let picture = entry.1.clone();
        cache.push_back(entry);
        Some(picture)
    }

    fn store(&self, url: &str, picture: &Picture) {
        let (entries, bytes) = (self.limits.cache_entries, self.limits.cache_bytes);
        if entries == 0 || picture.bytes.len() > bytes {
            return;
        }
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|(key, _)| key != url);
        let mut total: usize = cache.iter().map(|(_, p)| p.bytes.len()).sum();
        while !cache.is_empty() && (cache.len() >= entries || total + picture.bytes.len() > bytes) {
            if let Some((_, old)) = cache.pop_front() {
                total -= old.bytes.len();
            }
        }
        cache.push_back((url.to_string(), picture.clone()));
    }
}

#[derive(Deserialize)]
pub struct WebImageQuery {
    url: String,
}

pub async fn image_web(State(state): State<AppState>, Query(query): Query<WebImageQuery>) -> Result<Response, ApiError> {
    let picture = state.0.image_proxy.get(&query.url).await?;
    Ok(picture_response(picture))
}

fn picture_response(picture: Picture) -> Response {
    let cache = picture
        .cache_control
        .as_deref()
        .and_then(|v| HeaderValue::from_str(v).ok())
        .unwrap_or(HeaderValue::from_static(DEFAULT_CACHE));
    let content_type =
        HeaderValue::from_str(&picture.content_type).unwrap_or(HeaderValue::from_static("application/octet-stream"));
    let mut response = Response::new(Body::from(picture.bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, content_type);
    headers.insert(header::CACHE_CONTROL, cache);
    // The browser must not guess another type, and an SVG opened on its own runs nothing.
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'none'; sandbox"));
    response
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/image_proxy.rs"]
mod tests;
