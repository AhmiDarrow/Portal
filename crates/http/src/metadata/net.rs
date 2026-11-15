//! Outgoing requests to the metadata sources. Failures are logged at debug
//! level without their URL, so a TMDB key in a query string never reaches the
//! log.

use std::time::Duration;

use reqwest::header::LOCATION;
use reqwest::{StatusCode, Url};
use serde_json::Value;

/// One request, connection included.
const REQUEST_LIMIT: Duration = Duration::from_secs(5);
/// The largest answer read from a source.
const BODY_CAP: usize = 4 << 20;

/// A source could not be asked, or answered with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed;

pub fn client() -> Option<reqwest::Client> {
    build(false)
}

/// For requests that carry an API key in a custom header. reqwest drops only
/// `Authorization` and cookies on a redirect to another host, so this client
/// stops there instead, and the answer is the redirect itself.
pub fn keyed_client() -> Option<reqwest::Client> {
    build(true)
}

fn build(same_host_only: bool) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!(
            "Portal/{} (+https://github.com/AhmiDarrow/Portal)",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(REQUEST_LIMIT)
        .timeout(REQUEST_LIMIT)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if follows(attempt.previous(), attempt.url(), same_host_only) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .ok()
}

/// Whether a redirect to `next` is followed, `previous` being the URLs asked
/// so far: at most five hops, HTTPS only, and with `same_host_only` never to
/// another host or port than the one first asked.
fn follows(previous: &[Url], next: &Url, same_host_only: bool) -> bool {
    if previous.len() >= 5 || next.scheme() != "https" {
        return false;
    }
    !same_host_only
        || previous.first().is_some_and(|first| {
            first.host_str() == next.host_str()
                && first.port_or_known_default() == next.port_or_known_default()
        })
}

/// Status and parsed body. A body that is not JSON reads as `None`.
pub async fn fetch(
    request: reqwest::RequestBuilder,
) -> Result<(StatusCode, Option<Value>), Failed> {
    let (status, body) = fetch_bytes(request, BODY_CAP).await?;
    Ok((status, serde_json::from_slice(&body).ok()))
}

/// Status and raw body, at most `cap` bytes; a larger body is `Failed`.
pub async fn fetch_bytes(
    request: reqwest::RequestBuilder,
    cap: usize,
) -> Result<(StatusCode, Vec<u8>), Failed> {
    read(send(request).await?, cap).await
}

/// As `fetch_bytes`, for a request sent with the keyed client. A redirect to
/// another host is asked again with `open`, which does not send the key.
pub async fn fetch_bytes_keyed(
    request: reqwest::RequestBuilder,
    open: &reqwest::Client,
    cap: usize,
) -> Result<(StatusCode, Vec<u8>), Failed> {
    let response = send(request).await?;
    match elsewhere(&response) {
        Some(next) => fetch_bytes(open.get(next), cap).await,
        None => read(response, cap).await,
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, Failed> {
    request.send().await.map_err(|err| {
        tracing::debug!(error = %err.without_url(), "Online source did not answer");
        Failed
    })
}

/// Where a redirect the keyed client did not follow points, over HTTPS.
fn elsewhere(response: &reqwest::Response) -> Option<Url> {
    if !response.status().is_redirection() {
        return None;
    }
    let location = response.headers().get(LOCATION)?.to_str().ok()?;
    let next = response.url().join(location).ok()?;
    (next.scheme() == "https").then_some(next)
}

async fn read(
    mut response: reqwest::Response,
    cap: usize,
) -> Result<(StatusCode, Vec<u8>), Failed> {
    let status = response.status();
    let host = response.url().host_str().unwrap_or("").to_string();
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        tracing::debug!(host, "Online answer was too large");
        return Err(Failed);
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > cap {
                    tracing::debug!(host, "Online answer was too large");
                    return Err(Failed);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(err) => {
                tracing::debug!(host, error = %err.without_url(), "Online answer broke off");
                return Err(Failed);
            }
        }
    }
    if !status.is_success() {
        tracing::debug!(host, status = status.as_u16(), "Online source refused");
    }
    Ok((status, body))
}

/// JSON from a 2xx answer, `None` for 404, and `Failed` for anything else.
pub async fn get_json(request: reqwest::RequestBuilder) -> Result<Option<Value>, Failed> {
    match fetch(request).await? {
        (status, Some(json)) if status.is_success() => Ok(Some(json)),
        (StatusCode::NOT_FOUND, _) => Ok(None),
        _ => Err(Failed),
    }
}

/// `base` with its query parameters encoded.
pub fn url(base: &str, params: &[(&str, &str)]) -> Result<Url, Failed> {
    Url::parse_with_params(base, params).map_err(|_| Failed)
}

/// `base` with one more path segment, encoded.
pub fn url_with_segment(base: &str, segment: &str) -> Result<Url, Failed> {
    let mut url = Url::parse(base).map_err(|_| Failed)?;
    url.path_segments_mut()
        .map_err(|_| Failed)?
        .pop_if_empty()
        .push(segment);
    Ok(url)
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/net.rs"]
mod tests;
