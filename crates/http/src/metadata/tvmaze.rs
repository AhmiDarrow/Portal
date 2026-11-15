//! TVmaze: shows and episodes, no key needed.

use portal_api::{InfoLink, MediaInfo, MediaKind, CAST_LIMIT};
use portal_engine::kodi_info::{date_prefix, is_http, rating, year_of_date};
use serde_json::Value;

use super::merge::cast_member;
use super::net::{self, Failed};
use super::text::{clip, digits_at, norm_title, str_at, strip_html, year_close, PLOT_LIMIT};
use super::{Query, SOURCE_TVMAZE};

const API: &str = "https://api.tvmaze.com";

pub async fn lookup(client: &reqwest::Client, query: &Query) -> Result<Option<MediaInfo>, Failed> {
    let Some(id) = find_show(client, query).await? else {
        return Ok(None);
    };
    let show_url = net::url(&format!("{API}/shows/{id}"), &[("embed", "cast")])?;
    let episode = match (query.kind, query.episode_numbers()) {
        (Some(MediaKind::Episode), Some((season, number))) => Some(net::url(
            &format!("{API}/shows/{id}/episodebynumber"),
            &[
                ("season", &season.to_string()),
                ("number", &number.to_string()),
            ],
        )?),
        _ => None,
    };
    let (show, episode) = tokio::join!(net::get_json(client.get(show_url)), async {
        match episode {
            Some(url) => net::get_json(client.get(url)).await,
            None => Ok(None),
        }
    });
    let Some(show) = show? else {
        return Ok(None);
    };
    Ok(Some(match (query.kind, episode?) {
        (Some(MediaKind::Episode), Some(episode)) => parse_episode(&show, &episode),
        (Some(MediaKind::Episode), None) => show_facts_for_episode(parse_show(&show)),
        _ => parse_show(&show),
    }))
}

/// The TVmaze show id: by IMDb id, then TheTVDB id, then (for a show or an
/// episode) the best title match. An episode's TheTVDB id is not a show id
/// and could name another show, so episodes skip it.
async fn find_show(client: &reqwest::Client, query: &Query) -> Result<Option<u64>, Failed> {
    let tvdb = query
        .tvdb_id
        .as_deref()
        .filter(|_| query.kind != Some(MediaKind::Episode));
    let external = [(query.imdb_id.as_deref(), "imdb"), (tvdb, "thetvdb")];
    for (id, key) in external {
        let Some(id) = id else { continue };
        let url = net::url(&format!("{API}/lookup/shows"), &[(key, id)])?;
        if let Some(show) = net::get_json(client.get(url)).await? {
            if let Some(id) = show.get("id").and_then(Value::as_u64) {
                return Ok(Some(id));
            }
        }
    }
    if query.kind.is_none() {
        return Ok(None);
    }
    let Some(title) = query.title.as_deref() else {
        return Ok(None);
    };
    let url = net::url(&format!("{API}/singlesearch/shows"), &[("q", title)])?;
    let Some(show) = net::get_json(client.get(url)).await? else {
        return Ok(None);
    };
    Ok(accept_search(&show, title, query.year))
}

/// A search hit only counts when its name matches and its first year is
/// within one of the year asked for.
pub fn accept_search(show: &Value, title: &str, year: Option<u32>) -> Option<u64> {
    let name = show.get("name").and_then(Value::as_str)?;
    if norm_title(name) != norm_title(title) || norm_title(title).is_empty() {
        return None;
    }
    let premiered = str_at(show, "premiered").and_then(|date| year_of_date(&date));
    if !year_close(year, premiered) {
        return None;
    }
    show.get("id").and_then(Value::as_u64)
}

pub fn parse_show(show: &Value) -> MediaInfo {
    let premiered = str_at(show, "premiered").and_then(|date| date_prefix(&date));
    let externals = show.get("externals").unwrap_or(&Value::Null);
    let channel = show
        .get("network")
        .filter(|network| network.is_object())
        .or_else(|| show.get("webChannel").filter(|channel| channel.is_object()));
    let studios: Vec<String> = channel
        .and_then(|channel| str_at(channel, "name"))
        .into_iter()
        .collect();
    let countries: Vec<String> = channel
        .and_then(|channel| channel.get("country"))
        .and_then(|country| str_at(country, "name"))
        .into_iter()
        .collect();
    let runtime = show
        .get("runtime")
        .and_then(Value::as_u64)
        .or_else(|| show.get("averageRuntime").and_then(Value::as_u64));
    MediaInfo {
        kind: Some(MediaKind::Tv),
        title: str_at(show, "name"),
        year: premiered.as_deref().and_then(year_of_date),
        premiered,
        plot: summary(show),
        genres: show
            .get("genres")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        runtime_minutes: runtime
            .filter(|minutes| *minutes > 0)
            .and_then(|minutes| u32::try_from(minutes).ok()),
        rating: show_rating(show),
        studios,
        countries,
        cast: cast(show),
        imdb_id: str_at(externals, "imdb").and_then(|id| portal_api::imdb_id(&id)),
        tvdb_id: digits_at(externals, "thetvdb"),
        poster: image(show),
        sources: vec![SOURCE_TVMAZE.to_string()],
        links: link(show),
        ..MediaInfo::default()
    }
}

pub fn parse_episode(show: &Value, episode: &Value) -> MediaInfo {
    let show_info = parse_show(show);
    let premiered = str_at(episode, "airdate").and_then(|date| date_prefix(&date));
    let number = |key: &str| {
        episode
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|number| u32::try_from(number).ok())
    };
    let mut links = link(episode);
    links.extend(show_info.links.iter().cloned().map(|mut show_link| {
        show_link.label = "TVmaze show".to_string();
        show_link
    }));
    MediaInfo {
        kind: Some(MediaKind::Episode),
        title: str_at(episode, "name"),
        show_title: show_info.title,
        season: number("season"),
        episode: number("number"),
        year: premiered.as_deref().and_then(year_of_date),
        premiered,
        plot: summary(episode),
        genres: show_info.genres,
        runtime_minutes: episode
            .get("runtime")
            .and_then(Value::as_u64)
            .filter(|minutes| *minutes > 0)
            .and_then(|minutes| u32::try_from(minutes).ok()),
        rating: show_rating(episode),
        studios: show_info.studios,
        countries: show_info.countries,
        cast: show_info.cast,
        poster: show_info.poster,
        fanart: image(episode),
        sources: vec![SOURCE_TVMAZE.to_string()],
        links,
        ..MediaInfo::default()
    }
}

/// Only what is true of the whole show, for an episode TVmaze does not list.
fn show_facts_for_episode(show: MediaInfo) -> MediaInfo {
    MediaInfo {
        kind: Some(MediaKind::Episode),
        show_title: show.title,
        genres: show.genres,
        studios: show.studios,
        countries: show.countries,
        cast: show.cast,
        poster: show.poster,
        sources: show.sources,
        links: show.links,
        ..MediaInfo::default()
    }
}

fn summary(row: &Value) -> Option<String> {
    let text = strip_html(row.get("summary")?.as_str()?);
    (!text.is_empty()).then(|| clip(&text, PLOT_LIMIT))
}

fn show_rating(row: &Value) -> Option<f64> {
    row.get("rating")?.get("average")?.as_f64().and_then(rating)
}

fn image(row: &Value) -> Option<String> {
    let image = row.get("image")?;
    str_at(image, "original")
        .or_else(|| str_at(image, "medium"))
        .filter(|url| is_http(url))
}

fn link(row: &Value) -> Vec<InfoLink> {
    str_at(row, "url")
        .filter(|url| is_http(url))
        .map(|url| InfoLink {
            label: "TVmaze".to_string(),
            url,
        })
        .into_iter()
        .collect()
}

fn cast(show: &Value) -> Vec<portal_api::CastMember> {
    let rows = show
        .get("_embedded")
        .and_then(|embedded| embedded.get("cast"))
        .and_then(Value::as_array);
    let mut out: Vec<portal_api::CastMember> = Vec::new();
    for row in rows.into_iter().flatten() {
        let person = row.get("person").unwrap_or(&Value::Null);
        let character = row.get("character").unwrap_or(&Value::Null);
        let Some(member) = cast_member(
            str_at(person, "name"),
            str_at(character, "name"),
            person
                .get("image")
                .and_then(|image| str_at(image, "medium")),
        ) else {
            continue;
        };
        if out.iter().any(|seen| seen.name == member.name) {
            continue;
        }
        out.push(member);
        if out.len() == CAST_LIMIT {
            break;
        }
    }
    out
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/tvmaze.rs"]
mod tests;
