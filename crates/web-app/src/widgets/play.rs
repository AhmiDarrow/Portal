//! What a page asks the player to play (player.ts `PlayTarget`), built the way main.ts
//! builds it (`movieTarget`, `playTitle`, `playEpisode`, `openItem`), and the one place pages
//! hand it over: [`play`]. The player dock (plan P7) takes it from there.

use portal_api::{Episode, MediaInfo, PlayKind, TitleDetail, TitleKind};

use crate::widgets::titles::{episode_line, resume_of};
use crate::Ctx;

/// player.ts `PlayTarget`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayTarget {
    pub kind: PlayKind,
    pub id: String,
    pub file: String,
    pub title: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub art: Option<String>,
    pub show_title: Option<String>,
    pub show_id: Option<String>,
    /// What browse knew about the title, so Info opens at once.
    pub info: Option<MediaInfo>,
    pub resume_seconds: Option<f64>,
    /// Show the add-on's source list instead of resolving one.
    pub manual_links: bool,
    pub progress_kind: TitleKind,
    pub progress_id: String,
    pub progress_name: String,
    pub track_progress: bool,
}

/// main.ts `movieTarget`.
pub fn movie_target(id: &str, file: &str, name: &str, art: Option<String>, resume: Option<f64>) -> PlayTarget {
    PlayTarget {
        kind: PlayKind::Movie,
        id: id.into(),
        file: file.into(),
        title: name.into(),
        season: None,
        episode: None,
        art,
        show_title: None,
        show_id: None,
        info: None,
        resume_seconds: resume,
        manual_links: false,
        progress_kind: TitleKind::Movie,
        progress_id: id.into(),
        progress_name: name.into(),
        track_progress: true,
    }
}

/// main.ts `playEpisode`.
pub fn episode_target(title: &TitleDetail, e: &Episode, from_start: bool) -> PlayTarget {
    let line = episode_line(Some(e.season), Some(e.episode), Some(&e.name));
    PlayTarget {
        kind: PlayKind::Episode,
        id: title.id.clone(),
        file: e.path.clone(),
        title: if line.is_empty() { e.name.clone() } else { line },
        season: Some(e.season),
        episode: Some(e.episode),
        art: e.art.clone().or(title.art.clone()),
        show_title: Some(title.name.clone()),
        show_id: Some(title.id.clone()),
        info: None,
        resume_seconds: if from_start || !e.resumable { None } else { resume_of(Some(e.position)) },
        manual_links: false,
        progress_kind: TitleKind::Tv,
        progress_id: title.id.clone(),
        progress_name: title.name.clone(),
        track_progress: true,
    }
}

/// main.ts `playTitle`: a movie from its file, a show from the episode it is up to.
pub fn title_target(title: &TitleDetail, from_start: bool) -> Option<PlayTarget> {
    if title.kind == TitleKind::Movie {
        let file = title.file.clone().unwrap_or_else(|| title.id.clone());
        let resume = if from_start { None } else { resume_of(title.resume_seconds) };
        return Some(movie_target(&title.id, &file, &title.name, title.art.clone(), resume));
    }
    let next = title.next.as_ref()?;
    let found = title.seasons.iter().flat_map(|s| &s.episodes).find(|e| e.path == next.path).cloned();
    let e = found.unwrap_or_else(|| Episode {
        season: next.season,
        episode: next.episode,
        name: next.name.clone(),
        path: next.path.clone(),
        art: next.art.clone(),
        position: 0.0,
        duration: 0.0,
        finished: false,
        resumable: false,
    });
    Some(episode_target(title, &e, from_start))
}

/// Hands `target` to the player. Until the player dock is ported (plan P7) this is where it
/// will pick it up; pages already build every target as main.ts does.
pub fn play(ctx: &Ctx, target: PlayTarget) {
    let _ = (ctx, target);
}
