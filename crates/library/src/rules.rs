//! Watch rules: when a title counts as watched, what resumes, and what plays next.
//!
//! Title ids are add-on paths. A progress key peels `:season:episode` only when
//! the episode is at least 1 and both trailing segments are integers, so season
//! 0 (a special) is its own key and a colon inside `plugin://` stays part of
//! the path. Episode 0 stays on the show key.

use portal_api::TitleKind;
use std::collections::BTreeMap;

pub const WATCHED_AT: f64 = portal_api::WATCHED_AT;
pub const RESUME_AFTER: f64 = portal_api::RESUME_AFTER_SECONDS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpisodeNum {
    pub season: u32,
    pub episode: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastPlayed {
    pub season: u32,
    pub episode: u32,
    pub watched: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeasonCount {
    pub number: u32,
    pub episodes: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParsedKey {
    Movie {
        id: String,
    },
    Show {
        id: String,
    },
    Episode {
        id: String,
        season: u32,
        episode: u32,
    },
}

impl ParsedKey {
    pub fn title_id(&self) -> &str {
        match self {
            Self::Movie { id } | Self::Show { id } | Self::Episode { id, .. } => id,
        }
    }

    pub fn title_key(&self) -> String {
        match self {
            Self::Movie { id } => title_key(TitleKind::Movie, id),
            Self::Show { id } | Self::Episode { id, .. } => title_key(TitleKind::Tv, id),
        }
    }

    pub fn kind(&self) -> TitleKind {
        match self {
            Self::Movie { .. } => TitleKind::Movie,
            Self::Show { .. } | Self::Episode { .. } => TitleKind::Tv,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProgressMark {
    pub position: f64,
    pub duration: f64,
    pub at: i64,
    pub watched: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContinuePick {
    pub progress_key: String,
    pub title_key: String,
    pub season: u32,
    pub episode: u32,
    /// Set when the card is the episode still in progress. Empty when the card
    /// is the next episode, which has not started.
    pub progress: Option<ProgressMark>,
}

pub fn finished(position: f64, duration: f64) -> bool {
    duration.is_finite()
        && position.is_finite()
        && duration > 0.0
        && position >= duration * WATCHED_AT
}

pub fn resumable(position: f64, duration: f64) -> bool {
    position.is_finite()
        && duration.is_finite()
        && duration > 0.0
        && position > RESUME_AFTER
        && !finished(position, duration)
}

pub fn fraction(position: f64, duration: f64, watched: bool) -> f64 {
    if watched {
        return 1.0;
    }
    if duration.is_finite() && duration > 0.0 && position.is_finite() {
        return (position / duration).clamp(0.0, 1.0);
    }
    0.0
}

pub fn title_key(kind: TitleKind, id: &str) -> String {
    match kind {
        TitleKind::Movie => format!("movie:{id}"),
        TitleKind::Tv => format!("tv:{id}"),
    }
}

pub fn progress_key(kind: TitleKind, id: &str, season: u32, episode: u32) -> String {
    if kind == TitleKind::Tv && episode > 0 {
        format!("tv:{id}:{season}:{episode}")
    } else {
        title_key(kind, id)
    }
}

pub fn parse_key(key: &str) -> Option<ParsedKey> {
    if let Some(id) = key.strip_prefix("movie:") {
        if id.is_empty() {
            return None;
        }
        return Some(ParsedKey::Movie { id: id.to_string() });
    }
    let rest = key.strip_prefix("tv:")?;
    if let Some((id, season, episode)) = peel_episode(rest) {
        return Some(ParsedKey::Episode {
            id,
            season,
            episode,
        });
    }
    if rest.is_empty() {
        return None;
    }
    Some(ParsedKey::Show {
        id: rest.to_string(),
    })
}

fn peel_episode(rest: &str) -> Option<(String, u32, u32)> {
    let (head, episode) = rest.rsplit_once(':')?;
    let episode: u32 = episode.parse().ok()?;
    let (id, season) = head.rsplit_once(':')?;
    let season: u32 = season.parse().ok()?;
    if episode == 0 || id.is_empty() {
        return None;
    }
    Some((id.to_string(), season, episode))
}

/// The first episode of a show that has not been started. Specials (season 0)
/// wait until a regular season is finished, unless they are all the show has.
fn opening_episode(seasons: &[SeasonCount]) -> Option<EpisodeNum> {
    let season = seasons
        .iter()
        .find(|season| season.number > 0)
        .or_else(|| seasons.first())?;
    Some(EpisodeNum {
        season: season.number,
        episode: 1,
    })
}

/// Where a show picks up. `seasons` is in watch order. Each `episodes` value
/// is how many episodes that season has, numbered from 1.
pub fn next_episode(seasons: &[SeasonCount], last: Option<LastPlayed>) -> Option<EpisodeNum> {
    let Some(last) = last else {
        return opening_episode(seasons);
    };
    if !last.watched {
        return Some(EpisodeNum {
            season: last.season,
            episode: last.episode,
        });
    }
    let index = seasons
        .iter()
        .position(|season| season.number == last.season)?;
    let season = &seasons[index];
    if last.episode < season.episodes {
        return Some(EpisodeNum {
            season: last.season,
            episode: last.episode + 1,
        });
    }
    seasons.get(index + 1).map(|season| EpisodeNum {
        season: season.number,
        episode: 1,
    })
}

/// One card per title, newest first. The same timestamp yields the later
/// season and episode.
pub fn continuing(
    progress: &BTreeMap<String, ProgressMark>,
    seasons_of: impl Fn(&str) -> Vec<SeasonCount>,
) -> Vec<ContinuePick> {
    let mut entries: Vec<(&String, &ProgressMark)> = progress.iter().collect();
    entries.sort_by(|(a_key, a), (b_key, b)| {
        b.at.cmp(&a.at)
            .then_with(|| episode_rank(b_key).cmp(&episode_rank(a_key)))
    });
    let mut seen = std::collections::BTreeSet::new();
    let mut cards = Vec::new();
    for (key, mark) in entries {
        let Some(parsed) = parse_key(key) else {
            continue;
        };
        let title = parsed.title_key();
        if seen.contains(&title) {
            continue;
        }
        if resumable(mark.position, mark.duration) {
            seen.insert(title.clone());
            let (season, episode) = match &parsed {
                ParsedKey::Episode {
                    season, episode, ..
                } => (*season, *episode),
                _ => (0, 0),
            };
            cards.push(ContinuePick {
                progress_key: key.clone(),
                title_key: title,
                season,
                episode,
                progress: Some(mark.clone()),
            });
            continue;
        }
        let ParsedKey::Episode {
            id,
            season,
            episode,
        } = &parsed
        else {
            continue;
        };
        if !mark.watched {
            continue;
        }
        // A finished special leaves the show where it was; an older regular
        // episode further down this list says where that is.
        if *season == 0 && has_regular_place(progress, id) {
            continue;
        }
        let up = next_episode(
            &seasons_of(id),
            Some(LastPlayed {
                season: *season,
                episode: *episode,
                watched: true,
            }),
        );
        let Some(up) = up else {
            continue;
        };
        let next_key = progress_key(TitleKind::Tv, id, up.season, up.episode);
        if progress.get(&next_key).is_some_and(|item| item.watched) {
            continue;
        }
        seen.insert(title.clone());
        cards.push(ContinuePick {
            progress_key: next_key,
            title_key: title,
            season: up.season,
            episode: up.episode,
            progress: None,
        });
    }
    cards
}

/// Whether a regular episode of the show is watched or part way through, so
/// it gives the show a place of its own. A barely started one does not.
fn has_regular_place(progress: &BTreeMap<String, ProgressMark>, show_id: &str) -> bool {
    progress.iter().any(|(key, mark)| {
        (mark.watched || resumable(mark.position, mark.duration))
            && matches!(
                parse_key(key),
                Some(ParsedKey::Episode { id, season, .. }) if id == show_id && season > 0
            )
    })
}

fn episode_rank(key: &str) -> (u32, u32) {
    match parse_key(key) {
        Some(ParsedKey::Episode {
            season, episode, ..
        }) => (season, episode),
        _ => (0, 0),
    }
}

#[cfg(test)]
#[path = "../../../tests-private/library/src/rules.rs"]
mod tests;
