//! Details for the info panel: `POST /api/info` merges what the page knows
//! (usually the add-on's own labels) with TMDB (only with the user's key),
//! TVmaze (shows and episodes) and Wikipedia (movies). `GET`/`PUT
//! /api/metadata` turn online lookups on or off and store the TMDB key in
//! `%LOCALAPPDATA%\Portal\metadata.json`. The key is never sent back or logged.
//!
//! Sources run side by side, each with its own time limit, so an answer comes
//! within about 8 seconds even when a source hangs. A source that fails only
//! means fewer sources.

mod cache;
mod merge;
pub(crate) mod net;
mod settings;
pub(crate) mod text;
mod tmdb;
mod tvmaze;
mod wiki;

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::Json;
use portal_api::{MediaInfo, MediaKind, MetadataPut, MetadataSettings};

use crate::{fail, ApiError, AppState};

pub const SOURCE_TMDB: &str = "TMDB";
pub const SOURCE_TVMAZE: &str = "TVmaze";
pub const SOURCE_WIKIPEDIA: &str = "Wikipedia";

/// One source's whole lookup, every request in it together.
const SOURCE_LIMIT: Duration = Duration::from_secs(7);
/// Checking a new TMDB key.
const CHECK_LIMIT: Duration = Duration::from_secs(10);

const KEY_REFUSED: &str = "TMDB did not accept that key.";

/// What the sources look up by, taken from a cleaned seed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub kind: Option<MediaKind>,
    /// The movie's or show's title; for an episode, the show's.
    pub title: Option<String>,
    pub year: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub imdb_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub tvdb_id: Option<String>,
}

impl Query {
    pub fn from_seed(seed: &MediaInfo) -> Self {
        let episode = seed.kind == Some(MediaKind::Episode);
        Self {
            kind: seed.kind,
            title: if episode {
                seed.show_title.clone()
            } else {
                seed.title.clone()
            },
            // An episode's year is when it aired, not when the show began.
            year: seed.year.filter(|_| !episode),
            season: seed.season,
            episode: seed.episode,
            imdb_id: seed.imdb_id.clone(),
            tmdb_id: seed.tmdb_id.clone(),
            tvdb_id: seed.tvdb_id.clone(),
        }
    }

    pub fn episode_numbers(&self) -> Option<(u32, u32)> {
        Some((self.season?, self.episode?))
    }

    fn has_id(&self) -> bool {
        self.imdb_id.is_some() || self.tmdb_id.is_some() || self.tvdb_id.is_some()
    }

    fn wants_tmdb(&self) -> bool {
        self.has_id() || (self.title.is_some() && self.kind.is_some())
    }

    /// Shows and episodes; with no kind, only an exact id lookup.
    fn wants_tvmaze(&self) -> bool {
        match self.kind {
            Some(MediaKind::Tv) | Some(MediaKind::Episode) => {
                self.title.is_some() || self.imdb_id.is_some() || self.tvdb_id.is_some()
            }
            Some(MediaKind::Movie) => false,
            None => self.imdb_id.is_some() || self.tvdb_id.is_some(),
        }
    }

    fn wants_wikipedia(&self) -> bool {
        match self.kind {
            Some(MediaKind::Movie) | None => self.imdb_id.is_some() || self.title.is_some(),
            _ => false,
        }
    }
}

/// How one source's lookup ended.
#[derive(Debug)]
enum Outcome {
    Found(Box<MediaInfo>),
    Missing,
    Failed,
}

pub struct Metadata {
    settings_path: PathBuf,
    settings: Mutex<settings::Stored>,
    /// One change at a time: two at once would each save over the other.
    applying: tokio::sync::Mutex<()>,
    cache: cache::Cache,
    client: Option<reqwest::Client>,
}

impl Metadata {
    pub fn open(portal_dir: &Path) -> Self {
        let settings_path = portal_dir.join("metadata.json");
        let cache_path = portal_dir.join("metadata-cache.json");
        Self {
            settings: Mutex::new(settings::load(&settings_path)),
            settings_path,
            applying: tokio::sync::Mutex::new(()),
            cache: cache::Cache::open(Some(&cache_path)),
            client: net::client(),
        }
    }

    fn stored(&self) -> settings::Stored {
        self.settings
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn public(&self) -> MetadataSettings {
        let stored = self.stored();
        MetadataSettings {
            online: stored.online,
            tmdb: stored.tmdb_key.is_some(),
        }
    }

    /// `PUT /api/metadata`: loopback peers only. A new key is checked with
    /// TMDB before it is saved; `null` or a blank key removes it.
    pub async fn apply(
        &self,
        peer: SocketAddr,
        put: MetadataPut,
    ) -> Result<MetadataSettings, ApiError> {
        if !crate::from_this_pc(peer) {
            return Err(fail(
                StatusCode::FORBIDDEN,
                "Metadata settings can only be changed on this PC.",
            ));
        }
        let _applying = self.applying.lock().await;
        let key = put.tmdb_key.map(|key| {
            key.map(|key| key.trim().to_string())
                .filter(|key| !key.is_empty())
        });
        if let Some(Some(key)) = &key {
            self.check_key(key).await?;
        }
        let mut next = self.stored();
        if let Some(online) = put.online {
            next.online = online;
        }
        let key_changed = match key {
            Some(key) if key != next.tmdb_key => {
                next.tmdb_key = key;
                true
            }
            _ => false,
        };
        {
            let mut stored = self
                .settings
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            settings::save(&self.settings_path, &next).map_err(|err| {
                tracing::warn!(error = %err, "Could not save metadata settings");
                fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Portal could not save that setting.",
                )
            })?;
            *stored = next;
        }
        if key_changed {
            // Earlier lookups ran without (or with another) TMDB key.
            self.cache.clear();
            tracing::info!("TMDB key updated");
        }
        Ok(self.public())
    }

    async fn check_key(&self, key: &str) -> Result<(), ApiError> {
        if !tmdb::plausible_key(key) {
            return Err(fail(StatusCode::BAD_REQUEST, KEY_REFUSED));
        }
        let Some(client) = &self.client else {
            return Err(unreachable_tmdb());
        };
        let checked = tokio::time::timeout(CHECK_LIMIT, tmdb::Tmdb::new(client, key).check()).await;
        match checked {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(fail(StatusCode::BAD_REQUEST, KEY_REFUSED)),
            _ => Err(unreachable_tmdb()),
        }
    }

    /// `POST /api/info`.
    pub async fn lookup(&self, seed: MediaInfo) -> MediaInfo {
        let seed = merge::normalize_seed(seed);
        let stored = self.stored();
        if !stored.online {
            return merge::merge(seed, Vec::new());
        }
        let (Some(key), Some(client)) = (cache::key(&seed), &self.client) else {
            return merge::merge(seed, Vec::new());
        };
        if let Some(found) = self.cache.get(&key, cache::now()) {
            return merge::merge(seed, found);
        }
        let query = Query::from_seed(&seed);
        let tmdb_key = stored.tmdb_key.as_deref();
        let (from_tmdb, from_tvmaze, from_wiki) = tokio::join!(
            run(
                tmdb_key.is_some() && query.wants_tmdb(),
                SOURCE_TMDB,
                async {
                    match tmdb_key {
                        Some(key) => tmdb::Tmdb::new(client, key).lookup(&query).await,
                        None => Ok(None),
                    }
                },
            ),
            run(
                query.wants_tvmaze(),
                SOURCE_TVMAZE,
                tvmaze::lookup(client, &query)
            ),
            run(
                query.wants_wikipedia(),
                SOURCE_WIKIPEDIA,
                wiki::lookup(client, &query)
            ),
        );
        let outcomes: Vec<Outcome> = [from_tmdb, from_tvmaze, from_wiki]
            .into_iter()
            .flatten()
            .collect();
        let failed = outcomes
            .iter()
            .any(|outcome| matches!(outcome, Outcome::Failed));
        let found: Vec<MediaInfo> = outcomes
            .into_iter()
            .filter_map(|outcome| match outcome {
                Outcome::Found(info) => Some(*info),
                _ => None,
            })
            .collect();
        let ttl = if failed {
            cache::FAILED_TTL
        } else if found.is_empty() {
            cache::MISSING_TTL
        } else {
            cache::FOUND_TTL
        };
        self.cache.put(&key, found.clone(), ttl, cache::now());
        merge::merge(seed, found)
    }
}

fn unreachable_tmdb() -> ApiError {
    fail(
        StatusCode::BAD_GATEWAY,
        "Portal could not reach TMDB to check that key. Try again.",
    )
}

/// Runs one source when it applies, within `SOURCE_LIMIT`.
async fn run<F>(wanted: bool, name: &str, lookup: F) -> Option<Outcome>
where
    F: Future<Output = Result<Option<MediaInfo>, net::Failed>>,
{
    if !wanted {
        return None;
    }
    Some(match tokio::time::timeout(SOURCE_LIMIT, lookup).await {
        Ok(Ok(Some(info))) => Outcome::Found(Box::new(info)),
        Ok(Ok(None)) => Outcome::Missing,
        Ok(Err(net::Failed)) => {
            tracing::debug!(source = name, "Metadata source failed");
            Outcome::Failed
        }
        Err(_) => {
            tracing::debug!(source = name, "Metadata source timed out");
            Outcome::Failed
        }
    })
}

pub async fn info_post(
    State(state): State<AppState>,
    Json(seed): Json<MediaInfo>,
) -> Json<MediaInfo> {
    Json(state.0.metadata.lookup(seed).await)
}

pub async fn metadata_get(State(state): State<AppState>) -> Json<MetadataSettings> {
    Json(state.0.metadata.public())
}

/// Loopback only, but without the launcher's header: the dashboard sends it.
/// The JSON body forces a CORS preflight, which this service never approves,
/// so a page on another origin cannot send it either.
pub async fn metadata_put(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(put): Json<MetadataPut>,
) -> Result<Json<MetadataSettings>, ApiError> {
    state.0.metadata.apply(peer, put).await.map(Json)
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/mod.rs"]
mod tests;
