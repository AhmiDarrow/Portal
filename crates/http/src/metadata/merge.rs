//! Cleans the page's seed and merges it with what the sources found.
//!
//! Identity fields (kind, title, show title, season, episode, year, ids) come
//! from the seed when it has them: the page knows which item was picked.
//! Descriptive fields come from the richest online source that has them, then
//! the seed, then Wikipedia (whose lead paragraph and image are a fallback,
//! not a better plot than the add-on's). Artwork comes from the seed first:
//! the add-on's matches its cards and is already sized for the page.

use std::cmp::Reverse;

use portal_api::{CastMember, InfoLink, MediaInfo, MediaKind, CAST_LIMIT};
use portal_engine::kodi_info::{
    all_digits, date_prefix, imdb_link, is_http, plausible_year, rating,
};

use super::text::{clip, PLOT_LIMIT};
use super::{SOURCE_TMDB, SOURCE_TVMAZE, SOURCE_WIKIPEDIA};

const ONLINE: [&str; 3] = [SOURCE_TMDB, SOURCE_TVMAZE, SOURCE_WIKIPEDIA];

const TEXT_LIMIT: usize = 300;
const LIST_LIMIT: usize = 20;
const LINK_LIMIT: usize = 12;

/// Drops malformed values from what the page sent, and works out an episode
/// from its numbers when the page left out the kind.
pub fn normalize_seed(mut seed: MediaInfo) -> MediaInfo {
    seed.title = short_text(seed.title);
    seed.original_title = short_text(seed.original_title);
    seed.show_title = short_text(seed.show_title);
    seed.year = seed.year.and_then(|year| plausible_year(i64::from(year)));
    seed.premiered = seed.premiered.as_deref().and_then(date_prefix);
    seed.plot = long_text(seed.plot);
    seed.tagline = short_text(seed.tagline);
    seed.mpaa = short_text(seed.mpaa);
    seed.rating = seed.rating.and_then(rating);
    seed.votes = seed.votes.filter(|votes| *votes > 0);
    seed.runtime_minutes = seed.runtime_minutes.filter(|minutes| *minutes > 0);
    for list in [
        &mut seed.genres,
        &mut seed.studios,
        &mut seed.directors,
        &mut seed.writers,
        &mut seed.countries,
    ] {
        *list = clean_list(std::mem::take(list));
    }
    seed.cast.retain(|member| !member.name.trim().is_empty());
    seed.cast.truncate(CAST_LIMIT);
    seed.imdb_id = seed.imdb_id.as_deref().and_then(portal_api::imdb_id);
    seed.tmdb_id = digits(seed.tmdb_id);
    seed.tvdb_id = digits(seed.tvdb_id);
    seed.trailer = seed.trailer.filter(|url| is_http(url));
    seed.poster = short_url(seed.poster);
    seed.fanart = short_url(seed.fanart);
    // Online sources are named by what this lookup finds, not by the page.
    seed.sources = clean_list(std::mem::take(&mut seed.sources));
    seed.sources
        .retain(|source| !ONLINE.contains(&source.as_str()));
    seed.links = clean_links(std::mem::take(&mut seed.links));
    if seed.kind.is_none()
        && seed.season.is_some()
        && seed.episode.is_some()
        && seed.show_title.is_some()
    {
        seed.kind = Some(MediaKind::Episode);
    }
    seed
}

/// The seed with what the sources found. Each found `MediaInfo` names its
/// source in `sources`.
pub fn merge(seed: MediaInfo, found: Vec<MediaInfo>) -> MediaInfo {
    let (weak, mut strong): (Vec<MediaInfo>, Vec<MediaInfo>) = found.into_iter().partition(is_weak);
    // Stable: equally rich sources keep the order they were asked in.
    strong.sort_by_key(|info| Reverse(richness(info)));

    let identity: Vec<&MediaInfo> = std::iter::once(&seed)
        .chain(strong.iter())
        .chain(weak.iter())
        .collect();
    let described: Vec<&MediaInfo> = strong
        .iter()
        .chain(std::iter::once(&seed))
        .chain(weak.iter())
        .collect();

    fn first<T: Clone>(order: &[&MediaInfo], get: impl Fn(&MediaInfo) -> &Option<T>) -> Option<T> {
        order.iter().find_map(|info| get(info).clone())
    }
    fn list<T: Clone>(order: &[&MediaInfo], get: impl Fn(&MediaInfo) -> &Vec<T>) -> Vec<T> {
        order
            .iter()
            .map(|info| get(info))
            .find(|list| !list.is_empty())
            .cloned()
            .unwrap_or_default()
    }

    let title = first(&identity, |info| &info.title);
    let rated = described.iter().find(|info| info.rating.is_some());
    let mut merged = MediaInfo {
        kind: first(&identity, |info| &info.kind),
        original_title: first(&described, |info| &info.original_title).filter(|original| {
            title
                .as_deref()
                .is_none_or(|title| !title.eq_ignore_ascii_case(original))
        }),
        title,
        show_title: first(&identity, |info| &info.show_title),
        season: first(&identity, |info| &info.season),
        episode: first(&identity, |info| &info.episode),
        year: first(&identity, |info| &info.year),
        premiered: first(&described, |info| &info.premiered),
        plot: first(&described, |info| &info.plot),
        tagline: first(&described, |info| &info.tagline),
        genres: list(&described, |info| &info.genres),
        runtime_minutes: first(&described, |info| &info.runtime_minutes),
        mpaa: first(&described, |info| &info.mpaa),
        rating: rated.and_then(|info| info.rating),
        votes: match rated {
            Some(info) => info.votes,
            None => first(&described, |info| &info.votes),
        },
        studios: list(&described, |info| &info.studios),
        directors: list(&described, |info| &info.directors),
        writers: list(&described, |info| &info.writers),
        countries: list(&described, |info| &info.countries),
        cast: list(&described, |info| &info.cast),
        imdb_id: first(&identity, |info| &info.imdb_id),
        tmdb_id: first(&identity, |info| &info.tmdb_id),
        tvdb_id: first(&identity, |info| &info.tvdb_id),
        trailer: first(&described, |info| &info.trailer),
        // The add-on's artwork matches its cards and is sized for them;
        // online artwork only fills a gap.
        poster: first(&identity, |info| &info.poster),
        fanart: first(&identity, |info| &info.fanart),
        sources: Vec::new(),
        links: Vec::new(),
    };
    merged.cast.truncate(CAST_LIMIT);

    for source in identity.iter().flat_map(|info| info.sources.iter()) {
        if !merged.sources.contains(source) {
            merged.sources.push(source.clone());
        }
    }
    if let Some(id) = &merged.imdb_id {
        merged.links.push(imdb_link(id));
    }
    let has_imdb = !merged.links.is_empty();
    for link in identity.iter().flat_map(|info| info.links.iter()) {
        if has_imdb && link.label == "IMDb" {
            continue;
        }
        if !merged.links.iter().any(|seen| seen.url == link.url) {
            merged.links.push(link.clone());
        }
    }
    merged.links.truncate(LINK_LIMIT);
    merged
}

fn is_weak(info: &MediaInfo) -> bool {
    info.sources.iter().any(|source| source == SOURCE_WIKIPEDIA)
}

/// How many descriptive fields a source filled.
fn richness(info: &MediaInfo) -> usize {
    [
        info.original_title.is_some(),
        info.premiered.is_some(),
        info.plot.is_some(),
        info.tagline.is_some(),
        !info.genres.is_empty(),
        info.runtime_minutes.is_some(),
        info.mpaa.is_some(),
        info.rating.is_some(),
        info.votes.is_some(),
        !info.studios.is_empty(),
        !info.directors.is_empty(),
        !info.writers.is_empty(),
        !info.countries.is_empty(),
        !info.cast.is_empty(),
        info.trailer.is_some(),
        info.poster.is_some(),
        info.fanart.is_some(),
    ]
    .into_iter()
    .filter(|filled| *filled)
    .count()
}

fn short_text(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| clip(trimmed, TEXT_LIMIT))
}

fn long_text(text: Option<String>) -> Option<String> {
    let text = text?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| clip(trimmed, PLOT_LIMIT))
}

fn short_url(url: Option<String>) -> Option<String> {
    url.map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty() && url.len() <= 2048)
}

fn digits(id: Option<String>) -> Option<String> {
    id.map(|id| id.trim().to_string())
        .filter(|id| all_digits(id))
}

fn clean_list(list: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let item = item.trim();
        if !item.is_empty() && !out.iter().any(|seen| seen == item) {
            out.push(clip(item, TEXT_LIMIT));
        }
        if out.len() == LIST_LIMIT {
            break;
        }
    }
    out
}

fn clean_links(links: Vec<InfoLink>) -> Vec<InfoLink> {
    links
        .into_iter()
        .filter(|link| {
            !link.label.trim().is_empty() && is_http(&link.url) && link.url.len() <= 2048
        })
        .take(LINK_LIMIT)
        .collect()
}

/// A cast member with a trimmed name, or `None`.
pub fn cast_member(
    name: Option<String>,
    role: Option<String>,
    thumb: Option<String>,
) -> Option<CastMember> {
    Some(CastMember {
        name: name?,
        role: role.filter(|role| !role.is_empty()),
        thumb: thumb.filter(|thumb| is_http(thumb)),
    })
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/merge.rs"]
mod tests;
