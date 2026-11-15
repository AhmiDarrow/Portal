//! TVsubtitles: TV subtitles from tvsubtitles.net, no key. The site's own
//! search is gone, so Portal reads its list of every show at most once a
//! day (kept in memory and in `%LOCALAPPDATA%\Portal\subtitles\`), finds
//! the show by name, then reads the season's page for the episode and the
//! episode's page for its subtitles. Pages are read with plain string
//! scanning; when their markup changes, Portal finds nothing rather than
//! failing. A subtitle downloads as a zip whose path the site's download
//! page spells out in pieces in a small script.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use reqwest::StatusCode;

use super::langs;
use super::rank::{Found, Source};
use super::SourceError;
use crate::metadata::net;
use crate::metadata::text::{norm_title, strip_html};

pub const SITE: &str = "https://www.tvsubtitles.net";

/// The show list is read again after this long.
const LIST_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// When reading it again fails, the old list is used this long first.
const LIST_RETRY: Duration = Duration::from_secs(30 * 60);
/// The show list is over half a megabyte.
const LIST_CAP: usize = 4 << 20;
const PAGE_CAP: usize = 2 << 20;
const LIST_FILE: &str = "tvsubtitles-shows.txt";
const BUSY: &str = "TVsubtitles asked Portal to slow down. Try again in a minute.";

/// One show in the site's list.
#[derive(Debug, Clone, PartialEq)]
pub struct Show {
    pub id: u32,
    pub name: String,
    /// The years it ran, when the list says.
    pub first: Option<u32>,
    pub last: Option<u32>,
}

type Kept = Option<(Arc<Vec<Show>>, SystemTime)>;

/// The site's show list, read at most once a day.
pub struct ShowList {
    file: Option<PathBuf>,
    kept: Arc<tokio::sync::Mutex<Kept>>,
    refreshing: Arc<AtomicBool>,
}

impl ShowList {
    /// `None` keeps the list in memory only.
    pub fn open(dir: Option<&Path>) -> Self {
        Self {
            file: dir.map(|dir| dir.join(LIST_FILE)),
            kept: Arc::new(tokio::sync::Mutex::new(None)),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The list. The first read waits for the site; after that an old list
    /// answers at once while a fresh one is read in the background, so a
    /// slow site never holds up a search or loses a download to a timeout.
    pub async fn get(&self, client: &reqwest::Client) -> Result<Arc<Vec<Show>>, SourceError> {
        let mut kept = self.kept.lock().await;
        if kept.is_none() {
            *kept = read_file(self.file.as_deref());
        }
        if let Some((shows, at)) = kept.as_ref() {
            if !fresh(*at, LIST_TTL) {
                self.refresh_later(client.clone());
            }
            return Ok(shows.clone());
        }
        let shows = fetch_shows(client).await?;
        if shows.is_empty() {
            return Err(SourceError::Failed);
        }
        write_file(self.file.as_deref(), &shows);
        let shows = Arc::new(shows);
        *kept = Some((shows.clone(), SystemTime::now()));
        Ok(shows)
    }

    /// Reads the site's list in the background, once at a time; a failed read
    /// is tried again after `LIST_RETRY`.
    fn refresh_later(&self, client: reqwest::Client) {
        if self.refreshing.swap(true, Ordering::SeqCst) {
            return;
        }
        let kept = self.kept.clone();
        let refreshing = self.refreshing.clone();
        let file = self.file.clone();
        tokio::spawn(async move {
            let fetched = fetch_shows(&client).await;
            let mut kept = kept.lock().await;
            match fetched {
                Ok(shows) if !shows.is_empty() => {
                    write_file(file.as_deref(), &shows);
                    *kept = Some((Arc::new(shows), SystemTime::now()));
                }
                _ => {
                    if let Some((_, at)) = kept.as_mut() {
                        *at = SystemTime::now()
                            .checked_sub(LIST_TTL - LIST_RETRY)
                            .unwrap_or(*at);
                    }
                }
            }
            refreshing.store(false, Ordering::SeqCst);
        });
    }
}

fn read_file(file: Option<&Path>) -> Kept {
    let file = file?;
    let at = std::fs::metadata(file)
        .and_then(|meta| meta.modified())
        .ok()?;
    let shows = read_list(&std::fs::read_to_string(file).ok()?);
    (!shows.is_empty()).then(|| (Arc::new(shows), at))
}

fn write_file(file: Option<&Path>, shows: &[Show]) {
    let Some(file) = file else { return };
    let part = file.with_extension("txt.part");
    let written = file
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&part, write_list(shows)))
        .and_then(|()| std::fs::rename(&part, file));
    if let Err(err) = written {
        tracing::debug!(error = %err, "Could not keep the TVsubtitles show list");
    }
}

fn fresh(at: SystemTime, ttl: Duration) -> bool {
    at.elapsed().is_ok_and(|age| age < ttl)
}

async fn fetch_shows(client: &reqwest::Client) -> Result<Vec<Show>, SourceError> {
    let request = client.get(format!("{SITE}/tvshows.html"));
    let (status, body) = net::fetch_bytes(request, LIST_CAP).await?;
    if status.is_success() {
        Ok(parse_shows(&String::from_utf8_lossy(&body)))
    } else {
        Err(refusal(status))
    }
}

fn refusal(status: StatusCode) -> SourceError {
    match status {
        StatusCode::TOO_MANY_REQUESTS => SourceError::Note(BUSY.into()),
        _ => SourceError::Failed,
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let at = text.find(start)? + start.len();
    let rest = &text[at..];
    Some(&rest[..rest.find(end)?])
}

fn number<T: std::str::FromStr>(text: &str, most: usize) -> Option<T> {
    let text = text.trim();
    let digits = (1..=most).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
    digits.then(|| text.parse().ok()).flatten()
}

/// The rows of `tvshows.html`: `<a href="tvshow-19-15.html"><b>Example Show</b>`
/// (the show's id, then its latest season), the years last in the row.
pub fn parse_shows(html: &str) -> Vec<Show> {
    html.split("href=\"tvshow-")
        .skip(1)
        .filter_map(|chunk| parse_show_row(chunk.split("</tr>").next().unwrap_or("")))
        .collect()
}

fn parse_show_row(row: &str) -> Option<Show> {
    let (id, _) = row.split_once('-')?;
    let name = strip_html(between(row, "<b>", "</b>")?);
    if name.is_empty() || name.chars().count() > 200 {
        return None;
    }
    let years = row
        .rfind("<td>")
        .and_then(|at| between(&row[at..], "<td>", "</td>"))
        .unwrap_or("");
    let (first, last) = match years.split_once('-') {
        Some((first, last)) => (number(first, 4), number(last, 4)),
        None => (number(years, 4), None),
    };
    Some(Show {
        id: number(id, 9)?,
        name,
        first,
        last,
    })
}

/// The list as kept on disk: one show per line, "id, first year, last
/// year, name" separated by tabs.
fn write_list(shows: &[Show]) -> String {
    let year = |year: Option<u32>| year.map(|year| year.to_string()).unwrap_or_default();
    shows
        .iter()
        .map(|show| {
            let name = show.name.replace(['\t', '\n', '\r'], " ");
            format!(
                "{}\t{}\t{}\t{name}\n",
                show.id,
                year(show.first),
                year(show.last)
            )
        })
        .collect()
}

fn read_list(text: &str) -> Vec<Show> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, '\t');
            let id = number(parts.next()?, 9)?;
            let first = number(parts.next()?, 4);
            let last = number(parts.next()?, 4);
            let name = parts.next()?.trim().to_string();
            (!name.is_empty()).then_some(Show {
                id,
                name,
                first,
                last,
            })
        })
        .collect()
}

/// A show name for comparing: normalised, without a leading "The", and
/// with a year at its end apart ("Charmed (2018)").
fn name_key(name: &str) -> (String, Option<u32>) {
    let norm = norm_title(name);
    let norm = norm.strip_prefix("the ").unwrap_or(&norm);
    match norm.rsplit_once(' ') {
        Some((base, year)) => match number::<u32>(year, 4).filter(|year| *year >= 1900) {
            Some(year) => (base.to_string(), Some(year)),
            None => (norm.to_string(), None),
        },
        None => (norm.to_string(), None),
    }
}

/// The show by name. Of shows sharing a name ("Charmed", "Charmed
/// (2018)"), the one whose year or run fits the asked-for year, else the
/// one named exactly so, else the only one.
pub fn pick_show<'a>(shows: &'a [Show], name: &str, year: Option<u32>) -> Option<&'a Show> {
    let (base, named_year) = name_key(name);
    if base.is_empty() {
        return None;
    }
    let candidates: Vec<(&Show, Option<u32>)> = shows
        .iter()
        .filter_map(|show| {
            let (theirs, their_year) = name_key(&show.name);
            (theirs == base).then_some((show, their_year))
        })
        .collect();
    let exact = |their_year: &Option<u32>| *their_year == named_year;
    if let Some(year) = named_year.or(year) {
        let fits = |show: &Show, their_year: Option<u32>| {
            let ran = show.first.is_some_and(|first| {
                year + 1 >= first && show.last.is_none_or(|last| year <= last + 1)
            });
            ran || their_year.is_some_and(|their| their.abs_diff(year) <= 1)
        };
        let fitting: Vec<&(&Show, Option<u32>)> = candidates
            .iter()
            .filter(|(show, their_year)| fits(show, *their_year))
            .collect();
        if let Some((show, _)) = fitting
            .iter()
            .find(|(_, their_year)| exact(their_year))
            .or(fitting.first())
        {
            return Some(show);
        }
    }
    let only = (candidates.len() == 1).then(|| &candidates[0]);
    candidates
        .iter()
        .find(|(_, their_year)| exact(their_year))
        .or(only)
        .map(|(show, _)| *show)
}

/// The episode's page number on a season's page and the code the site
/// files it under. The asked-for code is used unless its title differs
/// from the asked-for one and exactly one other episode of the season has
/// that title: the site numbers some seasons from an unaired pilot.
pub fn parse_season(
    html: &str,
    want: (u32, u32),
    title: Option<&str>,
) -> Option<(u64, (u32, u32))> {
    let rows: Vec<((u32, u32), u64, String)> = html.split("<tr").filter_map(season_row).collect();
    let by_code = rows.iter().find(|row| row.0 == want);
    if let Some(title) = title.map(norm_title).filter(|title| !title.is_empty()) {
        let named = |row: &&((u32, u32), u64, String)| norm_title(&row.2) == title;
        let titled: Vec<_> = rows
            .iter()
            .filter(|row| row.0 .0 == want.0)
            .filter(named)
            .collect();
        if !by_code.as_ref().is_some_and(named) && titled.len() == 1 {
            return Some((titled[0].1, titled[0].0));
        }
    }
    by_code.map(|row| (row.1, row.0))
}

/// `<td>1x01</td>` then `<a href="episode-1000.html"><b>Unaired Pilot</b>`.
fn season_row(row: &str) -> Option<((u32, u32), u64, String)> {
    let code = between(row, "<td>", "</td>")?;
    let (season, episode) = code.trim().split_once('x')?;
    let code = (number(season, 3)?, number(episode, 4)?);
    let id = number(between(row, "href=\"episode-", ".html\"")?, 10)?;
    let name = between(row, "<b>", "</b>")
        .map(strip_html)
        .unwrap_or_default();
    Some((code, id, name))
}

/// The subtitles on an episode's page in the wanted languages (or their
/// family). `episode` is the code the site files it under, which its zips
/// are named for.
pub fn parse_episode(html: &str, languages: &[String], episode: (u32, u32)) -> Vec<Found> {
    html.split("<a href=\"/subtitle-")
        .skip(1)
        .filter_map(|chunk| parse_subtitle(chunk, episode))
        .filter(|found| {
            languages
                .iter()
                .any(|code| langs::same_family(code, &found.language))
        })
        .collect()
}

fn parse_subtitle(chunk: &str, episode: (u32, u32)) -> Option<Found> {
    let (id, rest) = chunk.split_once(".html\"")?;
    if !valid_id(id) {
        return None;
    }
    let block = rest.split("</a>").next().unwrap_or("");
    if !block.contains("class=\"subtitlen\"") {
        return None;
    }
    let language = flag_language(between(block, "images/flags/", ".gif")?)?;
    let release = between(block, "<h5", "</h5>")
        .and_then(|heading| heading.split_once('>'))
        .map(|(_, text)| strip_html(text))
        .unwrap_or_default();
    let downloads = between(block, "title=\"downloaded\">", "</p>")
        .map(strip_html)
        .and_then(|text| number(&text, 12));
    Some(Found {
        id: format!("{}:{id}", Source::TvSubtitles.prefix()),
        source: Source::TvSubtitles,
        language: language.to_string(),
        release,
        hearing_impaired: false,
        downloads,
        rating: None,
        hash_match: false,
        episode: Some(episode),
        episode_unsure: false,
        machine_translated: false,
    })
}

/// A flag image's name as a language. Most are language codes; a few are
/// countries ("br" is Brazilian Portuguese, "gr" Greek).
fn flag_language(flag: &str) -> Option<&'static str> {
    let code = match flag.to_ascii_lowercase().as_str() {
        "br" => "pt-BR",
        "gr" => "el",
        "ua" => "uk",
        "cz" => "cs",
        "cn" => "zh-CN",
        "tw" => "zh-TW",
        "jp" => "ja",
        "kr" => "ko",
        "se" => "sv",
        "dk" => "da",
        "il" => "he",
        "ir" => "fa",
        other => return langs::find(other).map(|language| language.code),
    };
    Some(code)
}

/// A subtitle's number.
pub fn valid_id(id: &str) -> bool {
    (1..=10).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_digit())
}

/// The zip's path that the download page's script builds from pieces:
/// `var s1= 'fil'; ... var s4= 'e_1x10_en.zip'; document.location = s1+s2+s3+s4;`.
pub fn zip_path(html: &str) -> Option<String> {
    let expression = between(html, "document.location", ";")?;
    let expression = expression.trim_start().strip_prefix('=')?;
    let mut path = String::new();
    for name in expression.split('+') {
        let name = name.trim();
        let plain = (1..=20).contains(&name.len())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        if !plain {
            return None;
        }
        path.push_str(piece(html, name)?);
    }
    valid_zip_path(&path).then_some(path)
}

/// The quoted text a `var name= '...'` line gives `name`.
fn piece<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("var {name}");
    let mut rest = html;
    while let Some(at) = rest.find(&marker) {
        rest = &rest[at + marker.len()..];
        let value = rest
            .trim_start()
            .strip_prefix('=')
            .map(str::trim_start)
            .and_then(|text| text.strip_prefix('\''));
        if let Some(value) = value {
            return value.split_once('\'').map(|(value, _)| value);
        }
    }
    None
}

/// "files/<name>.zip", the name made of letters, digits, spaces and
/// `._()'-`, never "..".
pub fn valid_zip_path(path: &str) -> bool {
    let Some(name) = path.strip_prefix("files/") else {
        return false;
    };
    (5..=200).contains(&name.len())
        && name.ends_with(".zip")
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b" ._()'-".contains(&byte))
}

async fn page(client: &reqwest::Client, url: &str) -> Result<Option<String>, SourceError> {
    let (status, body) = net::fetch_bytes(client.get(url), PAGE_CAP).await?;
    match status {
        status if status.is_success() => Ok(Some(String::from_utf8_lossy(&body).into_owned())),
        StatusCode::NOT_FOUND => Ok(None),
        status => Err(refusal(status)),
    }
}

/// What one search needs.
pub struct Search<'a> {
    pub show: &'a str,
    pub year: Option<u32>,
    /// The episode's own title, to catch the site's other numbering.
    pub title: Option<&'a str>,
    pub episode: (u32, u32),
}

/// One episode's subtitles: the show from the list, then its season's
/// page, then the episode's page.
pub async fn search(
    client: &reqwest::Client,
    shows: &ShowList,
    search: &Search<'_>,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let list = shows.get(client).await?;
    let Some(show) = pick_show(&list, search.show, search.year) else {
        return Ok(Vec::new());
    };
    let (season, _) = search.episode;
    let url = format!("{SITE}/tvshow-{}-{season}.html", show.id);
    let Some(html) = page(client, &url).await? else {
        return Ok(Vec::new());
    };
    let Some((episode_id, code)) = parse_season(&html, search.episode, search.title) else {
        return Ok(Vec::new());
    };
    let url = format!("{SITE}/episode-{episode_id}.html");
    let Some(html) = page(client, &url).await? else {
        return Ok(Vec::new());
    };
    let mut found = parse_episode(&html, languages, code);
    if code != search.episode {
        // The release names carry the site's number; say it is still the asked episode.
        let (season, number) = search.episode;
        let note = format!(
            "S{season:02}E{number:02} (TVsubtitles numbers it {}x{:02})",
            code.0, code.1
        );
        for item in &mut found {
            item.release = if item.release.is_empty() {
                note.clone()
            } else {
                format!("{note}\n{}", item.release)
            };
        }
    }
    Ok(found)
}

pub async fn download(client: &reqwest::Client, id: &str) -> Result<Vec<u8>, SourceError> {
    if !valid_id(id) {
        return Err(SourceError::Failed);
    }
    let page = format!("{SITE}/download-{id}.html");
    let request = client.get(&page).header(
        reqwest::header::REFERER,
        format!("{SITE}/subtitle-{id}.html"),
    );
    let (status, body) = net::fetch_bytes(request, PAGE_CAP).await?;
    if !status.is_success() {
        return Err(refusal(status));
    }
    let path = zip_path(&String::from_utf8_lossy(&body)).ok_or(SourceError::Failed)?;
    let name = path.strip_prefix("files/").ok_or(SourceError::Failed)?;
    let mut url = reqwest::Url::parse(SITE).map_err(|_| SourceError::Failed)?;
    url.path_segments_mut()
        .map_err(|_| SourceError::Failed)?
        .clear()
        .push("files")
        .push(name);
    let request = client.get(url).header(reqwest::header::REFERER, page);
    let (status, body) = net::fetch_bytes(request, super::FILE_CAP).await?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(refusal(status))
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/tvsubtitles.rs"]
mod tests;
