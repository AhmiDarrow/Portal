//! Local library file. Watch state never goes into the Kodi profile.

use crate::rules::{
    self, finished, next_episode, progress_key, resumable, title_key, ContinuePick, EpisodeNum,
    LastPlayed, ParsedKey, ProgressMark, SeasonCount,
};
use portal_api::{
    BrowseItem, Card, Episode, EpisodeRef, HistoryEntry, ItemKind, NextItem, PlayKind, PlayRequest,
    ProgressReport, SavedState, Season, SeasonFolder, Source, TitleDetail, TitleKind,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_LIMIT: usize = 500;
/// Progress and remembered titles nothing else points at are dropped after
/// this long, so the library file does not grow forever.
const KEEP_UNUSED_MS: i64 = 180 * 24 * 60 * 60 * 1000;
/// At most this many remembered titles nothing else points at are kept.
const UNUSED_TITLE_LIMIT: usize = 5_000;
/// A steady playhead stays in memory and reaches the file on this cadence.
/// A pause, a stop, a seek, or finishing the title still writes at once.
const PROGRESS_KEEP_MS: i64 = 30_000;
const PROGRESS_KEEP_SECONDS: f64 = 30.0;

#[derive(Debug)]
pub enum StoreError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::Json(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            Self::Json(err) => Some(err),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TitleRecord {
    kind: TitleKind,
    id: String,
    name: String,
    #[serde(default)]
    art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    imdb_id: Option<String>,
    /// When the title was last remembered, in milliseconds. Zero in older files.
    #[serde(default)]
    seen: i64,
}

/// What a caller may know about a title besides its name and art. `None`
/// keeps what the library already has.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TitleFacts {
    pub year: Option<u32>,
    pub imdb_id: Option<String>,
}

impl TitleFacts {
    /// Keeps a plausible year and a well-formed IMDb id; drops anything else.
    pub fn new(year: Option<u32>, imdb_id: Option<&str>) -> Self {
        Self {
            year: year.filter(|year| (1870..=2200).contains(year)),
            imdb_id: imdb_id.and_then(portal_api::imdb_id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProgressRecord {
    position: f64,
    duration: f64,
    at: i64,
    watched: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EpisodeRecord {
    season: u32,
    episode: u32,
    name: String,
    path: String,
    #[serde(default)]
    art: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryRecord {
    id: String,
    kind: TitleKind,
    name: String,
    #[serde(default)]
    art: Option<String>,
    at: i64,
    position: f64,
    duration: f64,
    finished: bool,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
    #[serde(default)]
    file: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
struct SeasonLink {
    show_id: String,
    season: u32,
    #[serde(default)]
    show_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct FileDoc {
    #[serde(default)]
    titles: BTreeMap<String, TitleRecord>,
    #[serde(default)]
    progress: BTreeMap<String, ProgressRecord>,
    #[serde(default)]
    watchlist: BTreeMap<String, i64>,
    #[serde(default)]
    bookmarks: BTreeMap<String, i64>,
    #[serde(default)]
    history: Vec<HistoryRecord>,
    #[serde(default)]
    episodes: BTreeMap<String, Vec<EpisodeRecord>>,
    #[serde(default)]
    season_folders: BTreeMap<String, SeasonLink>,
    #[serde(default)]
    source: Option<Source>,
}

/// The playhead last written to disk, so a small nudge does not rewrite the file.
struct ProgressDisk {
    key: String,
    position: f64,
    duration: f64,
    watched: bool,
    at: i64,
}

pub struct Store {
    path: PathBuf,
    doc: FileDoc,
    progress_disk: Option<ProgressDisk>,
}

/// One Continue Watching card: TV or not, name, season, episode, a film's year.
type ContinueKey = (bool, String, Option<u32>, Option<u32>, Option<u32>);

impl Store {
    /// A file that no longer parses is set aside as `<name>.corrupt-<ms>.json`
    /// and the library starts empty, so the service still starts.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let path = path.as_ref().to_path_buf();
        let doc = if path.is_file() {
            let bytes = fs::read(&path).map_err(StoreError::Io)?;
            // A file saved in Notepad can start with a byte order mark.
            let body = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
            match serde_json::from_slice(body) {
                Ok(doc) => doc,
                Err(_) => {
                    let stem = path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .unwrap_or("library");
                    let aside = path.with_file_name(format!("{stem}.corrupt-{}.json", now_ms()));
                    fs::rename(&path, &aside).map_err(StoreError::Io)?;
                    eprintln!(
                        "library file did not parse; moved it to {} and started empty",
                        aside.display()
                    );
                    FileDoc::default()
                }
            }
        } else {
            FileDoc::default()
        };
        let mut store = Self {
            path,
            doc,
            progress_disk: None,
        };
        store.prune(now_ms());
        Ok(store)
    }

    /// Drops old progress and remembered titles that the watchlist, bookmarks
    /// and history do not point at. Those three lists are never touched. The
    /// result is written with the next save.
    fn prune(&mut self, now: i64) {
        let cutoff = now.saturating_sub(KEEP_UNUSED_MS);
        let mut used: BTreeSet<String> = self
            .doc
            .watchlist
            .keys()
            .chain(self.doc.bookmarks.keys())
            .cloned()
            .collect();
        used.extend(
            self.doc
                .history
                .iter()
                .map(|record| title_key(record.kind, &record.id)),
        );
        // A watched mark is kept however old: a show finished last year still
        // shows its episodes as watched. Only stale resume points go.
        self.doc.progress.retain(|key, record| {
            record.watched
                || record.at >= cutoff
                || rules::parse_key(key).is_none_or(|parsed| used.contains(&parsed.title_key()))
        });
        used.extend(
            self.doc
                .progress
                .keys()
                .filter_map(|key| rules::parse_key(key).map(|parsed| parsed.title_key())),
        );
        let mut unused: Vec<(i64, String)> = Vec::new();
        for (key, record) in self.doc.titles.iter_mut() {
            if used.contains(key) {
                continue;
            }
            // A title from an older file starts its clock now.
            if record.seen <= 0 {
                record.seen = now;
            }
            unused.push((record.seen, key.clone()));
        }
        unused.sort_by_key(|(seen, _)| std::cmp::Reverse(*seen));
        let dropped: BTreeSet<String> = unused
            .into_iter()
            .enumerate()
            .filter(|(rank, (seen, _))| *rank >= UNUSED_TITLE_LIMIT || *seen < cutoff)
            .map(|(_, (_, key))| key)
            .collect();
        if dropped.is_empty() {
            return;
        }
        self.doc.titles.retain(|key, _| !dropped.contains(key));
        self.doc.episodes.retain(|key, _| !dropped.contains(key));
        self.doc
            .season_folders
            .retain(|_, link| !dropped.contains(&title_key(TitleKind::Tv, &link.show_id)));
    }

    pub fn source(&self) -> Source {
        self.doc.source.clone().unwrap_or(Source {
            addon_id: None,
            name: None,
            path: None,
        })
    }

    pub fn set_source(&mut self, source: Source) -> Result<(), StoreError> {
        self.doc.source = Some(source);
        self.save()
    }

    /// Where a play should go once an add-on is chosen.
    ///
    /// Continue, history, and an open title page still carry the file they
    /// were saved with. After the source changes, that file is played on the
    /// chosen add-on when the library has the same episode or movie there.
    /// A same-addon show filed twice keeps the copy with more episodes.
    pub fn route_play(&self, request: &PlayRequest) -> PlayRoute {
        let Some(source) = self.doc.source.clone() else {
            return PlayRoute::Keep;
        };
        let Some(addon) = source_addon(&source) else {
            return PlayRoute::Keep;
        };
        let file = request.file.trim();
        let Some(file_addon) = plugin_host(file) else {
            return PlayRoute::Keep;
        };
        let label = source_label(&source, &addon);
        match request.kind {
            PlayKind::Episode => self.route_episode(request, &addon, &file_addon, &label),
            PlayKind::Movie => self.route_movie(request, &addon, &file_addon, &label),
            PlayKind::Song | PlayKind::File => PlayRoute::Keep,
        }
    }

    fn route_episode(
        &self,
        request: &PlayRequest,
        addon: &str,
        file_addon: &str,
        label: &str,
    ) -> PlayRoute {
        let same = file_addon.eq_ignore_ascii_case(addon);
        let Some(season) = request.season else {
            return other_addon_missing(same, label);
        };
        let Some(episode) = request.episode.filter(|episode| *episode > 0) else {
            return other_addon_missing(same, label);
        };
        let show_id = request
            .show_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .unwrap_or(request.id.trim());
        let name = request
            .show_title
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(request.title.trim());
        let want = comparable_title(name);
        if want.is_empty() {
            return other_addon_missing(same, label);
        }
        let incoming_year = self.request_year(request, TitleKind::Tv, show_id);
        let current_count = self
            .show_owning_file(request.file.trim(), addon)
            .map(|id| self.episode_len(&id))
            .unwrap_or_else(|| self.episode_len(show_id));
        let mut best: Option<ShowPick> = None;
        for record in self.doc.titles.values() {
            if record.kind != TitleKind::Tv {
                continue;
            }
            if !plugin_host(&record.id).is_some_and(|host| host.eq_ignore_ascii_case(addon)) {
                continue;
            }
            if comparable_title(&record.name) != want {
                continue;
            }
            if years_differ(incoming_year, record_year(record)) {
                continue;
            }
            let Some(file) = self.episode_file(&record.id, season, episode) else {
                continue;
            };
            if !plugin_host(&file).is_some_and(|host| host.eq_ignore_ascii_case(addon)) {
                continue;
            }
            let count = self.episode_len(&record.id);
            if same && count <= current_count {
                continue;
            }
            let pick = ShowPick {
                count,
                seasons: is_seasons_action(&record.id),
                id: record.id.clone(),
                file,
            };
            best = Some(match best {
                Some(current) if !pick_better(&pick, &current) => current,
                _ => pick,
            });
        }
        let Some(pick) = best else {
            return other_addon_missing(same, label);
        };
        if pick.id == show_id && pick.file.trim() == request.file.trim() {
            return PlayRoute::Keep;
        }
        PlayRoute::Retarget {
            id: pick.id.clone(),
            file: pick.file,
            show_id: Some(pick.id),
        }
    }

    fn route_movie(
        &self,
        request: &PlayRequest,
        addon: &str,
        file_addon: &str,
        label: &str,
    ) -> PlayRoute {
        if file_addon.eq_ignore_ascii_case(addon) {
            return PlayRoute::Keep;
        }
        let want = comparable_title(request.title.trim());
        if want.is_empty() {
            return PlayRoute::Missing {
                addon: label.to_string(),
            };
        }
        let incoming_year = self.request_year(request, TitleKind::Movie, request.id.trim());
        let mut best: Option<(bool, String)> = None;
        for record in self.doc.titles.values() {
            if record.kind != TitleKind::Movie {
                continue;
            }
            if !plugin_host(&record.id).is_some_and(|host| host.eq_ignore_ascii_case(addon)) {
                continue;
            }
            if comparable_title(&record.name) != want {
                continue;
            }
            if years_differ(incoming_year, record_year(record)) {
                continue;
            }
            let has = self
                .doc
                .progress
                .contains_key(&title_key(TitleKind::Movie, &record.id));
            let id = record.id.clone();
            best = Some(match best {
                Some((best_has, best_id)) if !movie_better(has, &id, best_has, &best_id) => {
                    (best_has, best_id)
                }
                _ => (has, id),
            });
        }
        match best {
            Some((_, id)) => PlayRoute::Retarget {
                id: id.clone(),
                file: id,
                show_id: None,
            },
            None => PlayRoute::Missing {
                addon: label.to_string(),
            },
        }
    }

    fn request_year(&self, request: &PlayRequest, kind: TitleKind, id: &str) -> Option<u32> {
        let stored = self
            .doc
            .titles
            .get(&title_key(kind, id))
            .and_then(|record| record.year);
        let paren = request
            .show_title
            .as_deref()
            .and_then(year_in_parens)
            .or_else(|| year_in_parens(&request.title));
        let url = year_in_query(&request.file)
            .or_else(|| year_in_query(&request.id))
            .or_else(|| request.show_id.as_deref().and_then(year_in_query));
        stored.or(paren).or(url)
    }

    fn episode_len(&self, show_id: &str) -> usize {
        self.doc
            .episodes
            .get(&title_key(TitleKind::Tv, show_id))
            .map(|rows| rows.len())
            .unwrap_or(0)
    }

    fn episode_file(&self, show_id: &str, season: u32, episode: u32) -> Option<String> {
        self.doc
            .episodes
            .get(&title_key(TitleKind::Tv, show_id))
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.season == season && row.episode == episode)
                    .map(|row| row.path.clone())
            })
            .filter(|path| !path.trim().is_empty())
    }

    /// The same-addon show whose stored episode path is this file.
    fn show_owning_file(&self, file: &str, addon: &str) -> Option<String> {
        let file = file.trim();
        if file.is_empty() {
            return None;
        }
        self.doc.episodes.iter().find_map(|(key, rows)| {
            let id = key.strip_prefix("tv:")?;
            if !plugin_host(id).is_some_and(|host| host.eq_ignore_ascii_case(addon)) {
                return None;
            }
            rows.iter()
                .any(|row| row.path.trim() == file)
                .then(|| id.to_string())
        })
    }

    /// One pass over remembered shows, keyed by add-on and comparable name.
    /// A season folder asks this once instead of once per episode.
    fn show_index(&self) -> ShowIndex {
        let mut index = HashMap::new();
        for record in self.doc.titles.values() {
            if record.kind != TitleKind::Tv {
                continue;
            }
            let Some(host) = plugin_host(&record.id) else {
                continue;
            };
            let name = comparable_title(&record.name);
            if name.is_empty() {
                continue;
            }
            index
                .entry((host.to_ascii_lowercase(), name))
                .or_insert_with(Vec::new)
                .push(ListedShow {
                    id: record.id.clone(),
                    seasons: is_seasons_action(&record.id),
                    year: record_year(record),
                });
        }
        index
    }

    /// File an episode listing onto the show it belongs to.
    ///
    /// An episodes folder and a seasons folder of one show become two titles
    /// when the add-on does not set a show path. The seasons folder is the
    /// one playback should keep. A seasons folder is never moved onto a
    /// thinner listing. The caller keeps the item's own show title: the
    /// folder path's last token is often `year`, not the show name.
    fn show_for_listing(&self, folder: &str, hinted: &str, index: &ShowIndex) -> String {
        if is_seasons_action(folder) {
            return folder.to_string();
        }
        let Some(host) = plugin_host(folder) else {
            return folder.to_string();
        };
        let name = {
            let hinted = hinted.trim();
            if !hinted.is_empty() {
                hinted.to_string()
            } else if let Some(from_url) = query_value(folder, "tvshowtitle") {
                from_url
            } else {
                return folder.to_string();
            }
        };
        let want = comparable_title(&name);
        if want.is_empty() {
            return folder.to_string();
        }
        let Some(candidates) = index.get(&(host.to_ascii_lowercase(), want)) else {
            return folder.to_string();
        };
        let incoming_year = year_in_parens(&name).or_else(|| year_in_query(folder));
        let folder_count = self.episode_len(folder);
        let mut seasons: Option<ShowPick> = None;
        let mut fatter: Option<ShowPick> = None;
        for listed in candidates {
            if listed.id == folder || years_differ(incoming_year, listed.year) {
                continue;
            }
            let count = self.episode_len(&listed.id);
            let pick = ShowPick {
                count,
                seasons: listed.seasons,
                id: listed.id.clone(),
                file: String::new(),
            };
            if pick.seasons {
                seasons = Some(match seasons {
                    Some(current) if !pick_better(&pick, &current) => current,
                    _ => pick,
                });
            } else if count > folder_count {
                fatter = Some(match fatter {
                    Some(current) if !pick_better(&pick, &current) => current,
                    _ => pick,
                });
            }
        }
        if let Some(pick) = seasons {
            return pick.id;
        }
        if let Some(pick) = fatter {
            return pick.id;
        }
        folder.to_string()
    }

    pub fn home_continue(&self) -> Vec<Card> {
        let picks = rules::continuing(&self.continue_progress(), |show_id| {
            self.season_counts(show_id)
        });
        // An add-on can file one show under two addresses (from different
        // folders); the same title and episode shows once, newest first.
        let mut seen: BTreeSet<ContinueKey> = BTreeSet::new();
        picks
            .into_iter()
            .filter_map(|pick| self.card_for_pick(pick))
            .filter(|card| {
                let tv = card.kind == TitleKind::Tv;
                // One card per show, its newest episode (an add-on can file a
                // show under its season folders too); a film's year tells apart
                // two that share a title.
                let (season, episode) = if tv {
                    (None, None)
                } else {
                    (card.season, card.episode)
                };
                let year = if tv { None } else { card.year };
                seen.insert((tv, card.name.trim().to_lowercase(), season, episode, year))
            })
            .collect()
    }

    pub fn watchlist(&self) -> Vec<Card> {
        self.list_cards(&self.doc.watchlist)
    }

    pub fn bookmarks(&self) -> Vec<Card> {
        self.list_cards(&self.doc.bookmarks)
    }

    pub fn history(&self) -> Vec<HistoryEntry> {
        let rows: Vec<(usize, HistoryRecord)> = self
            .doc
            .history
            .iter()
            .enumerate()
            .rev()
            .map(|(slot, record)| (slot, record.clone()))
            .collect();
        rows.into_iter()
            .map(|(slot, record)| {
                let name = self.choose_title_name(record.kind, &record.id, &record.name);
                let mut entry = history_entry(record, slot);
                entry.name = name;
                entry
            })
            .collect()
    }

    /// The stored row a client means by `slot` and the title it showed there.
    /// Rows shift as history grows, so when that slot no longer holds the
    /// title, the nearest row that does is used.
    pub fn history_slot(
        &self,
        slot: usize,
        id: &str,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> Option<usize> {
        let same =
            |row: &HistoryRecord| row.id == id && row.season == season && row.episode == episode;
        if self.doc.history.get(slot).is_some_and(same) {
            return Some(slot);
        }
        self.doc
            .history
            .iter()
            .enumerate()
            .filter(|(_, row)| same(row))
            .min_by_key(|(at, _)| at.abs_diff(slot))
            .map(|(at, _)| at)
    }

    pub fn rename_history(&mut self, slot: usize, name: &str) -> Result<bool, StoreError> {
        if name.trim().is_empty() {
            return Ok(false);
        }
        let Some(existing) = self.doc.history.get(slot) else {
            return Ok(false);
        };
        let kind = existing.kind;
        let id = existing.id.clone();
        let art = existing.art.clone();
        let cleaned = self.choose_title_name(kind, &id, name);
        self.doc.history[slot].name = cleaned.clone();
        self.remember_title(kind, &id, &cleaned, art);
        self.save()?;
        Ok(true)
    }

    pub fn clear_history(&mut self) -> Result<usize, StoreError> {
        let count = self.doc.history.len();
        if count == 0 {
            return Ok(0);
        }
        self.doc.history.clear();
        self.save()?;
        Ok(count)
    }

    pub fn delete_history(&mut self, slot: usize) -> Result<bool, StoreError> {
        if slot >= self.doc.history.len() {
            return Ok(false);
        }
        self.doc.history.remove(slot);
        self.save()?;
        Ok(true)
    }

    pub fn put_watchlist(
        &mut self,
        id: &str,
        kind: TitleKind,
        name: &str,
        art: Option<String>,
    ) -> Result<(), StoreError> {
        self.put_watchlist_with(id, kind, name, art, TitleFacts::default())
    }

    /// Saves any title id, played or not; the id is an add-on path and may
    /// carry a query string.
    pub fn put_watchlist_with(
        &mut self,
        id: &str,
        kind: TitleKind,
        name: &str,
        art: Option<String>,
        facts: TitleFacts,
    ) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Ok(());
        }
        self.remember_title(kind, id, name, art);
        self.remember_facts(kind, id, facts);
        self.doc.watchlist.insert(title_key(kind, id), now_ms());
        self.save()
    }

    pub fn delete_watchlist(&mut self, id: &str) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Ok(());
        }
        self.doc.watchlist.retain(|key, _| !key_matches_id(key, id));
        self.save()
    }

    pub fn put_bookmark(
        &mut self,
        id: &str,
        kind: TitleKind,
        name: &str,
        art: Option<String>,
    ) -> Result<(), StoreError> {
        self.put_bookmark_with(id, kind, name, art, TitleFacts::default())
    }

    /// See `put_watchlist_with`.
    pub fn put_bookmark_with(
        &mut self,
        id: &str,
        kind: TitleKind,
        name: &str,
        art: Option<String>,
        facts: TitleFacts,
    ) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Ok(());
        }
        self.remember_title(kind, id, name, art);
        self.remember_facts(kind, id, facts);
        self.doc.bookmarks.insert(title_key(kind, id), now_ms());
        self.save()
    }

    /// Whether `id` is on the watchlist and in bookmarks, as a movie or a show.
    pub fn saved_state(&self, id: &str) -> SavedState {
        let on = |list: &BTreeMap<String, i64>| {
            [TitleKind::Movie, TitleKind::Tv]
                .into_iter()
                .any(|kind| list.contains_key(&title_key(kind, id)))
        };
        SavedState {
            watchlist: on(&self.doc.watchlist),
            bookmarks: on(&self.doc.bookmarks),
        }
    }

    pub fn delete_bookmark(&mut self, id: &str) -> Result<(), StoreError> {
        if id.trim().is_empty() {
            return Ok(());
        }
        self.doc.bookmarks.retain(|key, _| !key_matches_id(key, id));
        self.save()
    }

    pub fn title(&self, id: &str) -> Option<TitleDetail> {
        // `movie:` sorts before `tv:`, which is the order a full scan used to find.
        let record = self
            .doc
            .titles
            .get(&title_key(TitleKind::Movie, id))
            .or_else(|| self.doc.titles.get(&title_key(TitleKind::Tv, id)))?
            .clone();
        let key = title_key(record.kind, &record.id);
        let name = self.choose_title_name(record.kind, &record.id, &record.name);
        let bookmarked = self.doc.bookmarks.contains_key(&key);
        let watchlisted = self.doc.watchlist.contains_key(&key);
        if record.kind == TitleKind::Movie {
            let progress = self.doc.progress.get(&key);
            let resume = progress
                .and_then(|item| resumable(item.position, item.duration).then_some(item.position));
            let watched = progress.is_some_and(|item| item.watched);
            return Some(TitleDetail {
                id: record.id.clone(),
                kind: TitleKind::Movie,
                name: name.clone(),
                art: record.art,
                bookmarked,
                watchlisted,
                resume_seconds: resume,
                watched,
                seasons: Vec::new(),
                season_folders: Vec::new(),
                next: None,
                file: Some(record.id),
                year: record.year,
                imdb_id: record.imdb_id,
            });
        }
        let seasons = self.seasons_for(&record.id);
        let last = self.latest_episode(&record.id);
        let counts = self.season_counts(&record.id);
        let next_num = next_episode(&counts, last.as_ref().map(|item| item.played));
        let next = next_num.and_then(|num| self.episode_ref(&record.id, num));
        let resume = last.and_then(|item| {
            resumable(item.mark.position, item.mark.duration).then_some(item.mark.position)
        });
        let watched = counts_all_watched(&counts, &self.doc.progress, &record.id);
        let season_folders = self.season_folders_of(&record.id);
        Some(TitleDetail {
            id: record.id,
            kind: TitleKind::Tv,
            name,
            art: record.art,
            bookmarked,
            watchlisted,
            resume_seconds: resume,
            watched,
            seasons,
            season_folders,
            next,
            file: None,
            year: record.year,
            imdb_id: record.imdb_id,
        })
    }

    pub fn record_progress(&mut self, report: &ProgressReport) -> Result<(), StoreError> {
        self.write_progress(report, false)
    }

    /// `flush` writes the library file now. Steady playback leaves it false:
    /// the playhead in memory still moves, and the file is written on a
    /// coarser cadence. A pause, a stop, a seek, or a finished title passes
    /// true, or is recognized from the numbers.
    pub fn write_progress(
        &mut self,
        report: &ProgressReport,
        flush: bool,
    ) -> Result<(), StoreError> {
        self.apply_progress(report, now_ms(), flush)
    }

    pub fn record_progress_at(
        &mut self,
        report: &ProgressReport,
        at: i64,
    ) -> Result<(), StoreError> {
        self.apply_progress(report, at, false)
    }

    fn apply_progress(
        &mut self,
        report: &ProgressReport,
        at: i64,
        flush: bool,
    ) -> Result<(), StoreError> {
        let name = self.choose_title_name(report.kind, &report.id, &report.name);
        let mut structural =
            self.remember_title(report.kind, &report.id, &name, report.art.clone());
        let season = report.season.unwrap_or(0);
        let episode = report.episode.unwrap_or(0);
        // Season 0 is a special and keeps its own row. Episode 0 is not one.
        if report.kind == TitleKind::Tv && episode > 0 {
            let file = report.file.clone().unwrap_or_default();
            if !file.is_empty() {
                structural |= self.upsert_episode(
                    &report.id,
                    EpisodeRecord {
                        season,
                        episode,
                        name: report.episode_name.clone().unwrap_or_else(|| name.clone()),
                        path: file,
                        art: report.art.clone(),
                    },
                );
            }
        }
        let key = progress_key(report.kind, &report.id, season, episode);
        // JSON has no NaN or infinity. Either would be saved as null, and the
        // next start would set the whole library aside as unreadable.
        let position = finite_or_zero(report.position);
        let duration = finite_or_zero(report.duration);
        let watched = finished(position, duration);
        // A clock that once ran ahead leaves later stamps on this title's
        // older progress. What plays now is still its newest place.
        let stamp = match self.newest_progress_at(report.kind, &report.id) {
            Some(newest) if newest > at => newest.saturating_add(1),
            _ => at,
        };
        self.doc.progress.insert(
            key.clone(),
            ProgressRecord {
                position,
                duration,
                at: stamp,
                watched,
            },
        );
        // Progress arrives every few seconds while playing, so the same title
        // keeps updating its newest row instead of filling history.
        let mut same_row = false;
        if let Some(last) = self.doc.history.last_mut() {
            // The same show can arrive under two addresses (the dashboard's
            // and a TV's); the same named episode still updates its row.
            // Only an episode (named, with season and episode) matches by name:
            // two films can share a title.
            let same_title = last.id == report.id
                || (report.kind == TitleKind::Tv
                    && report.season.is_some()
                    && report.episode.is_some()
                    && !name.trim().is_empty()
                    && last.name.trim().eq_ignore_ascii_case(name.trim()));
            if last.kind == report.kind
                && same_title
                && last.season == report.season
                && last.episode == report.episode
            {
                same_row = true;
                // The row follows the address now playing: Continue reads the
                // progress filed under the row's id, so a row left on the old
                // address would offer that address's stale resume point.
                if last.id != report.id {
                    structural = true;
                    last.id = report.id.clone();
                }
                last.at = at;
                last.position = position;
                last.duration = duration;
                last.finished = watched;
                // A later report can know the file or art the first one lacked.
                // The name stays: it may have been renamed on the History page.
                if report.file.is_some() && last.file != report.file {
                    structural = true;
                    last.file = report.file.clone();
                }
                if report.art.is_some() && last.art != report.art {
                    structural = true;
                    last.art = report.art.clone();
                }
            }
        }
        if !same_row {
            structural = true;
            // History lists each title once, at its latest play: an older row for
            // the same movie or episode goes.
            let episode_name = (report.kind == TitleKind::Tv
                && report.season.is_some()
                && report.episode.is_some())
            .then(|| name.trim().to_lowercase())
            .filter(|name| !name.is_empty());
            self.doc.history.retain(|row| {
                let same_item = row.kind == report.kind
                    && row.season == report.season
                    && row.episode == report.episode;
                let same_title = row.id == report.id
                    || episode_name
                        .as_deref()
                        .is_some_and(|name| row.name.trim().eq_ignore_ascii_case(name));
                !(same_item && same_title)
            });
            self.doc.history.push(HistoryRecord {
                id: report.id.clone(),
                kind: report.kind,
                name,
                art: report.art.clone(),
                at,
                position,
                duration,
                finished: watched,
                season: report.season,
                episode: report.episode,
                file: report.file.clone(),
            });
            if self.doc.history.len() > HISTORY_LIMIT {
                let extra = self.doc.history.len() - HISTORY_LIMIT;
                self.doc.history.drain(0..extra);
            }
        }
        if !self.progress_is_due(&key, position, duration, watched, at, structural || flush) {
            return Ok(());
        }
        self.save()?;
        self.progress_disk = Some(ProgressDisk {
            key,
            position,
            duration,
            watched,
            at,
        });
        Ok(())
    }

    /// Whether this playhead is worth a rewrite of the library file.
    fn progress_is_due(
        &self,
        key: &str,
        position: f64,
        duration: f64,
        watched: bool,
        at: i64,
        force: bool,
    ) -> bool {
        // A flush, or a history row that changed shape, is written immediately.
        if force {
            return true;
        }
        let Some(prev) = &self.progress_disk else {
            return true;
        };
        if prev.key != key || prev.watched != watched || (prev.duration - duration).abs() > 1.0 {
            return true;
        }
        // A seek backward, or a jump further than the quiet interval.
        if position + 0.5 < prev.position || position - prev.position >= PROGRESS_KEEP_SECONDS {
            return true;
        }
        at.saturating_sub(prev.at) >= PROGRESS_KEEP_MS
    }

    /// Remember titles and episodes as the dashboard walks an add-on folder.
    pub fn observe(&mut self, folder: &str, items: &[BrowseItem]) -> Result<(), StoreError> {
        let mut changed = false;
        let linked = self.doc.season_folders.get(folder).cloned();
        if let Some(link) = &linked {
            for item in items {
                if item.kind != ItemKind::Play {
                    continue;
                }
                let Some(episode) = episode_index(item) else {
                    continue;
                };
                // A tagged season wins, including 0 (specials). A bare file takes the folder's.
                let season = item.season.unwrap_or(link.season);
                changed |= self.upsert_episode(
                    &link.show_id,
                    EpisodeRecord {
                        season,
                        episode,
                        name: item.label.clone(),
                        path: item.path.clone(),
                        art: item.art.clone(),
                    },
                );
            }
        }
        let season_dirs: Vec<(u32, String, String)> = items
            .iter()
            .filter(|item| item.kind == ItemKind::Open)
            .filter_map(|item| {
                let season = season_number(&item.label)?;
                Some((season, item.path.clone(), item.label.clone()))
            })
            .collect();
        if !season_dirs.is_empty() {
            let raw_name = items
                .iter()
                .find_map(|item| item.show_title.clone())
                .unwrap_or_else(|| folder_label(folder));
            let show_name = self.choose_title_name(TitleKind::Tv, folder, &raw_name);
            changed |= self.remember_title(TitleKind::Tv, folder, &show_name, None);
            for (season, path, _label) in &season_dirs {
                let link = SeasonLink {
                    show_id: folder.to_string(),
                    season: *season,
                    show_name: show_name.clone(),
                };
                if self.doc.season_folders.get(path) != Some(&link) {
                    self.doc.season_folders.insert(path.clone(), link);
                    changed = true;
                }
            }
        }
        let mut shows: Option<ShowIndex> = None;
        for item in items {
            if item.kind != ItemKind::Play {
                continue;
            }
            // The linked season folder above already filed this episode under
            // its show. Filing it again would make a show of the folder itself.
            if linked.as_ref().is_some_and(|link| {
                episode_index(item).is_some()
                    && item
                        .show_path
                        .as_ref()
                        .is_none_or(|show| *show == link.show_id)
            }) {
                continue;
            }
            if let Some((season, episode)) = episode_numbers(item) {
                let show_id = match item.show_path.as_deref() {
                    Some(path) if !path.trim().is_empty() => path.to_string(),
                    _ => {
                        let hinted = item.show_title.as_deref().unwrap_or("");
                        if shows.is_none() {
                            shows = Some(self.show_index());
                        }
                        self.show_for_listing(folder, hinted, shows.as_ref().unwrap())
                    }
                };
                let redirected = show_id != folder
                    && item
                        .show_path
                        .as_deref()
                        .is_none_or(|path| path.trim().is_empty());
                // The redirected id often ends in `year=`, and that token is
                // not the show's name.
                let raw_name = item
                    .show_title
                    .clone()
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| {
                        if redirected {
                            query_value(&show_id, "tvshowtitle")
                                .or_else(|| query_value(folder, "tvshowtitle"))
                                .unwrap_or_else(|| folder_label(folder))
                        } else {
                            folder_label(&show_id)
                        }
                    });
                let show_name = self.choose_title_name(TitleKind::Tv, &show_id, &raw_name);
                let art = item.art.clone();
                changed |= self.remember_title(TitleKind::Tv, &show_id, &show_name, art.clone());
                changed |= self.upsert_episode(
                    &show_id,
                    EpisodeRecord {
                        season,
                        episode,
                        name: item.label.clone(),
                        path: item.path.clone(),
                        art,
                    },
                );
                continue;
            }
            if item.title_kind == Some(TitleKind::Movie) || looks_like_movie(item) {
                changed |= self.remember_title(
                    TitleKind::Movie,
                    &item.path,
                    &item.label,
                    item.art.clone(),
                );
                if let Some(info) = &item.info {
                    let facts = TitleFacts::new(info.year, info.imdb_id.as_deref());
                    changed |= self.remember_facts(TitleKind::Movie, &item.path, facts);
                }
            }
        }
        if changed {
            self.save()?;
        }
        Ok(())
    }

    pub fn next_after(
        &self,
        kind: TitleKind,
        id: &str,
        season: Option<u32>,
        episode: Option<u32>,
    ) -> Option<NextItem> {
        if kind != TitleKind::Tv {
            return None;
        }
        let last = match (season, episode) {
            (Some(season), Some(episode)) if episode > 0 => Some(LastPlayed {
                season,
                episode,
                watched: true,
            }),
            _ => self.latest_episode(id).map(|item| LastPlayed {
                season: item.played.season,
                episode: item.played.episode,
                watched: item.mark.watched,
            }),
        };
        let num = next_episode(&self.season_counts(id), last)?;
        if last.is_some_and(|item| {
            item.season == num.season && item.episode == num.episode && !item.watched
        }) {
            return None;
        }
        let episode_ref = self.episode_ref(id, num)?;
        if episode_ref.path.is_empty() {
            return None;
        }
        let stored = self
            .doc
            .titles
            .get(&title_key(TitleKind::Tv, id))
            .map(|item| item.name.clone());
        Some(NextItem {
            kind: PlayKind::Episode,
            id: id.to_string(),
            file: episode_ref.path,
            title: episode_ref.name,
            season: Some(episode_ref.season),
            episode: Some(episode_ref.episode),
            show_id: Some(id.to_string()),
            show_title: stored
                .map(|stored_name| self.choose_title_name(TitleKind::Tv, id, &stored_name)),
            art: episode_ref.art,
        })
    }

    /// The folder of the show's first season after `season`, once its season
    /// list has been seen. Specials (0) never follow a season.
    pub fn season_folder_after(&self, show_id: &str, season: u32) -> Option<String> {
        self.doc
            .season_folders
            .iter()
            .filter(|(_, link)| link.show_id == show_id && link.season > season)
            .min_by_key(|(_, link)| link.season)
            .map(|(path, _)| path.clone())
    }

    /// One folder per season of the show, lowest season first.
    fn season_folders_of(&self, show_id: &str) -> Vec<SeasonFolder> {
        let mut by_season: BTreeMap<u32, String> = BTreeMap::new();
        for (path, link) in &self.doc.season_folders {
            if link.show_id == show_id {
                by_season.entry(link.season).or_insert_with(|| path.clone());
            }
        }
        by_season
            .into_iter()
            .map(|(season, path)| SeasonFolder { season, path })
            .collect()
    }

    fn card_for_pick(&self, pick: ContinuePick) -> Option<Card> {
        let parsed = rules::parse_key(&pick.title_key)?;
        let (id, kind, art, stored, year, imdb_id) = {
            let record = self.doc.titles.get(&pick.title_key)?;
            (
                record.id.clone(),
                record.kind,
                record.art.clone(),
                record.name.clone(),
                record.year,
                record.imdb_id.clone(),
            )
        };
        let name = self.choose_title_name(kind, &id, &stored);
        let (season, episode, episode_name, file, fraction, resume) = match &pick.progress {
            Some(mark) => {
                let file = self.file_for(&parsed, pick.season, pick.episode);
                let episode_name = self.episode_name(id.as_str(), pick.season, pick.episode);
                (
                    (pick.episode > 0).then_some(pick.season),
                    (pick.episode > 0).then_some(pick.episode),
                    episode_name,
                    file,
                    Some(rules::fraction(mark.position, mark.duration, false)),
                    Some(mark.position),
                )
            }
            None => {
                let file = self.file_for(&parsed, pick.season, pick.episode);
                let episode_name = self.episode_name(id.as_str(), pick.season, pick.episode);
                (
                    Some(pick.season),
                    Some(pick.episode),
                    episode_name,
                    file,
                    None,
                    None,
                )
            }
        };
        Some(Card {
            id,
            kind,
            name,
            art,
            fraction,
            resume_seconds: resume.filter(|seconds| *seconds > rules::RESUME_AFTER),
            season,
            episode,
            episode_name,
            file,
            year,
            imdb_id,
        })
    }

    fn file_for(&self, parsed: &ParsedKey, season: u32, episode: u32) -> Option<String> {
        match parsed {
            ParsedKey::Movie { id } => Some(id.clone()),
            ParsedKey::Show { id } | ParsedKey::Episode { id, .. } => self
                .doc
                .episodes
                .get(&title_key(TitleKind::Tv, id))
                .and_then(|rows| {
                    rows.iter()
                        .find(|row| row.season == season && row.episode == episode)
                        .map(|row| row.path.clone())
                }),
        }
    }

    fn episode_name(&self, show_id: &str, season: u32, episode: u32) -> Option<String> {
        self.doc
            .episodes
            .get(&title_key(TitleKind::Tv, show_id))
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row.season == season && row.episode == episode)
                    .map(|row| row.name.clone())
            })
    }

    fn list_cards(&self, list: &BTreeMap<String, i64>) -> Vec<Card> {
        let mut rows: Vec<(&String, &i64)> = list.iter().collect();
        rows.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        rows.into_iter()
            .filter_map(|(key, _)| {
                let (id, kind, art, stored, year, imdb_id) = {
                    let record = self.doc.titles.get(key)?;
                    (
                        record.id.clone(),
                        record.kind,
                        record.art.clone(),
                        record.name.clone(),
                        record.year,
                        record.imdb_id.clone(),
                    )
                };
                let name = self.choose_title_name(kind, &id, &stored);
                let progress = self.doc.progress.get(key);
                let fraction = progress
                    .map(|item| rules::fraction(item.position, item.duration, item.watched));
                let resume = progress.and_then(|item| {
                    resumable(item.position, item.duration).then_some(item.position)
                });
                Some(Card {
                    id: id.clone(),
                    kind,
                    name,
                    art,
                    fraction,
                    resume_seconds: resume,
                    season: None,
                    episode: None,
                    episode_name: None,
                    file: (kind == TitleKind::Movie).then_some(id),
                    year,
                    imdb_id,
                })
            })
            .collect()
    }

    /// Returns whether the stored title changed.
    fn remember_title(
        &mut self,
        kind: TitleKind,
        id: &str,
        name: &str,
        art: Option<String>,
    ) -> bool {
        let key = title_key(kind, id);
        let name = self.choose_title_name(kind, id, name);
        let seen = now_ms();
        match self.doc.titles.get_mut(&key) {
            Some(existing) => {
                let changed = existing.name != name || (art.is_some() && existing.art != art);
                // Seeing a title again is not worth a save on its own.
                existing.seen = seen;
                existing.name = name;
                if art.is_some() {
                    existing.art = art;
                }
                changed
            }
            None => {
                self.doc.titles.insert(
                    key,
                    TitleRecord {
                        kind,
                        id: id.to_string(),
                        name,
                        art,
                        year: None,
                        imdb_id: None,
                        seen,
                    },
                );
                true
            }
        }
    }

    /// Fills in a known year and IMDb id; `None` keeps what is stored.
    /// Returns whether the stored title changed.
    fn remember_facts(&mut self, kind: TitleKind, id: &str, facts: TitleFacts) -> bool {
        let Some(record) = self.doc.titles.get_mut(&title_key(kind, id)) else {
            return false;
        };
        let mut changed = false;
        if facts.year.is_some() && record.year != facts.year {
            record.year = facts.year;
            changed = true;
        }
        if facts.imdb_id.is_some() && record.imdb_id != facts.imdb_id {
            record.imdb_id = facts.imdb_id;
            changed = true;
        }
        changed
    }

    /// Keep a human title. Artwork JSON is never stored as the name.
    fn choose_title_name(&self, kind: TitleKind, id: &str, incoming: &str) -> String {
        if usable_title_name(incoming) {
            return incoming.trim().to_string();
        }
        if let Some(existing) = self.doc.titles.get(&title_key(kind, id)) {
            if usable_title_name(&existing.name) {
                return existing.name.trim().to_string();
            }
        }
        if let Some(name) = self.latest_usable_history_name(kind, id) {
            return name;
        }
        fallback_title_name(id)
    }

    fn latest_usable_history_name(&self, kind: TitleKind, id: &str) -> Option<String> {
        self.doc.history.iter().rev().find_map(|row| {
            if row.kind == kind && row.id == id && usable_title_name(&row.name) {
                Some(row.name.trim().to_string())
            } else {
                None
            }
        })
    }

    /// Returns whether the stored episode changed.
    fn upsert_episode(&mut self, show_id: &str, episode: EpisodeRecord) -> bool {
        let key = title_key(TitleKind::Tv, show_id);
        let rows = self.doc.episodes.entry(key).or_default();
        if let Some(existing) = rows
            .iter_mut()
            .find(|row| row.season == episode.season && row.episode == episode.episode)
        {
            let changed = existing.name != episode.name
                || existing.path != episode.path
                || (episode.art.is_some() && existing.art != episode.art);
            existing.name = episode.name;
            existing.path = episode.path;
            if episode.art.is_some() {
                existing.art = episode.art;
            }
            changed
        } else {
            rows.push(episode);
            rows.sort_by(|a, b| a.season.cmp(&b.season).then(a.episode.cmp(&b.episode)));
            true
        }
    }

    fn season_counts(&self, show_id: &str) -> Vec<SeasonCount> {
        let Some(rows) = self.doc.episodes.get(&title_key(TitleKind::Tv, show_id)) else {
            return Vec::new();
        };
        let mut numbers: BTreeMap<u32, u32> = BTreeMap::new();
        for row in rows {
            let count = numbers.entry(row.season).or_insert(0);
            *count = (*count).max(row.episode);
        }
        numbers
            .into_iter()
            .map(|(number, episodes)| SeasonCount { number, episodes })
            .collect()
    }

    fn seasons_for(&self, show_id: &str) -> Vec<Season> {
        let Some(rows) = self.doc.episodes.get(&title_key(TitleKind::Tv, show_id)) else {
            return Vec::new();
        };
        let mut grouped: BTreeMap<u32, Vec<Episode>> = BTreeMap::new();
        for row in rows {
            let key = progress_key(TitleKind::Tv, show_id, row.season, row.episode);
            let progress = self.doc.progress.get(&key);
            let position = progress.map(|item| item.position).unwrap_or(0.0);
            let duration = progress.map(|item| item.duration).unwrap_or(0.0);
            let done = progress.is_some_and(|item| item.watched);
            grouped.entry(row.season).or_default().push(Episode {
                season: row.season,
                episode: row.episode,
                name: row.name.clone(),
                path: row.path.clone(),
                art: row.art.clone(),
                position,
                duration,
                finished: done,
                resumable: resumable(position, duration),
            });
        }
        grouped
            .into_iter()
            .map(|(number, episodes)| Season { number, episodes })
            .collect()
    }

    fn episode_ref(&self, show_id: &str, num: EpisodeNum) -> Option<EpisodeRef> {
        let row = self
            .doc
            .episodes
            .get(&title_key(TitleKind::Tv, show_id))?
            .iter()
            .find(|row| row.season == num.season && row.episode == num.episode)?;
        Some(EpisodeRef {
            season: row.season,
            episode: row.episode,
            name: row.name.clone(),
            path: row.path.clone(),
            art: row.art.clone(),
        })
    }

    /// The newest stamp on a title's progress: the movie, or any episode of the show.
    fn newest_progress_at(&self, kind: TitleKind, id: &str) -> Option<i64> {
        let title = title_key(kind, id);
        let episodes = format!("{title}:");
        self.doc
            .progress
            .range(title.clone()..)
            .take_while(|(key, _)| key.starts_with(&title))
            .filter(|(key, _)| {
                **key == title
                    || (key.starts_with(&episodes)
                        && rules::parse_key(key).is_some_and(|parsed| parsed.title_id() == id))
            })
            .map(|(_, record)| record.at)
            .max()
    }

    fn latest_episode(&self, show_id: &str) -> Option<Latest> {
        let prefix = format!("tv:{show_id}:");
        self.doc
            .progress
            .range(prefix.clone()..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .filter_map(|(key, record)| {
                let ParsedKey::Episode {
                    season, episode, ..
                } = rules::parse_key(key)?
                else {
                    return None;
                };
                Some(Latest {
                    played: LastPlayed {
                        season,
                        episode,
                        watched: record.watched,
                    },
                    mark: ProgressMark {
                        position: record.position,
                        duration: record.duration,
                        at: record.at,
                        watched: record.watched,
                    },
                })
            })
            .fold(None, |picked: Option<Latest>, item| match picked {
                Some(best) if !later_place(&item, &best) => Some(best),
                _ => Some(item),
            })
    }

    // Continue reads progress only where a history row still matches that key.
    // History is capped; the progress map is every episode ever watched.
    fn continue_progress(&self) -> BTreeMap<String, ProgressMark> {
        let mut map = BTreeMap::new();
        for record in &self.doc.history {
            let key = history_progress_key(record);
            if map.contains_key(&key) {
                continue;
            }
            let Some(item) = self.doc.progress.get(&key) else {
                continue;
            };
            map.insert(
                key,
                ProgressMark {
                    position: item.position,
                    duration: item.duration,
                    at: item.at,
                    watched: item.watched,
                },
            );
        }
        map
    }

    fn save(&self) -> Result<(), StoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(StoreError::Io)?;
        }
        // Compact, because the file is rewritten often and can grow to megabytes.
        let tmp = self.path.with_extension("json.tmp");
        let file = fs::File::create(&tmp).map_err(StoreError::Io)?;
        let mut writer = io::BufWriter::new(file);
        serde_json::to_writer(&mut writer, &self.doc).map_err(StoreError::Json)?;
        writer.flush().map_err(StoreError::Io)?;
        let file = writer
            .into_inner()
            .map_err(|err| StoreError::Io(err.into_error()))?;
        file.sync_all().map_err(StoreError::Io)?;
        drop(file);
        fs::rename(&tmp, &self.path).map_err(StoreError::Io)?;
        Ok(())
    }
}

struct Latest {
    played: LastPlayed,
    mark: ProgressMark,
}

/// Whether `item` is a better place to pick a show up from than `best`. A
/// finished special does not move the show back to the specials: the latest
/// regular episode wins over it. A special still in progress can be resumed.
fn later_place(item: &Latest, best: &Latest) -> bool {
    let aside = |latest: &Latest| latest.played.season == 0 && latest.mark.watched;
    match (aside(item), aside(best)) {
        (true, false) => return false,
        (false, true) => return true,
        _ => {}
    }
    item.mark.at.cmp(&best.mark.at).then_with(|| {
        (item.played.season, item.played.episode).cmp(&(best.played.season, best.played.episode))
    }) == std::cmp::Ordering::Greater
}

fn history_progress_key(record: &HistoryRecord) -> String {
    progress_key(
        record.kind,
        &record.id,
        record.season.unwrap_or(0),
        record.episode.unwrap_or(0),
    )
}

fn history_entry(record: HistoryRecord, slot: usize) -> HistoryEntry {
    HistoryEntry {
        id: record.id,
        kind: record.kind,
        name: record.name,
        art: record.art,
        at: record.at,
        position: record.position,
        duration: record.duration,
        finished: record.finished,
        season: record.season,
        episode: record.episode,
        file: record.file,
        slot,
    }
}

fn key_matches_id(key: &str, id: &str) -> bool {
    // An exact key first: an id that itself ends in `:<n>:<n>` would otherwise
    // be read as an episode key.
    key == title_key(TitleKind::Movie, id)
        || key == title_key(TitleKind::Tv, id)
        || rules::parse_key(key).is_some_and(|parsed| parsed.title_id() == id)
}

fn counts_all_watched(
    counts: &[SeasonCount],
    progress: &BTreeMap<String, ProgressRecord>,
    show_id: &str,
) -> bool {
    if counts.is_empty() {
        return false;
    }
    // Specials do not keep a show with regular seasons from counting as watched.
    let main: Vec<&SeasonCount> = counts.iter().filter(|season| season.number > 0).collect();
    let seasons: Vec<&SeasonCount> = if main.is_empty() {
        counts.iter().collect()
    } else {
        main
    };
    seasons.iter().all(|season| {
        (1..=season.episodes).all(|episode| {
            let key = progress_key(TitleKind::Tv, show_id, season.number, episode);
            progress.get(&key).is_some_and(|item| item.watched)
        })
    })
}

fn episode_numbers(item: &BrowseItem) -> Option<(u32, u32)> {
    if let (Some(season), Some(episode)) = (item.season, item.episode) {
        if episode > 0 {
            return Some((season, episode));
        }
    }
    parse_episode_label(&item.label)
}

fn episode_index(item: &BrowseItem) -> Option<u32> {
    if let Some(episode) = item.episode {
        if episode > 0 {
            return Some(episode);
        }
    }
    parse_episode_label(&item.label).map(|(_, episode)| episode)
}

fn parse_episode_label(label: &str) -> Option<(u32, u32)> {
    let text = label.trim();
    let bytes = text.as_bytes();
    let lower = text.to_ascii_lowercase();
    // Every `s` is a candidate. A show name comes first.
    for (index, _) in lower.match_indices('s') {
        if let Some((season, next)) = take_number(&lower[index + 1..]) {
            let rest = &lower[index + 1 + next..];
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('e') {
                if let Some((episode, _)) = take_number(rest.trim_start()) {
                    if episode > 0 {
                        return Some((season, episode));
                    }
                }
            }
        }
    }
    let mut digits = String::new();
    let mut seen = Vec::new();
    for (index, byte) in bytes.iter().enumerate() {
        if byte.is_ascii_digit() {
            digits.push(*byte as char);
            continue;
        }
        if !digits.is_empty() && (*byte == b'x' || *byte == b'X') {
            if let Ok(season) = digits.parse::<u32>() {
                let after = &text[index + 1..];
                if let Some((episode, _)) = take_number(after) {
                    if episode > 0 {
                        seen.push((season, episode));
                    }
                }
            }
        }
        digits.clear();
    }
    seen.into_iter().next()
}

fn season_number(label: &str) -> Option<u32> {
    let text = label.trim().to_ascii_lowercase();
    if text == "specials" {
        return Some(0);
    }
    for prefix in ["season ", "series "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            let (number, _) = take_number(rest.trim_start())?;
            return Some(number);
        }
    }
    None
}

fn take_number(text: &str) -> Option<(u32, usize)> {
    let digits: String = text.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some((digits.parse().ok()?, digits.len()))
}

fn looks_like_movie(item: &BrowseItem) -> bool {
    item.kind == ItemKind::Play
        && item.season.is_none()
        && item.episode.is_none()
        && item.title_kind != Some(TitleKind::Tv)
}

/// Where `route_play` sends a play.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayRoute {
    /// Play the request as it was sent.
    Keep,
    /// Play this copy on the chosen add-on. `id` is the show for an episode
    /// and the file for a movie.
    Retarget {
        id: String,
        file: String,
        show_id: Option<String>,
    },
    /// The chosen add-on has no copy. `addon` is the source's display name.
    Missing { addon: String },
}

struct ShowPick {
    count: usize,
    seasons: bool,
    id: String,
    file: String,
}

struct ListedShow {
    id: String,
    seasons: bool,
    year: Option<u32>,
}

type ShowIndex = HashMap<(String, String), Vec<ListedShow>>;

fn other_addon_missing(same_addon: bool, label: &str) -> PlayRoute {
    if same_addon {
        PlayRoute::Keep
    } else {
        PlayRoute::Missing {
            addon: label.to_string(),
        }
    }
}

fn source_addon(source: &Source) -> Option<String> {
    if let Some(id) = source
        .addon_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        return Some(plugin_host(id).unwrap_or_else(|| id.to_string()));
    }
    source.path.as_deref().and_then(plugin_host)
}

fn source_label(source: &Source, addon: &str) -> String {
    source
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(addon)
        .to_string()
}

fn plugin_host(path: &str) -> Option<String> {
    let path = path.trim();
    let rest = path
        .get(0..9)
        .filter(|prefix| prefix.eq_ignore_ascii_case("plugin://"))
        .and_then(|_| path.get("plugin://".len()..))?;
    let host = rest.split(['/', '?', '&', '|']).next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn is_seasons_action(url: &str) -> bool {
    query_value(url, "action").is_some_and(|action| action.eq_ignore_ascii_case("seasons"))
}

fn pick_better(next: &ShowPick, current: &ShowPick) -> bool {
    if next.count != current.count {
        return next.count > current.count;
    }
    if next.seasons != current.seasons {
        return next.seasons;
    }
    next.id < current.id
}

fn movie_better(has: bool, id: &str, best_has: bool, best_id: &str) -> bool {
    if has != best_has {
        return has;
    }
    id < best_id
}

fn record_year(record: &TitleRecord) -> Option<u32> {
    record
        .year
        .or_else(|| year_in_parens(&record.name))
        .or_else(|| year_in_query(&record.id))
}

fn years_differ(left: Option<u32>, right: Option<u32>) -> bool {
    matches!((left, right), (Some(left), Some(right)) if left != right)
}

/// Lowercase, one space, without a trailing `(YYYY)`.
fn comparable_title(name: &str) -> String {
    let trimmed = name.trim();
    let text = strip_trailing_year(trimmed).unwrap_or(trimmed);
    let mut out = String::new();
    let mut space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !out.is_empty() {
                space = true;
            }
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        for lower in ch.to_lowercase() {
            out.push(lower);
        }
    }
    out
}

fn strip_trailing_year(name: &str) -> Option<&str> {
    let name = name.trim();
    let open = name.rfind(" (")?;
    if open == 0 {
        return None;
    }
    let inside = name[open + 2..].strip_suffix(')')?;
    let year: u32 = inside.parse().ok()?;
    (1870..=2200).contains(&year).then(|| name[..open].trim())
}

fn year_in_parens(name: &str) -> Option<u32> {
    let name = name.trim();
    let open = name.rfind(" (")?;
    let inside = name[open + 2..].strip_suffix(')')?;
    let year: u32 = inside.parse().ok()?;
    (1870..=2200).contains(&year).then_some(year)
}

fn year_in_query(url: &str) -> Option<u32> {
    let text = query_value(url, "year")?;
    let year: u32 = text.parse().ok()?;
    (1870..=2200).contains(&year).then_some(year)
}

fn folder_label(path: &str) -> String {
    path.rsplit(['/', '?', '&', '='])
        .find(|part| !part.is_empty() && *part != "plugin:")
        .unwrap_or(path)
        .to_string()
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

/// False for an empty name, artwork JSON (raw or URL-encoded), or a plugin path.
fn usable_title_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    if trimmed.starts_with('{') || lower.starts_with("%7b") {
        return false;
    }
    if trimmed.contains("\"poster\"") || lower.contains("%22poster%22") {
        return false;
    }
    if lower.contains("%7b") && lower.contains("poster") {
        return false;
    }
    if lower.contains("://") || lower.starts_with("plugin:") || lower.starts_with("plugin.") {
        return false;
    }
    true
}

/// Short name taken from the id. Never the raw plugin path.
fn fallback_title_name(id: &str) -> String {
    for key in ["tvshowtitle", "showtitle", "title", "name"] {
        if let Some(value) = query_value(id, key) {
            return value;
        }
    }
    path_segment_title(id).unwrap_or_else(|| "Title".to_string())
}

fn query_value(id: &str, wanted: &str) -> Option<String> {
    for part in id.split('&') {
        let Some((raw_key, raw_value)) = part.split_once('=') else {
            continue;
        };
        let raw_key = raw_key.rsplit(['?', '#']).next().unwrap_or(raw_key);
        if !percent_decode_name(raw_key).eq_ignore_ascii_case(wanted) {
            continue;
        }
        let value = percent_decode_name(raw_value);
        let value = value.trim();
        if short_human_title(value) {
            return Some(value.to_string());
        }
    }
    None
}

fn path_segment_title(id: &str) -> Option<String> {
    let trimmed = id.trim().trim_end_matches(['/', '\\']);
    let segment = trimmed.rsplit(['/', '\\']).find(|part| !part.is_empty())?;
    let segment = segment.split(['?', '#']).next().unwrap_or("").trim();
    let segment = percent_decode_name(segment);
    let segment = segment.trim();
    short_human_title(segment).then(|| segment.to_string())
}

fn short_human_title(value: &str) -> bool {
    let value = value.trim();
    if !usable_title_name(value) || value.chars().count() > 160 {
        return false;
    }
    let lower = value.to_ascii_lowercase();
    if lower.contains("://") || value.contains('/') || value.contains('\\') {
        return false;
    }
    if lower.starts_with("plugin:") || lower.starts_with("plugin.") {
        return false;
    }
    true
}

fn percent_decode_name(text: &str) -> String {
    let plus = text.replace('+', " ");
    let bytes = plus.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
#[path = "../../../tests-private/library/src/store.rs"]
mod tests;
