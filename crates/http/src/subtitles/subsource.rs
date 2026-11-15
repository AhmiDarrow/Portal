//! SubSource's API, only with the user's own free API key: Portal ships
//! none. A search finds the movie or the show's season by IMDb id (or by
//! name), then lists its subtitles one language at a time, at most two
//! languages per search since the API allows 60 requests a minute, and for
//! an episode up to three pages of a language until its rows appear. A
//! subtitle downloads as a zip. The key is sent in a header and never
//! logged.

use reqwest::header::ACCEPT;
use reqwest::StatusCode;
use serde_json::Value;

use super::rank::{Found, Source};
use super::{EpisodeNamed, SourceError};
use crate::metadata::net;
use crate::metadata::text::norm_title;

pub const API: &str = "https://api.subsource.net/api/v1";

/// The most languages one search lists; each is a request.
const LANGUAGE_LIMIT: usize = 2;
/// The rows asked for per page.
const PAGE_LIMIT: u64 = 50;
/// The most pages read per language, looking for an episode's rows.
const PAGES: u64 = 3;

pub const KEY_REFUSED: &str = "SubSource did not accept that API key.";
const SAVED_KEY_REFUSED: &str =
    "SubSource did not accept the saved API key. Check it under Server → Subtitles.";
const BUSY: &str = "SubSource is busy. Try again in a minute.";

/// Portal's language codes and the names SubSource lists by. Languages
/// without one are left out of a search.
const NAMES: &[(&str, &str)] = &[
    ("en", "english"),
    ("es", "spanish"),
    ("fr", "french"),
    ("de", "german"),
    ("it", "italian"),
    ("pt-PT", "portuguese"),
    ("pt-BR", "brazillian_portuguese"),
    ("nl", "dutch"),
    ("sv", "swedish"),
    ("da", "danish"),
    ("no", "norwegian"),
    ("fi", "finnish"),
    ("is", "icelandic"),
    ("ca", "catalan"),
    ("id", "indonesian"),
    ("ms", "malay"),
    ("pl", "polish"),
    ("cs", "czech"),
    ("sk", "slovak"),
    ("hu", "hungarian"),
    ("ro", "romanian"),
    ("hr", "croatian"),
    ("sl", "slovenian"),
    ("bs", "bosnian"),
    ("sq", "albanian"),
    ("sr", "serbian"),
    ("ru", "russian"),
    ("uk", "ukrainian"),
    ("bg", "bulgarian"),
    ("mk", "macedonian"),
    ("el", "greek"),
    ("tr", "turkish"),
    ("he", "hebrew"),
    ("ar", "arabic"),
    ("fa", "farsi_persian"),
    ("ur", "urdu"),
    ("et", "estonian"),
    ("lv", "latvian"),
    ("lt", "lithuanian"),
    ("vi", "vietnamese"),
    ("th", "thai"),
    ("hi", "hindi"),
    ("bn", "bengali"),
    ("ta", "tamil"),
    ("te", "telugu"),
    ("ml", "malayalam"),
    ("si", "sinhala"),
    ("ja", "japanese"),
    ("ko", "korean"),
];

/// Portal's code for a language as SubSource prints it ("english",
/// "Brazillian Portuguese").
fn language_of(theirs: &str) -> Option<&'static str> {
    let key = theirs.trim().to_ascii_lowercase().replace([' ', '-'], "_");
    NAMES
        .iter()
        .find(|(_, name)| *name == key)
        .map(|(code, _)| *code)
        .or_else(|| super::langs::find(&theirs.replace('_', " ")).map(|language| language.code))
}

/// The wanted languages SubSource has, at most `LANGUAGE_LIMIT`.
pub fn wanted(languages: &[String]) -> Vec<(&'static str, &'static str)> {
    let mut out: Vec<(&'static str, &'static str)> = Vec::new();
    for language in languages {
        let Some(&(code, name)) = NAMES.iter().find(|(code, _)| code == language) else {
            continue;
        };
        if !out.iter().any(|(kept, _)| *kept == code) {
            out.push((code, name));
        }
        if out.len() == LANGUAGE_LIMIT {
            break;
        }
    }
    out
}

/// A number, sent as one or as a string.
fn number(row: &Value, key: &str) -> Option<u64> {
    let value = row.get(key)?;
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

/// The request must come from the keyed client, which keeps the key to the
/// host it was sent to.
fn with_key(request: reqwest::RequestBuilder, key: &str) -> reqwest::RequestBuilder {
    request.header("X-API-Key", key)
}

/// What to search by.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub movie: bool,
    /// "tt0000001"; the show's id for an episode.
    pub imdb: Option<String>,
    /// The movie's or show's name, used only without an IMDb id.
    pub title: Option<String>,
    pub year: Option<u32>,
    pub episode: Option<(u32, u32)>,
}

/// The title lookup's query. `None` when there is nothing to search by.
pub fn title_params(search: &Search) -> Option<Vec<(&'static str, String)>> {
    match (&search.imdb, &search.title) {
        (Some(imdb), _) => {
            let mut out = vec![
                ("searchType", "imdb".to_string()),
                ("imdb", imdb.clone()),
                (
                    "type",
                    if search.movie { "movie" } else { "series" }.to_string(),
                ),
            ];
            if let Some((season, _)) = search.episode.filter(|_| !search.movie) {
                out.push(("season", season.to_string()));
            }
            Some(out)
        }
        (None, Some(title)) => Some(vec![
            ("searchType", "text".to_string()),
            ("q", title.clone()),
        ]),
        (None, None) => None,
    }
}

/// What a refused request means.
fn refusal(status: StatusCode, unauthorized: &str) -> SourceError {
    match status.as_u16() {
        401 | 403 => SourceError::Note(unauthorized.to_string()),
        429 => SourceError::Note(BUSY.to_string()),
        _ => SourceError::Failed,
    }
}

pub async fn search(
    client: &reqwest::Client,
    key: &str,
    search: &Search,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let wanted = wanted(languages);
    let Some(pairs) = title_params(search).filter(|_| !wanted.is_empty()) else {
        return Ok(Vec::new());
    };
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let url = net::url(&format!("{API}/movies/search"), &refs)?;
    let request = with_key(client.get(url), key).header(ACCEPT, "application/json");
    let titles = match net::fetch(request).await? {
        (status, Some(json)) if status.is_success() => json,
        (StatusCode::NOT_FOUND, _) => return Ok(Vec::new()),
        (status, _) => return Err(refusal(status, SAVED_KEY_REFUSED)),
    };
    let Some(movie_id) = pick_title(&titles, search) else {
        return Ok(Vec::new());
    };
    let movie_id = movie_id.to_string();
    let limit = PAGE_LIMIT.to_string();
    let mut results = Vec::new();
    for (code, name) in wanted {
        // A language that fails keeps the rows the others found.
        let language = async {
            let mut found = Vec::new();
            for page in 1..=PAGES {
                let page_text = page.to_string();
                let url = net::url(
                    &format!("{API}/subtitles"),
                    &[
                        ("movieId", movie_id.as_str()),
                        ("language", name),
                        ("limit", limit.as_str()),
                        ("page", page_text.as_str()),
                    ],
                )?;
                let request = with_key(client.get(url), key).header(ACCEPT, "application/json");
                let json = match net::fetch(request).await? {
                    (status, Some(json)) if status.is_success() => json,
                    (StatusCode::NOT_FOUND, _) => break,
                    (status, _) => return Err(refusal(status, SAVED_KEY_REFUSED)),
                };
                let rows = parse(&json, search, code);
                let named = rows.iter().any(|item| !item.episode_unsure);
                found.extend(rows);
                if !more_pages(&json, page) || search.movie || search.episode.is_none() || named {
                    break;
                }
            }
            Ok::<_, SourceError>(found)
        };
        results.push(language.await);
    }
    super::merge_languages(results)
}

/// The list goes on past `page`.
fn more_pages(json: &Value, page: u64) -> bool {
    let rows = json
        .get("data")
        .and_then(Value::as_array)
        .map_or(0, Vec::len) as u64;
    let total = json
        .get("pagination")
        .and_then(|pagination| number(pagination, "total"));
    match total {
        Some(total) => page * PAGE_LIMIT < total,
        None => rows >= PAGE_LIMIT,
    }
}

/// SubSource's id for the movie, or for the show's season: by IMDb id, or
/// by an exact name (and year, for a movie), never just the first hit.
pub fn pick_title(json: &Value, search: &Search) -> Option<u64> {
    let rows: Vec<&Value> = json
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .collect();
    let text = |row: &Value, key: &str| {
        row.get(key)
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string()
    };
    let season = search
        .episode
        .filter(|_| !search.movie)
        .map(|(season, _)| season);
    let fits = |row: &&Value| {
        let named = match (&search.imdb, &search.title) {
            (Some(imdb), _) => text(row, "imdbId").eq_ignore_ascii_case(imdb),
            (None, Some(title)) => {
                let year_ok = !search.movie
                    || search
                        .year
                        .zip(number(row, "releaseYear"))
                        .is_none_or(|(want, found)| u64::from(want) == found);
                !norm_title(title).is_empty()
                    && norm_title(&text(row, "title")) == norm_title(title)
                    && year_ok
            }
            (None, None) => false,
        };
        named && number(row, "movieId").is_some()
    };
    let named: Vec<&Value> = rows.into_iter().filter(fits).collect();
    let chosen = match season {
        // The season's own entry, else, when no entry names a season, the
        // show's; never another season's.
        Some(season) => named
            .iter()
            .find(|row| number(row, "season") == Some(u64::from(season)))
            .or_else(|| {
                let seasons = named.iter().any(|row| number(row, "season").is_some());
                named.first().filter(|_| !seasons)
            }),
        None => named.first(),
    };
    chosen.and_then(|row| number(row, "movieId"))
}

/// Rows with a file to download. Forced subtitles (foreign parts only) are
/// left out. For an episode, a row whose release names another episode is
/// left out, and one that names none ranks lower.
pub fn parse(json: &Value, search: &Search, asked: &str) -> Vec<Found> {
    let rows = json.get("data").and_then(Value::as_array);
    let want = search.episode.filter(|_| !search.movie);
    rows.into_iter()
        .flatten()
        .filter_map(|row| {
            let flag = |key: &str| row.get(key).and_then(Value::as_bool) == Some(true);
            if flag("foreignParts") {
                return None;
            }
            let id = number(row, "subtitleId")?;
            let language = row
                .get("language")
                .and_then(Value::as_str)
                .and_then(language_of)
                .unwrap_or(asked);
            let releases: Vec<String> = row
                .get("releaseInfo")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .collect();
            let mut unsure = false;
            if let Some(want) = want {
                match super::release_episode(&releases, want) {
                    EpisodeNamed::This => {}
                    EpisodeNamed::Other => return None,
                    EpisodeNamed::Nothing => unsure = true,
                }
            }
            Some(Found {
                id: format!("{}:{id}", Source::Subsource.prefix()),
                source: Source::Subsource,
                language: language.to_string(),
                release: releases.join("\n"),
                hearing_impaired: flag("hearingImpaired"),
                downloads: number(row, "downloads"),
                rating: None,
                hash_match: false,
                episode: want,
                episode_unsure: unsure,
                machine_translated: false,
            })
        })
        .collect()
}

/// A new key works: one title lookup by id.
pub async fn check_key(client: &reqwest::Client, key: &str) -> Result<(), SourceError> {
    let url = net::url(
        &format!("{API}/movies/search"),
        &[
            ("searchType", "imdb"),
            ("imdb", "tt0000001"),
            ("type", "movie"),
        ],
    )?;
    let request = with_key(client.get(url), key).header(ACCEPT, "application/json");
    match net::fetch(request).await? {
        (status, _) if status.is_success() => Ok(()),
        (status, _) => Err(refusal(status, KEY_REFUSED)),
    }
}

pub fn valid_id(id: &str) -> bool {
    (1..=12).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// The zip. Should the API hand it off to another host, that host is asked
/// with the open client, without the key.
pub async fn download(
    client: &reqwest::Client,
    open: &reqwest::Client,
    key: &str,
    id: &str,
) -> Result<Vec<u8>, SourceError> {
    if !valid_id(id) {
        return Err(SourceError::Failed);
    }
    let request = with_key(client.get(format!("{API}/subtitles/{id}/download")), key);
    let (status, body) = net::fetch_bytes_keyed(request, open, super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(refusal(status, SAVED_KEY_REFUSED))
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/subsource.rs"]
mod tests;
