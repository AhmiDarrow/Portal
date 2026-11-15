//! Gestdown: TV subtitles from Addic7ed through a public JSON API, no key.
//! A show is found by name, then chosen by its TheTVDB or TMDB id, or by an
//! exact name, never just the first hit ("Example Show Revisited" is not
//! "Example Show").

use reqwest::StatusCode;
use serde_json::Value;

use super::rank::{Found, Source};
use super::SourceError;
use crate::metadata::net;
use crate::metadata::text::norm_title;

pub const API: &str = "https://api.gestdown.info";

/// What identifies the show Portal wants.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShowHints {
    pub name: String,
    pub tvdb: Vec<u64>,
    pub tmdb: Vec<u64>,
}

/// The shows Gestdown lists for a name. `None` for no match.
pub async fn search_shows(client: &reqwest::Client, name: &str) -> Result<Value, SourceError> {
    let url = net::url_with_segment(&format!("{API}/shows/search"), name)?;
    match net::fetch(client.get(url)).await? {
        (status, Some(json)) if status.is_success() => Ok(json),
        (StatusCode::NOT_FOUND, _) => Ok(Value::Null),
        (status, _) => Err(refusal(status)),
    }
}

/// The show's Gestdown id: a TheTVDB id match, then a TMDB id match, then
/// the only show with exactly this name.
pub fn pick_show(shows: &Value, hints: &ShowHints) -> Option<String> {
    let list = shows.get("shows").and_then(Value::as_array)?;
    let id_of = |show: &Value| show.get("id").and_then(Value::as_str).map(str::to_string);
    let number = |show: &Value, key: &str| show.get(key).and_then(Value::as_u64);
    for (key, wanted) in [("tvDbId", &hints.tvdb), ("tmdbId", &hints.tmdb)] {
        if wanted.is_empty() {
            continue;
        }
        if let Some(show) = list
            .iter()
            .find(|show| number(show, key).is_some_and(|id| wanted.contains(&id)))
        {
            return id_of(show);
        }
    }
    let name = norm_title(&hints.name);
    if name.is_empty() {
        return None;
    }
    let mut exact = list.iter().filter(|show| {
        show.get("name")
            .and_then(Value::as_str)
            .is_some_and(|found| norm_title(found) == name)
    });
    match (exact.next(), exact.next()) {
        (Some(show), None) => id_of(show),
        _ => None,
    }
}

/// The finished subtitles for one episode in one language.
pub async fn episode(
    client: &reqwest::Client,
    show: &str,
    season: u32,
    number: u32,
    language: &str,
) -> Result<Vec<Found>, SourceError> {
    let mut url =
        reqwest::Url::parse(&format!("{API}/subtitles/get")).map_err(|_| SourceError::Failed)?;
    url.path_segments_mut()
        .map_err(|_| SourceError::Failed)?
        .push(show)
        .push(&season.to_string())
        .push(&number.to_string())
        .push(language);
    match net::fetch(client.get(url)).await? {
        (status, Some(json)) if status.is_success() => {
            Ok(parse_episode(&json, language, (season, number)))
        }
        (StatusCode::NOT_FOUND, _) => Ok(Vec::new()),
        (status, _) => Err(refusal(status)),
    }
}

fn refusal(status: StatusCode) -> SourceError {
    match status.as_u16() {
        429 => {
            SourceError::Note("Gestdown asked Portal to slow down. Try again in a minute.".into())
        }
        423 => SourceError::Note(
            "Gestdown is still fetching this show's subtitles. Try again in a minute.".into(),
        ),
        _ => SourceError::Failed,
    }
}

pub fn parse_episode(json: &Value, language: &str, episode: (u32, u32)) -> Vec<Found> {
    let rows = json.get("matchingSubtitles").and_then(Value::as_array);
    rows.into_iter()
        .flatten()
        .filter(|row| row.get("completed").and_then(Value::as_bool) == Some(true))
        .filter_map(|row| {
            let id = row.get("subtitleId").and_then(Value::as_str)?;
            if !valid_id(id) {
                return None;
            }
            let version = row
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let qualities: Vec<&str> = row
                .get("qualities")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let release = if qualities.is_empty() || version.is_empty() {
                version.to_string()
            } else {
                format!("{version} ({})", qualities.join(", "))
            };
            Some(Found {
                id: format!("{}:{id}", Source::Gestdown.prefix()),
                source: Source::Gestdown,
                language: language.to_string(),
                release,
                hearing_impaired: row.get("hearingImpaired").and_then(Value::as_bool) == Some(true),
                downloads: row.get("downloadCount").and_then(Value::as_u64),
                rating: None,
                hash_match: false,
                episode: Some(episode),
                episode_unsure: false,
                machine_translated: false,
            })
        })
        .collect()
}

/// A Gestdown subtitle id: a UUID.
pub fn valid_id(id: &str) -> bool {
    id.len() == 36
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

pub async fn download(client: &reqwest::Client, id: &str) -> Result<Vec<u8>, SourceError> {
    if !valid_id(id) {
        return Err(SourceError::Failed);
    }
    let url = format!("{API}/subtitles/download/{id}");
    let (status, body) = net::fetch_bytes(client.get(url), super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(refusal(status))
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/gestdown.rs"]
mod tests;
