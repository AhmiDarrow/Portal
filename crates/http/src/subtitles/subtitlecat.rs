//! Subtitle Cat: movie and TV subtitles from subtitlecat.com, no key. A
//! search lists subtitle pages by file name; Portal keeps the ones named
//! for the asked-for episode or movie and reads the best three side by
//! side. Each page links its original file and machine translations of it
//! into many languages; translations are marked and rank lower. Pages are
//! read with plain string scanning; when their markup changes, Portal finds
//! nothing rather than failing. Files are plain SRT.

use reqwest::StatusCode;

use super::langs;
use super::rank::{Found, Source};
use super::SourceError;
use crate::metadata::net;
use crate::metadata::text::{norm_title, strip_html};

pub const SITE: &str = "https://www.subtitlecat.com";

const PAGE_CAP: usize = 2 << 20;
const BUSY: &str = "Subtitle Cat asked Portal to slow down. Try again in a minute.";

/// What one search asks for: a show and episode, or a movie.
#[derive(Debug, Clone, PartialEq)]
pub struct Search {
    pub title: String,
    pub year: Option<u32>,
    pub episode: Option<(u32, u32)>,
}

/// One row of the search listing.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// "subs/118/Name.html", escapes decoded.
    pub path: String,
    pub name: String,
    /// The language the subtitle was made in.
    pub original: Option<&'static str>,
    pub downloads: Option<u64>,
}

/// "Show S01E02" for an episode, "Title Year" for a movie.
pub fn query_text(search: &Search) -> String {
    match (search.episode, search.year) {
        (Some((season, number)), _) => format!("{} S{season:02}E{number:02}", search.title),
        (None, Some(year)) => format!("{} {year}", search.title),
        (None, None) => search.title.clone(),
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let at = text.find(start)? + start.len();
    let rest = &text[at..];
    Some(&rest[..rest.find(end)?])
}

fn refusal(status: StatusCode) -> SourceError {
    match status {
        StatusCode::TOO_MANY_REQUESTS => SourceError::Note(BUSY.into()),
        _ => SourceError::Failed,
    }
}

/// The listing's rows: `<a href="subs/1694/Name.html">Name</a> (translated
/// from English)`, then size, downloads and language count.
pub fn parse_listing(html: &str) -> Vec<Row> {
    html.split("<tr>")
        .skip(1)
        .filter_map(|chunk| parse_row(chunk.split("</tr>").next().unwrap_or("")))
        .collect()
}

fn parse_row(row: &str) -> Option<Row> {
    let start = row.find("<a href=\"")? + "<a href=\"".len();
    let (href, rest) = row[start..].split_once('"')?;
    let path = site_path(href, ".html")?;
    let (name, after) = rest.strip_prefix('>')?.split_once("</a>")?;
    let name = strip_html(name);
    if name.is_empty() {
        return None;
    }
    let note = after.split('<').next().unwrap_or("");
    let original = between(note, "translated from ", ")")
        .and_then(langs::find)
        .map(|language| language.code);
    let downloads = between(
        row,
        "Downloads</span><span class=\"sub-table__metric-value\">",
        "<",
    )
    .map(|text| text.trim().replace(',', ""))
    .and_then(|text| text.parse().ok());
    Some(Row {
        path,
        name,
        original,
        downloads,
    })
}

/// The words of a name with where each starts.
fn words(name: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start = None;
    for (at, ch) in name.char_indices().chain([(name.len(), ' ')]) {
        match (ch.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                out.push((from, name[from..at].to_lowercase()));
                start = None;
            }
            _ => {}
        }
    }
    out
}

fn digits(text: &str) -> Option<u32> {
    let plain = (1..=4).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
    plain.then(|| text.parse().ok()).flatten()
}

/// "s01e02", "1x02", or "s01" then "e02", at word `index`.
fn code_at(list: &[(usize, String)], index: usize) -> Option<(u32, u32)> {
    let word = list[index].1.as_str();
    if let Some((season, number)) = word.strip_prefix('s').and_then(|rest| rest.split_once('e')) {
        return Some((digits(season)?, digits(number)?));
    }
    if let Some((season, number)) = word.split_once('x') {
        return (season.len() <= 2).then_some((digits(season)?, digits(number)?));
    }
    let season = digits(word.strip_prefix('s')?)?;
    let number = digits(list.get(index + 1)?.1.strip_prefix('e')?)?;
    Some((season, number))
}

fn is_year(word: &str) -> bool {
    word.len() == 4 && digits(word).is_some_and(|year| (1900..=2100).contains(&year))
}

/// Words that start the release part of a movie's file name.
const RELEASE_WORDS: &[&str] = &[
    "480p", "576p", "720p", "1080p", "2160p", "4k", "bluray", "bdrip", "brrip", "web", "webrip",
    "webdl", "hdtv", "dvdrip", "hdrip", "x264", "x265", "h264", "hevc", "remux",
];

/// A title for comparing: normalised, without a leading "The" or a year at
/// its end.
fn title_key(title: &str) -> String {
    let norm = norm_title(title);
    let norm = norm.strip_prefix("the ").unwrap_or(&norm);
    match norm.rsplit_once(' ') {
        Some((base, year)) if is_year(year) => base.to_string(),
        _ => norm.to_string(),
    }
}

/// The name is for what is asked: for an episode, its show then its code
/// ("Show S01E02", "Show.1x02"); for a movie, its title then its year (when
/// asked) or the release part. "Example Show Academy" is not
/// "Example Show".
pub fn names_it(name: &str, search: &Search) -> bool {
    let list = words(name);
    let want = title_key(&search.title);
    if want.is_empty() {
        return false;
    }
    let cut = match search.episode {
        Some(episode) => {
            let Some(index) = (0..list.len()).find(|index| code_at(&list, *index).is_some()) else {
                return false;
            };
            if code_at(&list, index) != Some(episode) {
                return false;
            }
            list[index].0
        }
        None => {
            // The first word is the title's, even when it is a year ("1917").
            let year_index = (1..list.len()).find(|index| {
                let word = &list[*index].1;
                is_year(word)
                    && search.year.is_none_or(|year| {
                        digits(word).is_some_and(|found| found.abs_diff(year) <= 1)
                    })
            });
            match (year_index, search.year) {
                (Some(index), _) => list[index].0,
                (None, Some(_)) => return false,
                (None, None) => (1..list.len())
                    .find(|index| RELEASE_WORDS.contains(&list[*index].1.as_str()))
                    .map_or(name.len(), |index| list[index].0),
            }
        }
    };
    title_key(&name[..cut]) == want
}

/// The rows to read, at most `count`: named for what is asked, those made
/// in a wanted language first, then the most downloaded.
pub fn best_rows(rows: Vec<Row>, search: &Search, languages: &[String], count: usize) -> Vec<Row> {
    let mut rows: Vec<Row> = rows
        .into_iter()
        .filter(|row| names_it(&row.name, search))
        .collect();
    let made_in_wanted = |row: &Row| {
        row.original
            .is_some_and(|code| languages.iter().any(|want| langs::same_family(want, code)))
    };
    rows.sort_by(|a, b| {
        made_in_wanted(b)
            .cmp(&made_in_wanted(a))
            .then(b.downloads.cmp(&a.downloads))
    });
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| seen.insert(row.path.clone()));
    rows.truncate(count);
    rows
}

/// A site language code as Portal's: "iw" is Hebrew; others as `langs`
/// reads them ("pt" Portuguese, "zh-CN", "es-419" Spanish).
fn site_language(code: &str) -> Option<&'static str> {
    match code.to_ascii_lowercase().as_str() {
        "iw" => Some("he"),
        other => langs::find(other).map(|language| language.code),
    }
}

/// A page's files in the wanted languages (or their family), one per
/// language. The file in the language the subtitle was made in is the
/// original; the rest are machine translations, and so is every file when
/// the listing did not say the original language.
pub fn parse_page(
    html: &str,
    row: &Row,
    languages: &[String],
    episode: Option<(u32, u32)>,
) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for chunk in html.split("id=\"download_").skip(1) {
        let Some((code, rest)) = chunk.split_once('"') else {
            continue;
        };
        let Some(path) = between(rest, "href=\"", "\"").and_then(|href| site_path(href, ".srt"))
        else {
            continue;
        };
        let Some(language) = site_language(code) else {
            continue;
        };
        if !languages
            .iter()
            .any(|want| langs::same_family(want, language))
        {
            continue;
        }
        let plain_code = code.eq_ignore_ascii_case(language) || !code.contains('-');
        let original = row.original == Some(language) && plain_code;
        let found = Found {
            id: format!("{}:{path}", Source::SubtitleCat.prefix()),
            source: Source::SubtitleCat,
            language: language.to_string(),
            release: row.name.clone(),
            hearing_impaired: false,
            downloads: row.downloads,
            rating: None,
            hash_match: false,
            episode,
            episode_unsure: false,
            machine_translated: !original,
        };
        match out.iter_mut().find(|item| item.language == found.language) {
            Some(item) if item.machine_translated && original => *item = found,
            Some(_) => {}
            None => out.push(found),
        }
    }
    out
}

/// Percent escapes decoded; `None` for a broken escape or text that is not
/// UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A link on the site as "subs/<number>/<name>", escapes decoded, or
/// `None` when it is anything else.
fn site_path(href: &str, suffix: &str) -> Option<String> {
    let path = percent_decode(href.strip_prefix('/').unwrap_or(href))?;
    valid_path(&path, suffix).then_some(path)
}

/// "subs/<number>/<name><suffix>", the name without slashes, "?", "#",
/// control characters or "..".
fn valid_path(path: &str, suffix: &str) -> bool {
    let Some((folder, name)) = path
        .strip_prefix("subs/")
        .and_then(|rest| rest.split_once('/'))
    else {
        return false;
    };
    (1..=10).contains(&folder.len())
        && folder.bytes().all(|byte| byte.is_ascii_digit())
        && name.len() > suffix.len()
        && name.len() <= 250
        && name.ends_with(suffix)
        && !name.contains("..")
        && !name
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\' | '?' | '#'))
}

/// A subtitle file's id: "subs/<number>/<name>.srt".
pub fn valid_id(path: &str) -> bool {
    valid_path(path, ".srt")
}

fn site_url(path: &str) -> Result<reqwest::Url, SourceError> {
    let mut url = reqwest::Url::parse(SITE).map_err(|_| SourceError::Failed)?;
    {
        let mut segments = url.path_segments_mut().map_err(|_| SourceError::Failed)?;
        segments.clear();
        for segment in path.split('/') {
            segments.push(segment);
        }
    }
    Ok(url)
}

async fn read_page(
    client: &reqwest::Client,
    row: Option<&Row>,
    languages: &[String],
    episode: Option<(u32, u32)>,
) -> Result<Vec<Found>, SourceError> {
    let Some(row) = row else {
        return Ok(Vec::new());
    };
    let (status, body) = net::fetch_bytes(client.get(site_url(&row.path)?), PAGE_CAP).await?;
    match status {
        status if status.is_success() => Ok(parse_page(
            &String::from_utf8_lossy(&body),
            row,
            languages,
            episode,
        )),
        StatusCode::NOT_FOUND => Ok(Vec::new()),
        status => Err(refusal(status)),
    }
}

pub async fn search(
    client: &reqwest::Client,
    search: &Search,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let url = net::url(
        &format!("{SITE}/index.php"),
        &[("search", query_text(search).as_str())],
    )?;
    let (status, body) = net::fetch_bytes(client.get(url), PAGE_CAP).await?;
    match status {
        status if status.is_success() => {}
        StatusCode::NOT_FOUND => return Ok(Vec::new()),
        status => return Err(refusal(status)),
    }
    let rows = best_rows(
        parse_listing(&String::from_utf8_lossy(&body)),
        search,
        languages,
        3,
    );
    let (first, second, third) = tokio::join!(
        read_page(client, rows.first(), languages, search.episode),
        read_page(client, rows.get(1), languages, search.episode),
        read_page(client, rows.get(2), languages, search.episode),
    );
    let mut found = Vec::new();
    let mut refused = None;
    for answer in [first, second, third] {
        match answer {
            Ok(list) => found.extend(list),
            Err(err) => refused = refused.or(Some(err)),
        }
    }
    match refused {
        Some(err) if found.is_empty() => Err(err),
        _ => Ok(found),
    }
}

pub async fn download(client: &reqwest::Client, path: &str) -> Result<Vec<u8>, SourceError> {
    if !valid_id(path) {
        return Err(SourceError::Failed);
    }
    let (status, body) = net::fetch_bytes(client.get(site_url(path)?), super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(refusal(status))
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/subtitlecat.rs"]
mod tests;
