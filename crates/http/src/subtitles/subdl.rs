//! SubDL's API, only with the user's own free API key: Portal ships none.
//! A search names the movie or show by IMDb id (or by name), the episode
//! and the languages; a subtitle downloads as a zip from SubDL's download
//! host. The key goes in the query string, so failures are logged without
//! their URL and the key is never logged.

use reqwest::StatusCode;
use serde_json::Value;

use super::rank::{Found, Source};
use super::SourceError;
use crate::metadata::net;

pub const API: &str = "https://api.subdl.com/api/v1/subtitles";
pub const DOWNLOAD: &str = "https://dl.subdl.com";

/// The rows asked for.
const PAGE_SIZE: &str = "30";
/// The most release names kept for one subtitle.
const RELEASE_LIMIT: usize = 8;

pub const KEY_REFUSED: &str = "SubDL did not accept that API key.";
const SAVED_KEY_REFUSED: &str =
    "SubDL did not accept the saved API key. Check it under Server → Subtitles.";
const BUSY: &str = "SubDL asked Portal to slow down. Try again in a minute.";

/// Portal's language codes and SubDL's. Languages SubDL has no code for
/// are left out of a search.
const CODES: &[(&str, &str)] = &[
    ("en", "EN"),
    ("es", "ES"),
    ("fr", "FR"),
    ("de", "DE"),
    ("it", "IT"),
    ("pt-PT", "PT"),
    ("pt-BR", "BR_PT"),
    ("nl", "NL"),
    ("sv", "SV"),
    ("da", "DA"),
    ("no", "NO"),
    ("fi", "FI"),
    ("is", "IS"),
    ("ca", "CA"),
    ("id", "ID"),
    ("ms", "MS"),
    ("pl", "PL"),
    ("cs", "CS"),
    ("sk", "SK"),
    ("hu", "HU"),
    ("ro", "RO"),
    ("hr", "HR"),
    ("sl", "SL"),
    ("bs", "BS"),
    ("sq", "SQ"),
    ("sr", "SR"),
    ("ru", "RU"),
    ("uk", "UK"),
    ("bg", "BG"),
    ("mk", "MK"),
    ("el", "EL"),
    ("tr", "TR"),
    ("he", "HE"),
    ("ar", "AR"),
    ("fa", "FA"),
    ("ur", "UR"),
    ("et", "ET"),
    ("lv", "LV"),
    ("lt", "LT"),
    ("vi", "VI"),
    ("th", "TH"),
    ("hi", "HI"),
    ("bn", "BN"),
    ("ta", "TA"),
    ("te", "TE"),
    ("ml", "ML"),
    ("si", "SI"),
    ("zh-CN", "ZH"),
    ("zh-TW", "ZH_BG"),
    ("ja", "JA"),
    ("ko", "KO"),
];

/// SubDL's code for one of Portal's languages.
pub fn code_for(language: &str) -> Option<&'static str> {
    CODES
        .iter()
        .find(|(code, _)| *code == language)
        .map(|(_, theirs)| *theirs)
}

/// Portal's code for a language SubDL names by code ("EN", "BR_PT") or,
/// failing that, by name ("English").
fn language_of(theirs: &str, name: &str) -> Option<&'static str> {
    CODES
        .iter()
        .find(|(_, code)| code.eq_ignore_ascii_case(theirs.trim()))
        .map(|(code, _)| *code)
        .or_else(|| super::langs::find(name).map(|language| language.code))
}

/// What to search by.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub movie: bool,
    /// "tt0000001"; the show's id for an episode.
    pub imdb: Option<String>,
    /// The movie's or show's name, used only without an IMDb id.
    pub name: Option<String>,
    pub episode: Option<(u32, u32)>,
}

/// The query, without the key. `None` when there is nothing to search by
/// or no language SubDL knows.
pub fn params(search: &Search, languages: &[String]) -> Option<Vec<(&'static str, String)>> {
    let mut codes: Vec<&str> = Vec::new();
    for code in languages.iter().filter_map(|code| code_for(code)) {
        if !codes.contains(&code) {
            codes.push(code);
        }
    }
    if codes.is_empty() {
        return None;
    }
    let mut out: Vec<(&'static str, String)> = vec![(
        "type",
        if search.movie { "movie" } else { "tv" }.to_string(),
    )];
    match (&search.imdb, &search.name) {
        (Some(imdb), _) => out.push(("imdb_id", imdb.clone())),
        (None, Some(name)) => out.push(("film_name", name.clone())),
        (None, None) => return None,
    }
    if let Some((season, number)) = search.episode.filter(|_| !search.movie) {
        out.push(("season_number", season.to_string()));
        out.push(("episode_number", number.to_string()));
    }
    out.push(("languages", codes.join(",")));
    out.push(("subs_per_page", PAGE_SIZE.to_string()));
    out.push(("releases", "1".to_string()));
    Some(out)
}

pub async fn search(
    client: &reqwest::Client,
    key: &str,
    search: &Search,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let Some(pairs) = params(search, languages) else {
        return Ok(Vec::new());
    };
    let mut refs: Vec<(&str, &str)> = vec![("api_key", key)];
    refs.extend(pairs.iter().map(|(name, value)| (*name, value.as_str())));
    let url = net::url(API, &refs)?;
    let (status, json) = net::fetch(client.get(url)).await?;
    answer(status, json.as_ref(), search)
}

/// What SubDL's answer means. A refused key comes back as 401 or 403, or
/// as `"status": false` with an error about the key; any other error (such
/// as nothing found) is an empty list.
pub fn answer(
    status: StatusCode,
    json: Option<&Value>,
    search: &Search,
) -> Result<Vec<Found>, SourceError> {
    match (status.as_u16(), json) {
        (401 | 403, _) => Err(SourceError::Note(SAVED_KEY_REFUSED.to_string())),
        (429, _) => Err(SourceError::Note(BUSY.to_string())),
        (404, _) => Ok(Vec::new()),
        (_, Some(json)) if status.is_success() => {
            if refused(json) {
                return Err(SourceError::Note(SAVED_KEY_REFUSED.to_string()));
            }
            Ok(parse(json, search))
        }
        _ => Err(SourceError::Failed),
    }
}

/// `"status": false` with a reason about the key. A bare `false` is read as
/// nothing found: saying a working key was refused would be worse than
/// missing a refusal, which the 401/403 answers still catch.
fn refused(json: &Value) -> bool {
    if json.get("status").and_then(Value::as_bool) != Some(false) {
        return false;
    }
    let Some(error) = json.get("error").and_then(Value::as_str) else {
        return false;
    };
    let error = error.to_ascii_lowercase();
    ["key", "token", "unauthori", "forbidden"]
        .iter()
        .any(|word| error.contains(word))
}

/// "/subtitle/1234-5678.zip" as an id: "1234-5678". Anything else is
/// refused, so a download can only ever reach SubDL's own files.
pub fn path_id(url: &str) -> Option<&str> {
    let id = url.strip_prefix("/subtitle/")?.strip_suffix(".zip")?;
    valid_id(id).then_some(id)
}

pub fn valid_id(id: &str) -> bool {
    (1..=100).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Rows with a file to download. For an episode, rows for another season
/// or episode are left out; a whole-season zip stays, since the episode's
/// file is picked out of its zip.
pub fn parse(json: &Value, search: &Search) -> Vec<Found> {
    let rows = json.get("subtitles").and_then(Value::as_array);
    let want = search.episode.filter(|_| !search.movie);
    rows.into_iter()
        .flatten()
        .filter_map(|row| {
            let text = |key: &str| row.get(key).and_then(Value::as_str).unwrap_or("").trim();
            let id = path_id(text("url"))?;
            let language = language_of(text("language"), text("lang"))?;
            let number = |key: &str| row.get(key).and_then(Value::as_u64).map(|n| n as u32);
            let pack = row.get("full_season").and_then(Value::as_bool) == Some(true);
            let mut unsure = false;
            if let Some((season, episode)) = want {
                if number("season").is_some_and(|found| found != season) {
                    return None;
                }
                // Episode 0 reads as not given.
                let found = number("episode").filter(|found| *found > 0);
                if !pack && found.is_some_and(|found| found != episode) {
                    return None;
                }
                let named = super::release_episode(&releases(row), (season, episode));
                if !pack && named == super::EpisodeNamed::Other {
                    return None;
                }
                unsure = !pack && found.is_none() && named == super::EpisodeNamed::Nothing;
            }
            Some(Found {
                id: format!("{}:{id}", Source::Subdl.prefix()),
                source: Source::Subdl,
                language: language.to_string(),
                release: releases(row).join("\n"),
                hearing_impaired: row.get("hi").and_then(Value::as_bool) == Some(true),
                downloads: None,
                rating: None,
                hash_match: false,
                episode: want,
                episode_unsure: unsure,
                machine_translated: false,
            })
        })
        .collect()
}

/// The release name first, then the other releases it fits.
fn releases(row: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let first = row.get("release_name").and_then(Value::as_str);
    let others = row
        .get("releases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    for name in first.into_iter().chain(others) {
        let name = name.trim();
        if !name.is_empty() && !out.iter().any(|kept| kept == name) && out.len() < RELEASE_LIMIT {
            out.push(name.to_string());
        }
    }
    out
}

/// A new key works: one small search by id.
pub async fn check_key(client: &reqwest::Client, key: &str) -> Result<(), SourceError> {
    let url = net::url(
        API,
        &[
            ("api_key", key),
            ("type", "movie"),
            ("imdb_id", "tt0000001"),
            ("languages", "EN"),
            ("subs_per_page", "1"),
        ],
    )?;
    match net::fetch(client.get(url)).await? {
        (status, Some(json)) if status.is_success() && refused(&json) => {
            Err(SourceError::Note(KEY_REFUSED.to_string()))
        }
        (status, Some(_)) if status.is_success() => Ok(()),
        (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN, _) => {
            Err(SourceError::Note(KEY_REFUSED.to_string()))
        }
        (StatusCode::TOO_MANY_REQUESTS, _) => Err(SourceError::Note(BUSY.to_string())),
        _ => Err(SourceError::Failed),
    }
}

/// The zip. SubDL's download host needs no key.
pub async fn download(client: &reqwest::Client, id: &str) -> Result<Vec<u8>, SourceError> {
    if !valid_id(id) {
        return Err(SourceError::Failed);
    }
    let request = client.get(format!("{DOWNLOAD}/subtitle/{id}.zip"));
    let (status, body) = net::fetch_bytes(request, super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(SourceError::Failed)
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/subdl.rs"]
mod tests;
