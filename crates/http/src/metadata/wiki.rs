//! Wikipedia, for movies. With an IMDb id the English article comes from
//! Wikidata (property P345 is the IMDb id); without one, a title search is
//! accepted only when the article is described as a film, its title matches,
//! and (when a year is known) the year appears; the article's Wikidata item
//! then gives the IMDb id. Otherwise nothing is added.

use portal_api::{InfoLink, MediaInfo};
use portal_engine::kodi_info::is_http;
use serde_json::Value;

use super::net::{self, Failed};
use super::text::{norm_title, plot_at, str_at};
use super::{Query, SOURCE_WIKIPEDIA};

const WIKIDATA: &str = "https://www.wikidata.org/w/api.php";
const WIKIPEDIA: &str = "https://en.wikipedia.org/w/api.php";
const SUMMARY: &str = "https://en.wikipedia.org/api/rest_v1/page/summary/";
/// Search results whose summaries are read before giving up.
const TRIES: usize = 2;

pub async fn lookup(client: &reqwest::Client, query: &Query) -> Result<Option<MediaInfo>, Failed> {
    if let Some(imdb) = query.imdb_id.as_deref() {
        return by_imdb(client, imdb).await;
    }
    let Some(title) = query.title.as_deref() else {
        return Ok(None);
    };
    by_title(client, title, query.year).await
}

async fn by_imdb(client: &reqwest::Client, imdb: &str) -> Result<Option<MediaInfo>, Failed> {
    let statement = format!("haswbstatement:P345={imdb}");
    let url = net::url(
        WIKIDATA,
        &[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", &statement),
            ("srlimit", "1"),
            ("format", "json"),
        ],
    )?;
    let Some(found) = net::get_json(client.get(url)).await? else {
        return Ok(None);
    };
    let Some(item) = wikidata_item(&found) else {
        return Ok(None);
    };
    let url = net::url(
        WIKIDATA,
        &[
            ("action", "wbgetentities"),
            ("ids", &item),
            ("props", "sitelinks"),
            ("sitefilter", "enwiki"),
            ("format", "json"),
        ],
    )?;
    let Some(entity) = net::get_json(client.get(url)).await? else {
        return Ok(None);
    };
    let Some(title) = enwiki_title(&entity, &item) else {
        return Ok(None);
    };
    let Some(summary) = summary(client, &title).await? else {
        return Ok(None);
    };
    Ok(parse_summary(&summary))
}

async fn by_title(
    client: &reqwest::Client,
    title: &str,
    year: Option<u32>,
) -> Result<Option<MediaInfo>, Failed> {
    let search = match year {
        Some(year) => format!("{title} {year} film"),
        None => format!("{title} film"),
    };
    let url = net::url(
        WIKIPEDIA,
        &[
            ("action", "query"),
            ("list", "search"),
            ("srsearch", &search),
            ("srlimit", "5"),
            ("srprop", ""),
            ("format", "json"),
        ],
    )?;
    let Some(results) = net::get_json(client.get(url)).await? else {
        return Ok(None);
    };
    for candidate in title_candidates(&results, title).into_iter().take(TRIES) {
        let Some(summary) = summary(client, &candidate).await? else {
            continue;
        };
        if film_matches(&summary, title, year) {
            let mut info = parse_summary(&summary);
            if let (Some(info), Some(item)) = (info.as_mut(), str_at(&summary, "wikibase_item")) {
                // Only a bonus: the article stands without it.
                info.imdb_id = imdb_of(client, &item).await.ok().flatten();
            }
            return Ok(info);
        }
    }
    Ok(None)
}

/// The IMDb id (property P345) of a Wikidata item.
async fn imdb_of(client: &reqwest::Client, item: &str) -> Result<Option<String>, Failed> {
    if !item.starts_with('Q') || !item[1..].bytes().all(|byte| byte.is_ascii_digit()) {
        return Ok(None);
    }
    let url = net::url(
        WIKIDATA,
        &[
            ("action", "wbgetclaims"),
            ("entity", item),
            ("property", "P345"),
            ("format", "json"),
        ],
    )?;
    Ok(net::get_json(client.get(url))
        .await?
        .and_then(|claims| imdb_claim(&claims)))
}

/// The first IMDb id in a `wbgetclaims` answer.
pub fn imdb_claim(claims: &Value) -> Option<String> {
    claims
        .get("claims")?
        .get("P345")?
        .as_array()?
        .iter()
        .filter_map(|claim| {
            claim
                .get("mainsnak")?
                .get("datavalue")?
                .get("value")?
                .as_str()
        })
        .find_map(portal_api::imdb_id)
}

async fn summary(client: &reqwest::Client, title: &str) -> Result<Option<Value>, Failed> {
    let url = net::url_with_segment(SUMMARY, &title.replace(' ', "_"))?;
    net::get_json(client.get(url)).await
}

/// `Q…` from a Wikidata search answer.
pub fn wikidata_item(found: &Value) -> Option<String> {
    let title = found
        .get("query")?
        .get("search")?
        .as_array()?
        .first()?
        .get("title")?
        .as_str()?;
    let digits = title.strip_prefix('Q')?;
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| title.to_string())
}

/// The English Wikipedia article title from a `wbgetentities` answer.
pub fn enwiki_title(entity: &Value, item: &str) -> Option<String> {
    str_at(
        entity
            .get("entities")?
            .get(item)?
            .get("sitelinks")?
            .get("enwiki")?,
        "title",
    )
}

/// Search hits whose title, less a trailing "(… film)", matches.
pub fn title_candidates(results: &Value, title: &str) -> Vec<String> {
    let wanted = norm_title(title);
    if wanted.is_empty() {
        return Vec::new();
    }
    results
        .get("query")
        .and_then(|query| query.get("search"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("title").and_then(Value::as_str))
        .filter(|candidate| norm_title(bare_title(candidate)) == wanted)
        .map(str::to_string)
        .collect()
}

/// "Example (1995 film)" is "Example".
fn bare_title(title: &str) -> &str {
    match title.rfind(" (") {
        Some(open) if title.ends_with(')') => &title[..open],
        _ => title,
    }
}

/// A page is taken for a title search only when it describes a film of that
/// title, from that year when a year is known.
pub fn film_matches(summary: &Value, title: &str, year: Option<u32>) -> bool {
    if summary.get("type").and_then(Value::as_str) == Some("disambiguation") {
        return false;
    }
    let description = str_at(summary, "description")
        .unwrap_or_default()
        .to_lowercase();
    let page_title = str_at(summary, "title").unwrap_or_default();
    if !description.contains("film") && !page_title.to_lowercase().contains("film)") {
        return false;
    }
    if norm_title(bare_title(&page_title)) != norm_title(title) {
        return false;
    }
    match year {
        Some(year) => {
            let year = year.to_string();
            description.contains(&year) || page_title.contains(&year)
        }
        None => true,
    }
}

pub fn parse_summary(summary: &Value) -> Option<MediaInfo> {
    if summary.get("type").and_then(Value::as_str) == Some("disambiguation") {
        return None;
    }
    let page = summary
        .get("content_urls")
        .and_then(|urls| urls.get("desktop"))
        .and_then(|desktop| str_at(desktop, "page"))
        .filter(|url| is_http(url));
    let plot = plot_at(summary, "extract");
    if plot.is_none() && page.is_none() {
        return None;
    }
    let poster = ["originalimage", "thumbnail"]
        .iter()
        .find_map(|key| summary.get(*key).and_then(|image| str_at(image, "source")))
        .filter(|url| is_http(url));
    Some(MediaInfo {
        plot,
        poster,
        sources: vec![SOURCE_WIKIPEDIA.to_string()],
        links: page
            .map(|url| InfoLink {
                label: "Wikipedia".to_string(),
                url,
            })
            .into_iter()
            .collect(),
        ..MediaInfo::default()
    })
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/wiki.rs"]
mod tests;
