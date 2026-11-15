//! Video info labels an add-on sets on its items, as `MediaInfo`.
//!
//! Kodi fills every requested label, so an item without info still comes back
//! with empty strings, zero years and ratings, -1 seasons, and empty arrays.
//! Those count as absent.

use crate::catalog::{plain_block, plain_label};
use portal_api::{CastMember, InfoLink, MediaInfo, MediaKind, CAST_LIMIT};
use serde_json::Value;

/// Files.GetDirectory properties for the info labels. Every name is in
/// `List.Fields.Files` for JSON-RPC v12 (Kodi 19) through v13.5 (Kodi 21).
/// An unknown name fails the whole call, so keep this list to those names.
pub const INFO_PROPERTIES: &[&str] = &[
    "plot",
    "plotoutline",
    "tagline",
    "year",
    "premiered",
    "rating",
    "votes",
    "genre",
    "mpaa",
    "runtime",
    "studio",
    "director",
    "writer",
    "cast",
    "imdbnumber",
    "uniqueid",
    "originaltitle",
    "trailer",
    "showtitle",
    "season",
    "episode",
    "country",
    "duration",
];

pub const SOURCE_KODI: &str = "Kodi add-on";
const PLOT_LIMIT: usize = 4000;

/// `None` unless the add-on set at least one descriptive label or id.
pub fn media_info(item: &Value) -> Option<MediaInfo> {
    if !item.is_object() {
        return None;
    }
    let art = item.get("art");
    let kind = media_kind(item);
    let title = text(item, "title");
    let original_title = text(item, "originaltitle").filter(|original| {
        title
            .as_deref()
            .is_none_or(|title| !title.eq_ignore_ascii_case(original))
    });
    let premiered = item
        .get("premiered")
        .and_then(Value::as_str)
        .and_then(date_prefix);
    let year = item
        .get("year")
        .and_then(Value::as_i64)
        .and_then(plausible_year)
        .or_else(|| premiered.as_deref().and_then(year_of_date));
    // Menu folders often repeat their own name as the plot.
    let names = [title.as_deref(), item.get("label").and_then(Value::as_str)];
    let says_more = |plot: &String| {
        names
            .iter()
            .flatten()
            .all(|name| !plot.trim().eq_ignore_ascii_case(name.trim()))
    };
    let plot = block(item, "plot")
        .filter(says_more)
        .or_else(|| block(item, "plotoutline").filter(says_more));
    let unique = item.get("uniqueid");
    let imdb_id = item
        .get("imdbnumber")
        .and_then(Value::as_str)
        .and_then(portal_api::imdb_id)
        .or_else(|| unique_str(unique, "imdb").and_then(|id| portal_api::imdb_id(&id)));
    let episode = item.get("episode").and_then(positive_u32);
    let season = item
        .get("season")
        .and_then(Value::as_i64)
        .filter(|season| *season > 0 || (*season == 0 && episode.is_some()))
        .and_then(|season| u32::try_from(season).ok());
    let runtime_minutes = [item.get("runtime"), item.get("duration")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_i64)
        .find(|seconds| *seconds > 0)
        .and_then(seconds_to_minutes);

    let mut info = MediaInfo {
        kind,
        title,
        original_title,
        show_title: text(item, "showtitle"),
        season,
        episode,
        year,
        premiered,
        plot,
        tagline: text(item, "tagline"),
        genres: strings(item, "genre"),
        runtime_minutes,
        mpaa: text(item, "mpaa").map(|mpaa| clean_mpaa(&mpaa)),
        rating: item.get("rating").and_then(Value::as_f64).and_then(rating),
        votes: item.get("votes").and_then(votes),
        studios: strings(item, "studio"),
        directors: strings(item, "director"),
        writers: strings(item, "writer"),
        countries: strings(item, "country"),
        cast: cast(item.get("cast")),
        imdb_id,
        tmdb_id: unique_str(unique, "tmdb").filter(|id| all_digits(id)),
        tvdb_id: unique_str(unique, "tvdb").filter(|id| all_digits(id)),
        trailer: item
            .get("trailer")
            .and_then(Value::as_str)
            .filter(|url| is_http(url))
            .map(str::to_string),
        poster: None,
        fanart: None,
        sources: Vec::new(),
        links: Vec::new(),
    };
    if !has_details(&info) {
        return None;
    }
    info.poster = art_str(art, &["poster", "tvshow.poster", "season.poster"]).or_else(|| {
        if kind == Some(MediaKind::Episode) {
            None
        } else {
            nonempty(item.get("thumbnail")).or_else(|| art_str(art, &["thumb"]))
        }
    });
    info.fanart = art_str(art, &["fanart", "tvshow.fanart"]);
    info.sources.push(SOURCE_KODI.to_string());
    if let Some(id) = &info.imdb_id {
        info.links.push(imdb_link(id));
    }
    Some(info)
}

pub fn imdb_link(id: &str) -> InfoLink {
    InfoLink {
        label: "IMDb".to_string(),
        url: format!("https://www.imdb.com/title/{id}/"),
    }
}

/// True when anything beyond the title, kind, and episode numbers is known.
fn has_details(info: &MediaInfo) -> bool {
    info.original_title.is_some()
        || info.year.is_some()
        || info.premiered.is_some()
        || info.plot.is_some()
        || info.tagline.is_some()
        || !info.genres.is_empty()
        || info.runtime_minutes.is_some()
        || info.mpaa.is_some()
        || info.rating.is_some()
        || info.votes.is_some()
        || !info.studios.is_empty()
        || !info.directors.is_empty()
        || !info.writers.is_empty()
        || !info.countries.is_empty()
        || !info.cast.is_empty()
        || info.imdb_id.is_some()
        || info.tmdb_id.is_some()
        || info.tvdb_id.is_some()
        || info.trailer.is_some()
}

fn media_kind(item: &Value) -> Option<MediaKind> {
    match item
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "movie" => Some(MediaKind::Movie),
        "tvshow" | "season" => Some(MediaKind::Tv),
        "episode" => Some(MediaKind::Episode),
        _ => None,
    }
}

fn text(item: &Value, key: &str) -> Option<String> {
    item.get(key)
        .and_then(Value::as_str)
        .map(plain_label)
        .filter(|value| !value.is_empty())
}

fn block(item: &Value, key: &str) -> Option<String> {
    let value = plain_block(item.get(key).and_then(Value::as_str)?);
    if value.is_empty() {
        return None;
    }
    Some(if value.chars().count() > PLOT_LIMIT {
        let mut cut: String = value.chars().take(PLOT_LIMIT).collect();
        cut.push('…');
        cut
    } else {
        value
    })
}

fn nonempty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn art_str(art: Option<&Value>, keys: &[&str]) -> Option<String> {
    let art = art?;
    keys.iter().find_map(|key| nonempty(art.get(*key)))
}

/// A Kodi string array, with markup stripped, blanks dropped, and repeats removed.
fn strings(item: &Value, key: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let values: Vec<&str> = match item.get(key) {
        Some(Value::Array(rows)) => rows.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(one)) => vec![one.as_str()],
        _ => Vec::new(),
    };
    for value in values {
        for part in value.split(" / ") {
            let plain = plain_label(part);
            if !plain.is_empty() && !out.iter().any(|seen| seen.eq_ignore_ascii_case(&plain)) {
                out.push(plain);
            }
        }
    }
    out
}

fn cast(value: Option<&Value>) -> Vec<CastMember> {
    let Some(rows) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut rows: Vec<(i64, usize, &Value)> = rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let order = row.get("order").and_then(Value::as_i64).unwrap_or(i64::MAX);
            (order, index, row)
        })
        .collect();
    rows.sort_by_key(|(order, index, _)| (*order, *index));
    let mut out: Vec<CastMember> = Vec::new();
    for (_, _, row) in rows {
        let Some(name) = text(row, "name") else {
            continue;
        };
        if out.iter().any(|seen| seen.name == name) {
            continue;
        }
        out.push(CastMember {
            name,
            role: text(row, "role"),
            thumb: nonempty(row.get("thumbnail")),
        });
        if out.len() == CAST_LIMIT {
            break;
        }
    }
    out
}

fn unique_str(unique: Option<&Value>, key: &str) -> Option<String> {
    let value = unique?.get(key)?;
    match value {
        Value::String(text) => Some(text.trim().to_string()).filter(|text| !text.is_empty()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn positive_u32(value: &Value) -> Option<u32> {
    value
        .as_i64()
        .filter(|number| *number > 0)
        .and_then(|number| u32::try_from(number).ok())
}

pub fn all_digits(text: &str) -> bool {
    !text.is_empty() && text.len() <= 12 && text.bytes().all(|byte| byte.is_ascii_digit())
}

pub fn is_http(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://")) && lower.len() > 10
}

pub fn plausible_year(year: i64) -> Option<u32> {
    (1870..=2200)
        .contains(&year)
        .then(|| u32::try_from(year).ok())
        .flatten()
}

/// `YYYY-MM-DD` from the start of `text`, or `None`.
pub fn date_prefix(text: &str) -> Option<String> {
    let date = text.trim().get(..10)?;
    let bytes = date.as_bytes();
    let shape = bytes.iter().enumerate().all(|(index, byte)| match index {
        4 | 7 => *byte == b'-',
        _ => byte.is_ascii_digit(),
    });
    if !shape {
        return None;
    }
    let year: i64 = date[..4].parse().ok()?;
    let month: u32 = date[5..7].parse().ok()?;
    let day: u32 = date[8..10].parse().ok()?;
    plausible_year(year)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(date.to_string())
}

pub fn year_of_date(date: &str) -> Option<u32> {
    plausible_year(date.get(..4)?.parse().ok()?)
}

/// Kodi's item runtime is in seconds.
pub fn seconds_to_minutes(seconds: i64) -> Option<u32> {
    if seconds <= 0 {
        return None;
    }
    u32::try_from(((seconds + 30) / 60).max(1)).ok()
}

/// A 0-10 rating rounded to one decimal; 0 means unrated.
pub fn rating(value: f64) -> Option<f64> {
    if !value.is_finite() || value <= 0.0 || value > 10.0 {
        return None;
    }
    Some((value * 10.0).round() / 10.0)
}

/// Kodi sends votes as text, sometimes with thousands separators.
fn votes(value: &Value) -> Option<u64> {
    let number = match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => {
            let digits: String = text
                .chars()
                .filter(|ch| !matches!(ch, ',' | '.' | ' ' | '\u{a0}'))
                .collect();
            digits.parse::<u64>().ok()
        }
        _ => None,
    }?;
    (number > 0).then_some(number)
}

fn clean_mpaa(mpaa: &str) -> String {
    let trimmed = mpaa.trim();
    trimmed
        .strip_prefix("Rated ")
        .or_else(|| trimmed.strip_prefix("rated "))
        .unwrap_or(trimmed)
        .trim()
        .to_string()
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/kodi_info.rs"]
mod tests;
