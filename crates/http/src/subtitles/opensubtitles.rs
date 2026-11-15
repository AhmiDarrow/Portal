//! OpenSubtitles.com's REST API, only with the user's own API key: Portal
//! ships none. Downloads count against the key's daily quota, which a
//! signed-in account raises. The key, password and session token are never
//! logged; failures are logged without their URL.

use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use reqwest::StatusCode;
use serde_json::{json, Value};

use super::rank::{Found, Source};
use super::SourceError;
use crate::metadata::net;

pub const API: &str = "https://api.opensubtitles.com/api/v1";

/// The rows asked for; more are rarely useful.
const PAGE_LIMIT: usize = 60;

pub const QUOTA: &str = "OpenSubtitles says the daily download limit for your key is used up. Sign in under Server → Subtitles for a higher limit, or pick a subtitle from another source.";
pub const KEY_REFUSED: &str = "OpenSubtitles did not accept that API key.";
pub const LOGIN_REFUSED: &str = "OpenSubtitles did not accept that username and password.";
const BUSY: &str = "OpenSubtitles asked Portal to slow down. Try again in a minute.";
const SAVED_KEY_REFUSED: &str =
    "OpenSubtitles did not accept the saved API key. Check it under Server → Subtitles.";

/// The header OpenSubtitles asks every app to send: its name and version.
fn app_agent() -> String {
    format!("Portal v{}", env!("CARGO_PKG_VERSION"))
}

/// The request must come from the keyed client, which keeps the key to the
/// host it was sent to.
fn with_key(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    request
        .header("Api-Key", key)
        .header(USER_AGENT, app_agent())
        .header(ACCEPT, "application/json")
}

/// What to search by, as OpenSubtitles names it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub movie: bool,
    /// Without "tt" or leading zeros.
    pub imdb: Option<String>,
    pub parent_imdb: Option<String>,
    pub tmdb: Option<String>,
    pub query: Option<String>,
    pub year: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

/// "tt0012345" as OpenSubtitles wants it: "12345". Leading zeros make
/// the API answer with a redirect.
pub fn imdb_digits(id: &str) -> Option<String> {
    let id = portal_api::imdb_id(id)?;
    let digits = id.trim_start_matches("tt").trim_start_matches('0');
    (!digits.is_empty()).then(|| digits.to_string())
}

/// Parameters in alphabetical order, lowercase, as the API recommends so it
/// does not redirect.
pub fn params(search: &Search, languages: &[String]) -> Vec<(&'static str, String)> {
    let mut languages: Vec<String> = languages
        .iter()
        .map(|code| code.to_ascii_lowercase())
        .collect();
    languages.sort();
    let mut out: Vec<(&'static str, String)> = Vec::new();
    if let Some(episode) = search.episode.filter(|_| !search.movie) {
        out.push(("episode_number", episode.to_string()));
    }
    if let Some(imdb) = &search.imdb {
        out.push(("imdb_id", imdb.clone()));
    }
    out.push(("languages", languages.join(",")));
    if let Some(parent) = search.parent_imdb.as_ref().filter(|_| !search.movie) {
        out.push(("parent_imdb_id", parent.clone()));
    }
    let by_id = search.imdb.is_some() || search.parent_imdb.is_some() || search.tmdb.is_some();
    if let Some(query) = search.query.as_ref().filter(|_| !by_id) {
        out.push(("query", query.to_lowercase()));
    }
    if let Some(season) = search.season.filter(|_| !search.movie) {
        out.push(("season_number", season.to_string()));
    }
    if let Some(tmdb) = search.tmdb.as_ref().filter(|_| search.imdb.is_none()) {
        out.push(("tmdb_id", tmdb.clone()));
    }
    out.push((
        "type",
        if search.movie { "movie" } else { "episode" }.to_string(),
    ));
    if let Some(year) = search.year.filter(|_| search.movie && !by_id) {
        out.push(("year", year.to_string()));
    }
    out
}

pub async fn search(
    client: &reqwest::Client,
    key: &str,
    search: &Search,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let pairs = params(search, languages);
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let url = net::url(&format!("{API}/subtitles"), &refs)?;
    match net::fetch(with_key(client.get(url), key)).await? {
        (status, Some(json)) if status.is_success() => Ok(parse(&json, search)),
        (status, _) => Err(refusal(status, SAVED_KEY_REFUSED)),
    }
}

/// What a refused request means. OpenSubtitles answers 403 ("You cannot
/// consume this service") for a key it does not accept, and 401 for a
/// sign-in it does not accept.
fn refusal(status: StatusCode, unauthorized: &str) -> SourceError {
    match status.as_u16() {
        401 => SourceError::Note(unauthorized.to_string()),
        403 if unauthorized == LOGIN_REFUSED => SourceError::Note(SAVED_KEY_REFUSED.to_string()),
        403 => SourceError::Note(unauthorized.to_string()),
        406 => SourceError::Note(QUOTA.to_string()),
        429 => SourceError::Note(BUSY.to_string()),
        _ => SourceError::Failed,
    }
}

/// Rows with a file to download, in the asked-for episode. Machine and AI
/// translations are left out.
pub fn parse(json: &Value, search: &Search) -> Vec<Found> {
    let rows = json.get("data").and_then(Value::as_array);
    rows.into_iter()
        .flatten()
        .take(PAGE_LIMIT)
        .filter_map(|row| {
            let attributes = row.get("attributes")?;
            let flag = |key: &str| attributes.get(key).and_then(Value::as_bool) == Some(true);
            if flag("machine_translated") || flag("ai_translated") {
                return None;
            }
            let file_id = attributes
                .get("files")
                .and_then(Value::as_array)
                .and_then(|files| files.first())
                .and_then(|file| file.get("file_id"))
                .and_then(Value::as_u64)?;
            let language = attributes.get("language").and_then(Value::as_str)?;
            let language = super::langs::find(language)?.code.to_string();
            let details = attributes.get("feature_details").unwrap_or(&Value::Null);
            let number = |key: &str| details.get(key).and_then(Value::as_u64).map(|n| n as u32);
            let episode = match (number("season_number"), number("episode_number")) {
                (Some(season), Some(episode)) => Some((season, episode)),
                _ => None,
            };
            if !search.movie {
                if let (Some(want), Some(found)) = (search.season.zip(search.episode), episode) {
                    if want != found {
                        return None;
                    }
                }
            }
            Some(Found {
                id: format!("{}:{file_id}", Source::OpenSubtitles.prefix()),
                source: Source::OpenSubtitles,
                language,
                release: attributes
                    .get("release")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                hearing_impaired: flag("hearing_impaired"),
                downloads: attributes.get("download_count").and_then(Value::as_u64),
                rating: attributes
                    .get("ratings")
                    .and_then(Value::as_f64)
                    .filter(|r| *r > 0.0),
                hash_match: flag("moviehash_match"),
                episode: episode.or(search.season.zip(search.episode)),
                episode_unsure: false,
                machine_translated: false,
            })
        })
        .collect()
}

/// A new key works: one light request that needs a valid key. (The info
/// endpoints and search answer without one.)
pub async fn check_key(client: &reqwest::Client, key: &str) -> Result<(), SourceError> {
    let url = net::url(
        &format!("{API}/discover/popular"),
        &[("language", "en"), ("type", "movie")],
    )?;
    let request = with_key(client.get(url), key);
    match net::fetch(request).await? {
        (status, _) if status.is_success() => Ok(()),
        (status, _) => Err(refusal(status, KEY_REFUSED)),
    }
}

/// A signed-in session: its token and the API host to download from.
#[derive(Clone)]
pub struct Session {
    pub token: String,
    pub base: String,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session").field("base", &self.base).finish()
    }
}

pub async fn login(
    client: &reqwest::Client,
    key: &str,
    username: &str,
    password: &str,
) -> Result<Session, SourceError> {
    let body = json!({"username": username, "password": password});
    let request = with_key(client.post(format!("{API}/login")), key)
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string());
    match net::fetch(request).await? {
        (status, Some(json)) if status.is_success() => {
            let token = json.get("token").and_then(Value::as_str).unwrap_or("");
            if token.is_empty() {
                return Err(SourceError::Failed);
            }
            Ok(Session {
                token: token.to_string(),
                base: json
                    .get("base_url")
                    .and_then(Value::as_str)
                    .and_then(api_base)
                    .unwrap_or_else(|| API.to_string()),
            })
        }
        (status, _) => Err(refusal(status, LOGIN_REFUSED)),
    }
}

/// "vip-api.opensubtitles.com" as an API root, only on opensubtitles.com.
fn api_base(host: &str) -> Option<String> {
    let host = host
        .trim()
        .trim_start_matches("https://")
        .trim_end_matches('/');
    let host = host.split('/').next()?;
    let ok = host == "opensubtitles.com" || host.ends_with(".opensubtitles.com");
    let clean = host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-');
    (ok && clean).then(|| format!("https://{host}/api/v1"))
}

pub fn valid_id(id: &str) -> bool {
    (1..=12).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// The file: a download link first (this is what counts against the
/// quota), asked with the keyed client, then the file itself from wherever
/// the link points, with the open one.
pub async fn download(
    client: &reqwest::Client,
    open: &reqwest::Client,
    key: &str,
    session: Option<&Session>,
    file_id: &str,
) -> Result<Vec<u8>, SourceError> {
    let Ok(number) = file_id.parse::<u64>() else {
        return Err(SourceError::Failed);
    };
    let base = session.map_or(API, |session| session.base.as_str());
    let mut request = with_key(client.post(format!("{base}/download")), key)
        .header(CONTENT_TYPE, "application/json")
        .body(json!({"file_id": number}).to_string());
    if let Some(session) = session {
        request = request.header(AUTHORIZATION, format!("Bearer {}", session.token));
    }
    let link = match net::fetch(request).await? {
        (status, Some(json)) if status.is_success() => json
            .get("link")
            .and_then(Value::as_str)
            .filter(|link| link.starts_with("https://"))
            .map(str::to_string)
            .ok_or(SourceError::Failed)?,
        (status, _) => return Err(refusal(status, SAVED_KEY_REFUSED)),
    };
    let request = open.get(link).header(USER_AGENT, app_agent());
    let (status, body) = net::fetch_bytes(request, super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(SourceError::Failed)
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/opensubtitles.rs"]
mod tests;
