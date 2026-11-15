//! TMDB, with the user's own key. A 32-character hex key is a v3 API key and
//! goes in the `api_key` query parameter; anything longer (a v4 read access
//! token) goes in an `Authorization: Bearer` header.

use portal_api::{InfoLink, MediaInfo, MediaKind, CAST_LIMIT};
use portal_engine::kodi_info::{date_prefix, rating, year_of_date};
use reqwest::StatusCode;
use serde_json::Value;

use super::merge::cast_member;
use super::net::{self, Failed};
use super::text::{digits_at, names_at, norm_title, plot_at, str_at, year_close};
use super::{Query, SOURCE_TMDB};

const API: &str = "https://api.themoviedb.org/3";
const POSTER: &str = "https://image.tmdb.org/t/p/w500";
const FANART: &str = "https://image.tmdb.org/t/p/w1280";
const FACE: &str = "https://image.tmdb.org/t/p/w185";
const CREW_LIMIT: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// v3 API key: 32 hex digits.
    ApiKey,
    /// v4 read access token (a JWT).
    Bearer,
}

/// v3 keys are 32 hex digits; v4 tokens are JWTs, far longer and dotted.
pub fn key_kind(key: &str) -> KeyKind {
    if key.len() > 40 || key.contains('.') {
        KeyKind::Bearer
    } else {
        KeyKind::ApiKey
    }
}

/// Printable ASCII without spaces, as both key forms are.
pub fn plausible_key(key: &str) -> bool {
    (16..=1024).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic())
}

pub struct Tmdb<'a> {
    client: &'a reqwest::Client,
    key: &'a str,
}

impl<'a> Tmdb<'a> {
    pub fn new(client: &'a reqwest::Client, key: &'a str) -> Self {
        Self { client, key }
    }

    fn request(
        &self,
        path: &str,
        params: &[(&str, &str)],
    ) -> Result<reqwest::RequestBuilder, Failed> {
        let mut all: Vec<(&str, &str)> = params.to_vec();
        let kind = key_kind(self.key);
        if kind == KeyKind::ApiKey {
            all.push(("api_key", self.key));
        }
        let url = net::url(&format!("{API}{path}"), &all)?;
        let request = self.client.get(url).header("accept", "application/json");
        Ok(match kind {
            KeyKind::ApiKey => request,
            KeyKind::Bearer => request.bearer_auth(self.key),
        })
    }

    async fn get(&self, path: &str, params: &[(&str, &str)]) -> Result<Option<Value>, Failed> {
        net::get_json(self.request(path, params)?).await
    }

    /// `Ok(true)` when TMDB accepts the key, `Ok(false)` when it refuses it.
    pub async fn check(&self) -> Result<bool, Failed> {
        let (status, _) = net::fetch(self.request("/configuration", &[])?).await?;
        match status {
            status if status.is_success() => Ok(true),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Ok(false),
            _ => Err(Failed),
        }
    }

    pub async fn lookup(&self, query: &Query) -> Result<Option<MediaInfo>, Failed> {
        let Some(target) = self.resolve(query).await? else {
            return Ok(None);
        };
        match target {
            Target::Movie(id) => Ok(self
                .get(
                    &format!("/movie/{id}"),
                    &[(
                        "append_to_response",
                        "credits,external_ids,videos,release_dates",
                    )],
                )
                .await?
                .map(|movie| parse_movie(&movie))),
            Target::Tv(id) => Ok(self
                .get(
                    &format!("/tv/{id}"),
                    &[(
                        "append_to_response",
                        "credits,external_ids,videos,content_ratings",
                    )],
                )
                .await?
                .map(|show| parse_tv(&show))),
            Target::Episode {
                show,
                season,
                episode,
            } => {
                let show_path = format!("/tv/{show}");
                let episode_path = format!("/tv/{show}/season/{season}/episode/{episode}");
                let (show_json, episode_json) = tokio::join!(
                    self.get(
                        &show_path,
                        &[("append_to_response", "external_ids,content_ratings")]
                    ),
                    self.get(
                        &episode_path,
                        &[("append_to_response", "credits,external_ids,videos")]
                    ),
                );
                let show_json = show_json?;
                let episode_json = episode_json?;
                if show_json.is_none() && episode_json.is_none() {
                    return Ok(None);
                }
                Ok(Some(parse_episode(
                    show,
                    season,
                    episode,
                    show_json.as_ref(),
                    episode_json.as_ref(),
                )))
            }
        }
    }

    async fn resolve(&self, query: &Query) -> Result<Option<Target>, Failed> {
        let kind = query.kind;
        if kind != Some(MediaKind::Episode) {
            if let Some(id) = query.tmdb_id.as_deref().and_then(|id| id.parse().ok()) {
                match kind {
                    Some(MediaKind::Movie) => return Ok(Some(Target::Movie(id))),
                    Some(MediaKind::Tv) => return Ok(Some(Target::Tv(id))),
                    _ => {}
                }
            }
        }
        let external = [
            (query.imdb_id.as_deref(), "imdb_id"),
            (query.tvdb_id.as_deref(), "tvdb_id"),
        ];
        for (id, source) in external {
            let Some(id) = id else { continue };
            let path = format!("/find/{id}");
            if let Some(found) = self.get(&path, &[("external_source", source)]).await? {
                if let Some(target) = pick_found(&found, query) {
                    return Ok(Some(target));
                }
            }
        }
        let Some(title) = query.title.as_deref() else {
            return Ok(None);
        };
        let (path, date_key) = match kind {
            Some(MediaKind::Movie) => ("/search/movie", "release_date"),
            Some(MediaKind::Tv) | Some(MediaKind::Episode) => ("/search/tv", "first_air_date"),
            None => return Ok(None),
        };
        let Some(results) = self
            .get(path, &[("query", title), ("include_adult", "false")])
            .await?
        else {
            return Ok(None);
        };
        let Some(id) = pick_search(&results, title, query.year, date_key) else {
            return Ok(None);
        };
        Ok(match kind {
            Some(MediaKind::Movie) => Some(Target::Movie(id)),
            Some(MediaKind::Tv) => Some(Target::Tv(id)),
            _ => query
                .episode_numbers()
                .map(|(season, episode)| Target::Episode {
                    show: id,
                    season,
                    episode,
                }),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Movie(u64),
    Tv(u64),
    Episode {
        show: u64,
        season: u32,
        episode: u32,
    },
}

/// The match in a `/find` answer that suits what was asked.
pub fn pick_found(found: &Value, query: &Query) -> Option<Target> {
    let first_id = |key: &str| found.get(key)?.as_array()?.first()?.get("id")?.as_u64();
    let episode = || {
        let row = found.get("tv_episode_results")?.as_array()?.first()?;
        let show = row.get("show_id")?.as_u64()?;
        let season = u32::try_from(row.get("season_number")?.as_u64()?).ok()?;
        let episode = u32::try_from(row.get("episode_number")?.as_u64()?).ok()?;
        Some(Target::Episode {
            show,
            season,
            episode,
        })
    };
    match query.kind {
        Some(MediaKind::Movie) => first_id("movie_results").map(Target::Movie),
        Some(MediaKind::Tv) => first_id("tv_results").map(Target::Tv),
        Some(MediaKind::Episode) => {
            // The page's numbers win. The id may be the show's or the
            // episode's; an episode match is read first because a TheTVDB
            // episode id can equal some other show's id.
            let show = match episode() {
                Some(Target::Episode { show, .. }) => Some(show),
                _ => first_id("tv_results"),
            }?;
            let (season, number) = query.episode_numbers().or_else(|| match episode() {
                Some(Target::Episode {
                    season, episode, ..
                }) => Some((season, episode)),
                _ => None,
            })?;
            Some(Target::Episode {
                show,
                season,
                episode: number,
            })
        }
        None => first_id("movie_results")
            .map(Target::Movie)
            .or_else(|| first_id("tv_results").map(Target::Tv))
            .or_else(episode),
    }
}

/// The first search result whose title matches and whose year is within one.
pub fn pick_search(results: &Value, title: &str, year: Option<u32>, date_key: &str) -> Option<u64> {
    let wanted = norm_title(title);
    if wanted.is_empty() {
        return None;
    }
    let rows = results.get("results")?.as_array()?;
    let matches = |row: &&Value| {
        let names = ["title", "original_title", "name", "original_name"];
        names
            .iter()
            .filter_map(|key| row.get(*key).and_then(Value::as_str))
            .any(|name| norm_title(name) == wanted)
    };
    let found_year = |row: &Value| {
        row.get(date_key)
            .and_then(Value::as_str)
            .and_then(year_of_date)
    };
    let candidates: Vec<&Value> = rows.iter().take(10).filter(matches).collect();
    let exact = candidates
        .iter()
        .find(|row| year.is_some() && found_year(row) == year);
    let close = || {
        candidates
            .iter()
            .find(|row| year_close(year, found_year(row)))
    };
    exact.or_else(close)?.get("id")?.as_u64()
}

pub fn parse_movie(movie: &Value) -> MediaInfo {
    let id = digits_at(movie, "id");
    let premiered = str_at(movie, "release_date").and_then(|date| date_prefix(&date));
    let external = movie.get("external_ids").unwrap_or(&Value::Null);
    MediaInfo {
        kind: Some(MediaKind::Movie),
        title: str_at(movie, "title"),
        original_title: str_at(movie, "original_title"),
        year: premiered.as_deref().and_then(year_of_date),
        premiered,
        plot: plot_at(movie, "overview"),
        tagline: str_at(movie, "tagline"),
        genres: names_at(movie, "genres"),
        runtime_minutes: minutes(movie.get("runtime")),
        mpaa: us_movie_rating(movie),
        rating: movie
            .get("vote_average")
            .and_then(Value::as_f64)
            .and_then(rating),
        votes: votes(movie),
        studios: names_at(movie, "production_companies"),
        directors: crew(movie, |row| job(row) == Some("Director")),
        writers: crew(movie, |row| department(row) == Some("Writing")),
        countries: names_at(movie, "production_countries"),
        cast: cast(movie, &["cast"]),
        imdb_id: str_at(movie, "imdb_id")
            .or_else(|| str_at(external, "imdb_id"))
            .and_then(|id| portal_api::imdb_id(&id)),
        tvdb_id: None,
        trailer: trailer(movie),
        poster: image(movie, "poster_path", POSTER),
        fanart: image(movie, "backdrop_path", FANART),
        sources: vec![SOURCE_TMDB.to_string()],
        links: id
            .as_ref()
            .map(|id| page_link(format!("https://www.themoviedb.org/movie/{id}")))
            .into_iter()
            .collect(),
        tmdb_id: id,
        ..MediaInfo::default()
    }
}

pub fn parse_tv(show: &Value) -> MediaInfo {
    let id = digits_at(show, "id");
    let premiered = str_at(show, "first_air_date").and_then(|date| date_prefix(&date));
    let external = show.get("external_ids").unwrap_or(&Value::Null);
    let mut studios = names_at(show, "networks");
    for studio in names_at(show, "production_companies") {
        if !studios.contains(&studio) {
            studios.push(studio);
        }
    }
    MediaInfo {
        kind: Some(MediaKind::Tv),
        title: str_at(show, "name"),
        original_title: str_at(show, "original_name"),
        year: premiered.as_deref().and_then(year_of_date),
        premiered,
        plot: plot_at(show, "overview"),
        tagline: str_at(show, "tagline"),
        genres: names_at(show, "genres"),
        runtime_minutes: show
            .get("episode_run_time")
            .and_then(Value::as_array)
            .and_then(|times| times.first())
            .and_then(|time| minutes(Some(time))),
        mpaa: us_tv_rating(show),
        rating: show
            .get("vote_average")
            .and_then(Value::as_f64)
            .and_then(rating),
        votes: votes(show),
        studios,
        writers: names_at(show, "created_by"),
        countries: names_at(show, "production_countries"),
        cast: cast(show, &["cast"]),
        imdb_id: str_at(external, "imdb_id").and_then(|id| portal_api::imdb_id(&id)),
        tvdb_id: digits_at(external, "tvdb_id"),
        trailer: trailer(show),
        poster: image(show, "poster_path", POSTER),
        fanart: image(show, "backdrop_path", FANART),
        sources: vec![SOURCE_TMDB.to_string()],
        links: id
            .as_ref()
            .map(|id| page_link(format!("https://www.themoviedb.org/tv/{id}")))
            .into_iter()
            .collect(),
        tmdb_id: id,
        ..MediaInfo::default()
    }
}

/// An episode, with the show's genres, network, rating and artwork.
/// `tmdb_id` stays empty: TMDB's episode ids are not what add-ons store.
pub fn parse_episode(
    show_id: u64,
    season: u32,
    number: u32,
    show: Option<&Value>,
    episode: Option<&Value>,
) -> MediaInfo {
    let show_info = show.map(parse_tv).unwrap_or_default();
    let empty = Value::Null;
    let row = episode.unwrap_or(&empty);
    let external = row.get("external_ids").unwrap_or(&Value::Null);
    let premiered = str_at(row, "air_date").and_then(|date| date_prefix(&date));
    let mut cast_list = cast(row, &["cast", "guest_stars"]);
    if cast_list.is_empty() {
        cast_list = show_info.cast.clone();
    }
    let link = if episode.is_some() {
        format!("https://www.themoviedb.org/tv/{show_id}/season/{season}/episode/{number}")
    } else {
        format!("https://www.themoviedb.org/tv/{show_id}")
    };
    MediaInfo {
        kind: Some(MediaKind::Episode),
        title: str_at(row, "name"),
        show_title: show_info.title.clone(),
        season: Some(season),
        episode: Some(number),
        year: premiered.as_deref().and_then(year_of_date),
        premiered,
        plot: plot_at(row, "overview"),
        genres: show_info.genres,
        runtime_minutes: minutes(row.get("runtime")),
        mpaa: show_info.mpaa,
        rating: row
            .get("vote_average")
            .and_then(Value::as_f64)
            .and_then(rating),
        votes: votes(row),
        studios: show_info.studios,
        directors: crew(row, |crew| job(crew) == Some("Director")),
        writers: crew(row, |crew| department(crew) == Some("Writing")),
        countries: show_info.countries,
        cast: cast_list,
        imdb_id: str_at(external, "imdb_id").and_then(|id| portal_api::imdb_id(&id)),
        tvdb_id: digits_at(external, "tvdb_id"),
        trailer: trailer(row),
        poster: show_info.poster,
        fanart: image(row, "still_path", FANART).or(show_info.fanart),
        sources: vec![SOURCE_TMDB.to_string()],
        links: vec![page_link(link)],
        ..MediaInfo::default()
    }
}

fn page_link(url: String) -> InfoLink {
    InfoLink {
        label: "TMDB".to_string(),
        url,
    }
}

fn minutes(value: Option<&Value>) -> Option<u32> {
    value
        .and_then(Value::as_u64)
        .filter(|minutes| *minutes > 0)
        .and_then(|minutes| u32::try_from(minutes).ok())
}

fn votes(row: &Value) -> Option<u64> {
    row.get("vote_count")
        .and_then(Value::as_u64)
        .filter(|votes| *votes > 0)
}

fn image(row: &Value, key: &str, base: &str) -> Option<String> {
    let path = str_at(row, key)?;
    path.starts_with('/').then(|| format!("{base}{path}"))
}

fn job(row: &Value) -> Option<&str> {
    row.get("job").and_then(Value::as_str)
}

fn department(row: &Value) -> Option<&str> {
    row.get("department").and_then(Value::as_str)
}

/// Crew names from `credits.crew` (or a bare `crew`) that pass `keep`.
fn crew(row: &Value, keep: impl Fn(&Value) -> bool) -> Vec<String> {
    let rows = row
        .get("credits")
        .and_then(|credits| credits.get("crew"))
        .or_else(|| row.get("crew"))
        .and_then(Value::as_array);
    let mut out: Vec<String> = Vec::new();
    for member in rows.into_iter().flatten().filter(|member| keep(member)) {
        if let Some(name) = str_at(member, "name") {
            if !out.contains(&name) {
                out.push(name);
            }
        }
        if out.len() == CREW_LIMIT {
            break;
        }
    }
    out
}

/// Cast from `credits.<key>` for each key in turn, in billing order.
fn cast(row: &Value, keys: &[&str]) -> Vec<portal_api::CastMember> {
    let credits = row.get("credits");
    let mut rows: Vec<&Value> = Vec::new();
    for key in keys {
        let list = credits
            .and_then(|credits| credits.get(*key))
            .or_else(|| row.get(*key))
            .and_then(Value::as_array);
        let mut part: Vec<&Value> = list.into_iter().flatten().collect();
        part.sort_by_key(|member| {
            member
                .get("order")
                .and_then(Value::as_u64)
                .unwrap_or(u64::MAX)
        });
        rows.extend(part);
    }
    let mut out: Vec<portal_api::CastMember> = Vec::new();
    for member in rows {
        let Some(member) = cast_member(
            str_at(member, "name"),
            str_at(member, "character"),
            image(member, "profile_path", FACE),
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

/// A YouTube trailer, official ones first.
fn trailer(row: &Value) -> Option<String> {
    let videos = row.get("videos")?.get("results")?.as_array()?;
    let usable = |video: &&Value| {
        video.get("site").and_then(Value::as_str) == Some("YouTube")
            && video.get("type").and_then(Value::as_str) == Some("Trailer")
            && video.get("key").and_then(Value::as_str).is_some_and(|key| {
                !key.is_empty()
                    && key.len() <= 32
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
            })
    };
    let official = |video: &&Value| video.get("official").and_then(Value::as_bool) == Some(true);
    let pick = videos
        .iter()
        .filter(usable)
        .find(official)
        .or_else(|| videos.iter().find(usable))?;
    let key = pick.get("key")?.as_str()?;
    Some(format!("https://www.youtube.com/watch?v={key}"))
}

fn us_movie_rating(movie: &Value) -> Option<String> {
    movie
        .get("release_dates")?
        .get("results")?
        .as_array()?
        .iter()
        .find(|row| row.get("iso_3166_1").and_then(Value::as_str) == Some("US"))?
        .get("release_dates")?
        .as_array()?
        .iter()
        .find_map(|date| str_at(date, "certification"))
}

fn us_tv_rating(show: &Value) -> Option<String> {
    show.get("content_ratings")?
        .get("results")?
        .as_array()?
        .iter()
        .find(|row| row.get("iso_3166_1").and_then(Value::as_str) == Some("US"))
        .and_then(|row| str_at(row, "rating"))
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/tmdb.rs"]
mod tests;
