//! Online subtitles for whatever is playing. `POST /api/subtitles/search`
//! asks Gestdown (TV, from Addic7ed), TVsubtitles (TV), Subtitle Cat (both, machine translations marked) and, each
//! with the user's own API key, OpenSubtitles, SubDL and SubSource, side by
//! side within about 8 seconds, and ranks what they list. A source without
//! a key is skipped. `GET /api/subtitles/file/{id}` downloads one, turns it
//! into WebVTT and shifts it to the stream's start. `GET`/`PUT
//! /api/subtitles/settings` hold the preferred languages, the
//! hearing-impaired preference, the OpenSubtitles key and sign-in and the
//! SubDL and SubSource keys, in `%LOCALAPPDATA%\Portal\subtitles.json`.
//! Secrets are never sent back or logged.
//!
//! Searching sends the title (and season and episode) to those services;
//! the playing file's name is only used here, to rank.

mod convert;
mod gestdown;
mod langs;
mod opensubtitles;
mod rank;
mod settings;
mod store;
mod subdl;
mod subsource;
mod subtitlecat;
mod tvsubtitles;

use std::collections::HashMap;
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Path as UrlPath, Query as UrlQuery, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use portal_api::{
    MediaInfo, MediaKind, SubtitleResults, SubtitleSearch, SubtitleSettings, SubtitleSettingsPut,
};
use serde::Deserialize;

use crate::metadata::{net, Metadata};
use crate::{fail, ApiError, AppState};
use rank::{Found, Source};
pub(crate) use settings::{is_protected, protect, reveal};

/// One source's whole search, a show or movie lookup included.
const SOURCE_LIMIT: Duration = Duration::from_secs(8);
/// Fetching and converting one subtitle.
const DOWNLOAD_LIMIT: Duration = Duration::from_secs(20);
/// Checking a new key or sign-in.
const CHECK_LIMIT: Duration = Duration::from_secs(10);
/// The largest subtitle download read.
pub const FILE_CAP: usize = 8 << 20;
/// The most results one search returns.
const RESULT_LIMIT: usize = 40;
/// A signed-in OpenSubtitles session is reused this long.
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// The furthest a file may be shifted, in seconds.
const OFFSET_LIMIT: f64 = 48.0 * 3600.0;

const NO_KEY: &str =
    "Add an OpenSubtitles API key under Server → Subtitles to use subtitles from OpenSubtitles.";
const NO_SUBSOURCE_KEY: &str =
    "Add a SubSource API key under Server → Subtitles to use subtitles from SubSource.";
const NEED_KEY_FIRST: &str = "Save an OpenSubtitles API key before signing in.";
const LOCAL_ONLY: &str = "Subtitle settings can only be changed on this PC.";
const UNKNOWN_LANGUAGES: &str = "Choose languages Portal knows, such as en, fr or pt-BR.";

/// Why a source gave no list.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceError {
    /// It could not be asked or answered with an error.
    Failed,
    /// It said why, e.g. a used-up quota; shown to the user.
    Note(String),
}

impl From<net::Failed> for SourceError {
    fn from(_: net::Failed) -> Self {
        SourceError::Failed
    }
}

/// What the page asked for, cleaned.
#[derive(Debug, Clone, Default, PartialEq)]
struct Query {
    movie: bool,
    episode: Option<(u32, u32)>,
    title: Option<String>,
    show: Option<String>,
    year: Option<u32>,
    imdb: Option<String>,
    tmdb: Option<String>,
    tvdb: Option<String>,
    release: Option<String>,
}

fn text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|value| !value.is_empty() && value.chars().count() <= 300)
}

fn digits(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_string()).filter(|value| {
        (1..=12).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
    })
}

impl Query {
    fn from_search(search: SubtitleSearch) -> Self {
        let numbers = search.season.zip(search.episode);
        let episode = match search.kind {
            Some(MediaKind::Episode) | Some(MediaKind::Tv) | None => numbers,
            Some(MediaKind::Movie) => None,
        };
        let movie =
            search.kind == Some(MediaKind::Movie) || (search.kind.is_none() && numbers.is_none());
        let title = text(search.title);
        let show = text(search.show_title).or_else(|| title.clone().filter(|_| episode.is_some()));
        Self {
            movie,
            episode,
            title,
            show,
            year: search.year.filter(|year| (1870..=2200).contains(year)),
            imdb: search.imdb_id.as_deref().and_then(portal_api::imdb_id),
            tmdb: digits(search.tmdb_id),
            tvdb: digits(search.tvdb_id),
            release: text(search.release),
        }
    }

    /// What `POST /api/info` is asked, to learn a show's or movie's ids.
    fn seed(&self) -> Option<MediaInfo> {
        if self.episode.is_some() {
            // Only the name: an episode's ids are not the show's.
            return Some(MediaInfo {
                kind: Some(MediaKind::Tv),
                title: Some(self.show.clone()?),
                ..MediaInfo::default()
            });
        }
        if self.movie && (self.title.is_some() || self.tmdb.is_some()) {
            return Some(MediaInfo {
                kind: Some(MediaKind::Movie),
                title: self.title.clone(),
                year: self.year,
                tmdb_id: self.tmdb.clone(),
                ..MediaInfo::default()
            });
        }
        None
    }
}

/// The show's or movie's ids from the info lookup, asked at most once per
/// search and only when a source needs them.
struct Resolver<'a> {
    metadata: &'a Metadata,
    seed: Option<MediaInfo>,
    found: tokio::sync::OnceCell<MediaInfo>,
}

impl Resolver<'_> {
    async fn get(&self) -> &MediaInfo {
        self.found
            .get_or_init(|| async {
                match self.seed.clone() {
                    Some(seed) => self.metadata.lookup(seed).await,
                    None => MediaInfo::default(),
                }
            })
            .await
    }
}

pub struct Subtitles {
    settings_path: PathBuf,
    settings: Mutex<settings::Stored>,
    client: Option<reqwest::Client>,
    /// For OpenSubtitles and SubSource, whose keys go in a header.
    keyed: Option<reqwest::Client>,
    searches: store::Searches,
    /// Shared with the blocking thread that converts and writes a file.
    files: Arc<store::Files>,
    /// TVsubtitles' list of shows, read at most once a day.
    tv_shows: tvsubtitles::ShowList,
    /// Recent results by id, for a download's language and episode.
    listed: Mutex<HashMap<String, Found>>,
    session: tokio::sync::Mutex<Option<(opensubtitles::Session, Instant)>>,
    /// One settings change at a time: a change waiting seconds on a key
    /// check would otherwise save over one made meanwhile.
    applying: tokio::sync::Mutex<()>,
}

impl Subtitles {
    pub fn open(portal_dir: &Path) -> Self {
        let settings_path = portal_dir.join("subtitles.json");
        let cache_dir = portal_dir.join("subtitles");
        Self {
            settings: Mutex::new(settings::load(&settings_path)),
            settings_path,
            client: net::client(),
            keyed: net::keyed_client(),
            searches: store::Searches::default(),
            files: Arc::new(store::Files::open(Some(&cache_dir))),
            tv_shows: tvsubtitles::ShowList::open(Some(&cache_dir)),
            listed: Mutex::new(HashMap::new()),
            session: tokio::sync::Mutex::new(None),
            applying: tokio::sync::Mutex::new(()),
        }
    }

    fn stored(&self) -> settings::Stored {
        self.settings
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    pub fn public(&self) -> SubtitleSettings {
        let stored = self.stored();
        SubtitleSettings {
            languages: stored.languages.clone(),
            hearing_impaired: stored.hearing_impaired,
            opensubtitles: stored.opensubtitles_key.is_some(),
            opensubtitles_user: stored.has_login(),
            subdl: stored.subdl_key.is_some(),
            subsource: stored.subsource_key.is_some(),
        }
    }

    /// `POST /api/subtitles/search`.
    pub async fn search(&self, metadata: &Metadata, search: SubtitleSearch) -> SubtitleResults {
        let stored = self.stored();
        let asked = langs::normalize(&search.languages);
        let languages = if !asked.is_empty() {
            asked
        } else if !stored.languages.is_empty() {
            stored.languages.clone()
        } else {
            vec!["en".to_string()]
        };
        let query = Query::from_search(search);
        let (Some(client), Some(keyed)) = (&self.client, &self.keyed) else {
            return SubtitleResults::default();
        };
        let resolver = Resolver {
            metadata,
            seed: query.seed(),
            found: tokio::sync::OnceCell::new(),
        };
        let key = stored.opensubtitles_key.as_deref();
        let subdl_key = stored.subdl_key.as_deref();
        let subsource_key = stored.subsource_key.as_deref();
        let movie_or_episode = query.movie || query.episode.is_some();
        let started = Instant::now();
        let episode_with_show = query.episode.is_some() && query.show.is_some();
        let (
            from_gestdown,
            from_tvsubtitles,
            from_subtitlecat,
            from_opensubtitles,
            from_subdl,
            from_subsource,
        ) = tokio::join!(
            self.run(
                episode_with_show,
                Source::Gestdown,
                self.cached(
                    gestdown_key(&query, &languages),
                    gestdown_search(client, &query, &resolver, &languages),
                ),
            ),
            self.run(
                episode_with_show,
                Source::TvSubtitles,
                tvsubtitles_search(self, client, &query, &languages),
            ),
            self.run(
                episode_with_show || (query.movie && query.title.is_some()),
                Source::SubtitleCat,
                subtitlecat_search(self, client, &query, &languages),
            ),
            self.run(
                key.is_some() && (query.movie || query.episode.is_some()),
                Source::OpenSubtitles,
                opensubtitles_search(
                    self,
                    keyed,
                    key.unwrap_or(""),
                    &query,
                    &resolver,
                    &languages
                ),
            ),
            self.run(
                subdl_key.is_some() && movie_or_episode,
                Source::Subdl,
                subdl_search(
                    self,
                    client,
                    subdl_key.unwrap_or(""),
                    &query,
                    &resolver,
                    &languages
                ),
            ),
            self.run(
                subsource_key.is_some() && movie_or_episode,
                Source::Subsource,
                subsource_search(
                    self,
                    keyed,
                    subsource_key.unwrap_or(""),
                    &query,
                    &resolver,
                    &languages
                ),
            ),
        );
        let mut found: Vec<Found> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        for answer in [
            from_gestdown,
            from_tvsubtitles,
            from_subtitlecat,
            from_opensubtitles,
            from_subdl,
            from_subsource,
        ]
        .into_iter()
        .flatten()
        {
            match answer {
                Ok(list) => found.extend(list),
                Err(Some(note)) => notes.push(note),
                Err(None) => {}
            }
        }
        // A whole-season subtitle zip shares one id across its episodes; the id handed out
        // names the episode, so fetching it later never depends on which
        // episode was searched last.
        for item in &mut found {
            if item.episode.is_some() && split_episode(&item.id).1.is_none() {
                item.id = file_key(&item.id, item.episode);
            }
        }
        self.remember(&found);
        let prefs = rank::Prefs {
            languages: &languages,
            hearing_impaired: stored.hearing_impaired,
            release: query.release.as_deref(),
        };
        let mut items = rank::rank(&found, &prefs);
        items.truncate(RESULT_LIMIT);
        tracing::debug!(
            results = items.len(),
            millis = started.elapsed().as_millis() as u64,
            "Subtitle search finished"
        );
        SubtitleResults {
            best: items.first().map(|item| item.id.clone()),
            items,
            notes,
        }
    }

    /// Runs one source when it applies, within `SOURCE_LIMIT`. Each source's
    /// search lives on the heap: kept inline, seven of them made one search a
    /// 225 KB future, and moving that through the request handler overflowed
    /// a worker thread's stack.
    fn run<'a, F>(
        &'a self,
        wanted: bool,
        source: Source,
        search: F,
    ) -> std::pin::Pin<Box<dyn Future<Output = Option<store::Answer>> + Send + 'a>>
    where
        F: Future<Output = Result<Vec<Found>, SourceError>> + Send + 'a,
    {
        Box::pin(async move {
            if !wanted {
                return None;
            }
            Some(match tokio::time::timeout(SOURCE_LIMIT, search).await {
                Ok(Ok(found)) => Ok(found),
                Ok(Err(SourceError::Note(note))) => Err(Some(note)),
                Ok(Err(SourceError::Failed)) => {
                    tracing::debug!(source = source.name(), "Subtitle source failed");
                    Err(None)
                }
                Err(_) => {
                    tracing::debug!(source = source.name(), "Subtitle source timed out");
                    Err(None)
                }
            })
        })
    }

    /// A source's answer from the cache, or asked now and kept.
    async fn cached<F>(&self, key: String, search: F) -> Result<Vec<Found>, SourceError>
    where
        F: Future<Output = Result<Vec<Found>, SourceError>>,
    {
        if let Some(answer) = self.searches.get(&key) {
            return answer.map_err(|note| note.map_or(SourceError::Failed, SourceError::Note));
        }
        let answer = search.await;
        let kept: store::Answer = match &answer {
            Ok(found) => Ok(found.clone()),
            Err(SourceError::Note(note)) => Err(Some(note.clone())),
            Err(SourceError::Failed) => Err(None),
        };
        self.searches.put(&key, kept);
        answer
    }

    fn remember(&self, found: &[Found]) {
        let mut listed = self
            .listed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if listed.len() > 3000 {
            listed.clear();
        }
        for item in found {
            listed.insert(item.id.clone(), item.clone());
        }
    }

    /// `GET /api/subtitles/file/{id}`: WebVTT with every cue moved `offset`
    /// seconds earlier, so it lines up with a stream that starts there.
    pub async fn file(&self, id: &str, offset: f64) -> Result<String, ApiError> {
        let vtt = self.vtt(id).await?;
        if offset == 0.0 {
            return Ok(vtt.as_str().to_string());
        }
        Ok(drop_past_cues(&portal_engine::shift_vtt(&vtt, -offset)))
    }

    /// True when this subtitle is already stored, so serving it downloads nothing.
    pub fn already_downloaded(&self, id: &str) -> bool {
        if parse_id(id).is_none() {
            return false;
        }
        let listed = self
            .listed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned();
        let (base, named) = split_episode(id);
        let episode = named.or_else(|| listed.as_ref().and_then(|item| item.episode));
        self.files.get(&file_key(base, episode)).is_some()
    }

    async fn vtt(&self, id: &str) -> Result<Arc<String>, ApiError> {
        let (source, rest) = parse_id(id).ok_or_else(unknown_subtitle)?;
        let listed = self
            .listed
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(id)
            .cloned();
        let (base, named) = split_episode(id);
        let episode = named.or_else(|| listed.as_ref().and_then(|item| item.episode));
        let key = file_key(base, episode);
        if let Some(text) = self.files.get(&key) {
            return Ok(text);
        }
        let (Some(client), Some(keyed)) = (&self.client, &self.keyed) else {
            return Err(unreachable_source(source));
        };
        let download = self.download(client, keyed, source, rest);
        let bytes = tokio::time::timeout(DOWNLOAD_LIMIT, download)
            .await
            .map_err(|_| unreachable_source(source))?
            .map_err(|err| match err {
                SourceError::Note(note) => fail(StatusCode::BAD_GATEWAY, note),
                SourceError::Failed => unreachable_source(source),
            })?;
        let language = listed.map(|item| item.language);
        // Writing the file and pruning the folder are disk work too, kept off
        // the async workers along with the conversion.
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            let text = Arc::new(convert::to_vtt(&bytes, episode, language.as_deref()).ok()?);
            files.put(&key, text.clone());
            Some(text)
        })
        .await
        .map_err(|_| unusable())?
        .ok_or_else(unusable)
    }

    async fn download(
        &self,
        client: &reqwest::Client,
        keyed: &reqwest::Client,
        source: Source,
        id: &str,
    ) -> Result<Vec<u8>, SourceError> {
        match source {
            Source::Gestdown => gestdown::download(client, id).await,
            Source::TvSubtitles => tvsubtitles::download(client, id).await,
            Source::SubtitleCat => subtitlecat::download(client, id).await,
            Source::Subdl => subdl::download(client, id).await,
            Source::Subsource => {
                let stored = self.stored();
                let Some(key) = stored.subsource_key.as_deref() else {
                    return Err(SourceError::Note(NO_SUBSOURCE_KEY.into()));
                };
                subsource::download(keyed, client, key, id).await
            }
            Source::OpenSubtitles => {
                let stored = self.stored();
                let Some(key) = stored.opensubtitles_key.as_deref() else {
                    return Err(SourceError::Note(NO_KEY.into()));
                };
                let session = self.session(keyed, &stored).await;
                let result =
                    opensubtitles::download(keyed, client, key, session.as_ref(), id).await;
                if result.is_err() && session.is_some() {
                    // An expired sign-in is renewed on the next try.
                    *self.session.lock().await = None;
                }
                result
            }
        }
    }

    /// The signed-in session, when a username and password are saved.
    async fn session(
        &self,
        client: &reqwest::Client,
        stored: &settings::Stored,
    ) -> Option<opensubtitles::Session> {
        let (Some(key), Some(username), Some(password)) = (
            stored.opensubtitles_key.as_deref(),
            stored.opensubtitles_username.as_deref(),
            stored.opensubtitles_password.as_deref(),
        ) else {
            return None;
        };
        let mut current = self.session.lock().await;
        if let Some((session, at)) = current.as_ref() {
            if at.elapsed() < SESSION_TTL {
                return Some(session.clone());
            }
        }
        match opensubtitles::login(client, key, username, password).await {
            Ok(session) => {
                *current = Some((session.clone(), Instant::now()));
                Some(session)
            }
            Err(_) => {
                tracing::info!("OpenSubtitles sign-in failed; downloading without it");
                None
            }
        }
    }

    /// `PUT /api/subtitles/settings`: loopback peers only. A new key is
    /// checked with its service, and a new sign-in tried, before either is
    /// saved; `null` or a blank value removes it.
    pub async fn apply(
        &self,
        peer: SocketAddr,
        put: SubtitleSettingsPut,
    ) -> Result<SubtitleSettings, ApiError> {
        if !crate::from_this_pc(peer) {
            return Err(fail(StatusCode::FORBIDDEN, LOCAL_ONLY));
        }
        let _applying = self.applying.lock().await;
        let mut next = self.stored();
        if let Some(list) = &put.languages {
            let normalized = langs::normalize(list);
            if normalized.is_empty() && list.iter().any(|item| !item.trim().is_empty()) {
                return Err(fail(StatusCode::BAD_REQUEST, UNKNOWN_LANGUAGES));
            }
            next.languages = normalized;
        }
        if let Some(hearing) = put.hearing_impaired {
            next.hearing_impaired = hearing;
        }
        let trimmed = |value: &Option<Option<String>>| {
            value.as_ref().map(|value| {
                value
                    .as_ref()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
        };
        let key = trimmed(&put.opensubtitles_key);
        let subdl_key = trimmed(&put.subdl_key);
        let subsource_key = trimmed(&put.subsource_key);
        let username = trimmed(&put.opensubtitles_username);
        // A password keeps its spaces; only an empty one removes it.
        let password = put
            .opensubtitles_password
            .as_ref()
            .map(|value| value.clone().filter(|value| !value.is_empty()));
        let key_changed = key
            .as_ref()
            .is_some_and(|key| *key != next.opensubtitles_key);
        if let Some(key) = key {
            next.opensubtitles_key = key;
        }
        let subdl_changed = subdl_key.as_ref().is_some_and(|key| *key != next.subdl_key);
        if let Some(key) = subdl_key {
            next.subdl_key = key;
        }
        let subsource_changed = subsource_key
            .as_ref()
            .is_some_and(|key| *key != next.subsource_key);
        if let Some(key) = subsource_key {
            next.subsource_key = key;
        }
        let login_changed = username
            .as_ref()
            .is_some_and(|value| *value != next.opensubtitles_username)
            || password
                .as_ref()
                .is_some_and(|value| *value != next.opensubtitles_password);
        if let Some(username) = username {
            next.opensubtitles_username = username;
        }
        if let Some(password) = password {
            next.opensubtitles_password = password;
        }
        if key_changed {
            if let Some(key) = &next.opensubtitles_key {
                self.check_key(Source::OpenSubtitles, key).await?;
            }
        }
        if subdl_changed {
            if let Some(key) = &next.subdl_key {
                self.check_key(Source::Subdl, key).await?;
            }
        }
        if subsource_changed {
            if let Some(key) = &next.subsource_key {
                self.check_key(Source::Subsource, key).await?;
            }
        }
        let mut session = None;
        if let (true, Some(username), Some(password)) = (
            login_changed || key_changed,
            next.opensubtitles_username.as_deref(),
            next.opensubtitles_password.as_deref(),
        ) {
            let Some(key) = next.opensubtitles_key.as_deref() else {
                return Err(fail(StatusCode::BAD_REQUEST, NEED_KEY_FIRST));
            };
            session = Some(self.check_login(key, username, password).await?);
        }
        {
            let mut stored = self
                .settings
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            settings::save(&self.settings_path, &next).map_err(|err| {
                tracing::warn!(error = %err, "Could not save subtitle settings");
                fail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Portal could not save that setting.",
                )
            })?;
            *stored = next;
        }
        if key_changed || login_changed {
            *self.session.lock().await = session.map(|session| (session, Instant::now()));
        }
        if key_changed || subdl_changed || subsource_changed {
            // A source's results appear or go with its key.
            self.searches.clear();
        }
        for (changed, source) in [
            (key_changed, Source::OpenSubtitles),
            (subdl_changed, Source::Subdl),
            (subsource_changed, Source::Subsource),
        ] {
            if changed {
                tracing::info!(source = source.name(), "Subtitle API key updated");
            }
        }
        Ok(self.public())
    }

    /// A new key, checked with its service before it is saved.
    async fn check_key(&self, source: Source, key: &str) -> Result<(), ApiError> {
        let (refused, plausible) = match source {
            Source::Subdl => (subdl::KEY_REFUSED, plausible_token(key)),
            Source::Subsource => (subsource::KEY_REFUSED, plausible_token(key)),
            _ => (opensubtitles::KEY_REFUSED, plausible_key(key)),
        };
        if !plausible {
            return Err(fail(StatusCode::BAD_REQUEST, refused));
        }
        let (Some(client), Some(keyed)) = (&self.client, &self.keyed) else {
            return Err(unreachable_check(source));
        };
        let check = async {
            match source {
                Source::Subdl => subdl::check_key(client, key).await,
                Source::Subsource => subsource::check_key(keyed, key).await,
                _ => opensubtitles::check_key(keyed, key).await,
            }
        };
        match tokio::time::timeout(CHECK_LIMIT, check).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(SourceError::Note(note))) => Err(fail(StatusCode::BAD_REQUEST, note)),
            _ => Err(unreachable_check(source)),
        }
    }

    async fn check_login(
        &self,
        key: &str,
        username: &str,
        password: &str,
    ) -> Result<opensubtitles::Session, ApiError> {
        let Some(keyed) = &self.keyed else {
            return Err(unreachable_check(Source::OpenSubtitles));
        };
        let login = opensubtitles::login(keyed, key, username, password);
        match tokio::time::timeout(CHECK_LIMIT, login).await {
            Ok(Ok(session)) => Ok(session),
            Ok(Err(SourceError::Note(note))) => Err(fail(StatusCode::BAD_REQUEST, note)),
            _ => Err(unreachable_check(Source::OpenSubtitles)),
        }
    }
}

/// OpenSubtitles keys are 32 letters and digits; allow some slack.
fn plausible_key(key: &str) -> bool {
    (16..=64).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// SubDL and SubSource do not say what their keys look like: visible ASCII
/// without spaces, which a query string or a header can carry.
fn plausible_token(key: &str) -> bool {
    (8..=200).contains(&key.len()) && key.bytes().all(|byte| byte.is_ascii_graphic())
}

/// "gestdown:<uuid>", "opensubtitles:<file id>",
/// "subdl:<zip name>", "subsource:<subtitle id>", "tvsubtitles:<subtitle
/// number>" or "subtitlecat:subs/<number>/<file name>.srt".
/// Two language codes for the same language ("en" and "eng", "pt" and "pt-BR").
pub fn same_language(a: &str, b: &str) -> bool {
    match (langs::find(a), langs::find(b)) {
        (Some(left), Some(right)) => langs::same_family(left.code, right.code),
        _ => langs::same_family(a, b),
    }
}

/// The code Portal uses for a language named any known way ("eng" → "en").
pub fn language_code(text: &str) -> Option<&'static str> {
    langs::find(text).map(|language| language.code)
}

/// A subtitle id the file endpoint would serve; a cast hands its address to a TV.
pub fn valid_file_id(id: &str) -> bool {
    parse_id(id).is_some()
}

fn parse_id(id: &str) -> Option<(Source, &str)> {
    let (prefix, rest) = split_episode(id).0.split_once(':')?;
    let source = Source::from_prefix(prefix)?;
    let valid = match source {
        Source::Gestdown => gestdown::valid_id(rest),
        Source::OpenSubtitles => opensubtitles::valid_id(rest),
        Source::Subdl => subdl::valid_id(rest),
        Source::Subsource => subsource::valid_id(rest),
        Source::TvSubtitles => tvsubtitles::valid_id(rest),
        Source::SubtitleCat => subtitlecat::valid_id(rest),
    };
    valid.then_some((source, rest))
}

/// An id and the episode its `@S01E02` ending names, if it has one.
fn split_episode(id: &str) -> (&str, Option<(u32, u32)>) {
    let named = id.rsplit_once('@').and_then(|(base, tail)| {
        let (season, number) = tail.strip_prefix('S')?.split_once('E')?;
        let digits =
            |text: &str| (1..=4).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit());
        if !digits(season) || !digits(number) {
            return None;
        }
        Some((base, (season.parse().ok()?, number.parse().ok()?)))
    });
    match named {
        Some((base, episode)) => (base, Some(episode)),
        None => (id, None),
    }
}

/// The converted file's cache key: a whole-season subtitle zip shares one id across its
/// episodes, so the episode is part of it.
fn file_key(id: &str, episode: Option<(u32, u32)>) -> String {
    match episode {
        Some((season, number)) => format!("{id}@S{season:02}E{number:02}"),
        None => id.to_string(),
    }
}

/// What a subtitle's release names say about the asked-for episode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpisodeNamed {
    /// One of them names it.
    This,
    /// Some name another episode, none this one.
    Other,
    /// None names an episode.
    Nothing,
}

/// Reads "S01E02" or "1x02", in any case, in each release name.
fn release_episode(names: &[String], want: (u32, u32)) -> EpisodeNamed {
    let named: Vec<(u32, u32)> = names
        .iter()
        .filter_map(|name| convert::episode_in_name(name))
        .collect();
    if named.contains(&want) {
        EpisodeNamed::This
    } else if named.is_empty() {
        EpisodeNamed::Nothing
    } else {
        EpisodeNamed::Other
    }
}

/// Shifting clamps cues that ended before the stream's start to an empty
/// cue at 0; they are left out instead.
fn drop_past_cues(vtt: &str) -> String {
    const PAST: &str = "00:00:00.000 --> 00:00:00.000";
    let mut out = String::with_capacity(vtt.len());
    for block in vtt.split("\n\n") {
        if block.trim().is_empty() || block.trim_start().starts_with(PAST) {
            continue;
        }
        out.push_str(block.trim_matches('\n'));
        out.push_str("\n\n");
    }
    out
}

fn unknown_subtitle() -> ApiError {
    fail(StatusCode::NOT_FOUND, "Portal does not know that subtitle.")
}

fn unusable() -> ApiError {
    fail(
        StatusCode::UNPROCESSABLE_ENTITY,
        "That subtitle file could not be read. Pick another one.",
    )
}

fn unreachable_source(source: Source) -> ApiError {
    fail(
        StatusCode::BAD_GATEWAY,
        format!(
            "Portal could not download that subtitle from {}. Try again or pick another one.",
            source.name()
        ),
    )
}

fn unreachable_check(source: Source) -> ApiError {
    fail(
        StatusCode::BAD_GATEWAY,
        format!(
            "Portal could not reach {} to check that. Try again.",
            source.name()
        ),
    )
}

fn gestdown_key(query: &Query, languages: &[String]) -> String {
    let (season, number) = query.episode.unwrap_or_default();
    format!(
        "gestdown:{}:{}:{}:{season}x{number}:{}",
        crate::metadata::text::norm_title(query.show.as_deref().unwrap_or("")),
        query.tvdb.as_deref().unwrap_or(""),
        query.tmdb.as_deref().unwrap_or(""),
        languages.join(",")
    )
}

async fn gestdown_search(
    client: &reqwest::Client,
    query: &Query,
    resolver: &Resolver<'_>,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let (Some((season, number)), Some(show)) = (query.episode, query.show.as_deref()) else {
        return Ok(Vec::new());
    };
    let number_of = |id: &Option<String>| id.as_deref().and_then(|id| id.parse::<u64>().ok());
    let shows = gestdown::search_shows(client, show).await?;
    let mut hints = gestdown::ShowHints {
        name: show.to_string(),
        tvdb: number_of(&query.tvdb).into_iter().collect(),
        tmdb: number_of(&query.tmdb).into_iter().collect(),
    };
    let mut picked = gestdown::pick_show(&shows, &hints);
    if picked.is_none() {
        let resolved = resolver.get().await;
        hints.tvdb.extend(number_of(&resolved.tvdb_id));
        hints.tmdb.extend(number_of(&resolved.tmdb_id));
        picked = gestdown::pick_show(&shows, &hints);
    }
    let Some(show_id) = picked else {
        return Ok(Vec::new());
    };
    let mut results = Vec::new();
    // One language at a time, to go easy on a free service.
    for language in languages {
        results.push(gestdown::episode(client, &show_id, season, number, language).await);
    }
    merge_languages(results)
}

/// A source asked once per language keeps what the languages that answered
/// found. It fails only when every language failed, with a reason when one
/// of them gave it.
fn merge_languages<T>(results: Vec<Result<Vec<T>, SourceError>>) -> Result<Vec<T>, SourceError> {
    let mut found = Vec::new();
    let mut answered = results.is_empty();
    let mut failure = SourceError::Failed;
    for result in results {
        match result {
            Ok(rows) => {
                answered = true;
                found.extend(rows);
            }
            Err(note @ SourceError::Note(_)) if failure == SourceError::Failed => failure = note,
            Err(_) => {}
        }
    }
    if answered {
        Ok(found)
    } else {
        Err(failure)
    }
}

fn tvsubtitles_key(query: &Query, languages: &[String]) -> String {
    let (season, number) = query.episode.unwrap_or_default();
    format!(
        "tvsubtitles:{}:{}:{}:{season}x{number}:{}",
        crate::metadata::text::norm_title(query.show.as_deref().unwrap_or("")),
        query.year.unwrap_or_default(),
        crate::metadata::text::norm_title(query.title.as_deref().unwrap_or("")),
        languages.join(",")
    )
}

async fn tvsubtitles_search(
    subtitles: &Subtitles,
    client: &reqwest::Client,
    query: &Query,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let (Some(episode), Some(show)) = (query.episode, query.show.as_deref()) else {
        return Ok(Vec::new());
    };
    let search = tvsubtitles::Search {
        show,
        year: query.year,
        // The title is the show's own when the page sent no show title.
        title: query.title.as_deref().filter(|title| *title != show),
        episode,
    };
    subtitles
        .cached(
            tvsubtitles_key(query, languages),
            tvsubtitles::search(client, &subtitles.tv_shows, &search, languages),
        )
        .await
}

async fn subtitlecat_search(
    subtitles: &Subtitles,
    client: &reqwest::Client,
    query: &Query,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let search = match (query.episode, &query.show, &query.title) {
        (Some(episode), Some(show), _) => subtitlecat::Search {
            title: show.clone(),
            year: None,
            episode: Some(episode),
        },
        (None, _, Some(title)) if query.movie => subtitlecat::Search {
            title: title.clone(),
            year: query.year,
            episode: None,
        },
        _ => return Ok(Vec::new()),
    };
    let key = format!(
        "subtitlecat:{}:{}",
        subtitlecat::query_text(&search).to_lowercase(),
        languages.join(",")
    );
    subtitles
        .cached(key, subtitlecat::search(client, &search, languages))
        .await
}

async fn opensubtitles_search(
    subtitles: &Subtitles,
    client: &reqwest::Client,
    key: &str,
    query: &Query,
    resolver: &Resolver<'_>,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let digits = |id: &Option<String>| id.as_deref().and_then(opensubtitles::imdb_digits);
    let search = if let Some((season, number)) = query.episode {
        let parent = digits(&resolver.get().await.imdb_id);
        opensubtitles::Search {
            movie: false,
            query: query.show.clone().filter(|_| parent.is_none()),
            parent_imdb: parent,
            season: Some(season),
            episode: Some(number),
            ..opensubtitles::Search::default()
        }
    } else {
        let mut imdb = digits(&query.imdb);
        if imdb.is_none() && query.tmdb.is_none() {
            imdb = digits(&resolver.get().await.imdb_id);
        }
        opensubtitles::Search {
            movie: true,
            imdb,
            tmdb: query.tmdb.clone(),
            query: query.title.clone(),
            year: query.year,
            ..opensubtitles::Search::default()
        }
    };
    if search.imdb.is_none()
        && search.parent_imdb.is_none()
        && search.tmdb.is_none()
        && search.query.is_none()
    {
        return Ok(Vec::new());
    }
    let pairs = opensubtitles::params(&search, languages);
    let cache_key = format!(
        "opensubtitles:{}",
        pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    );
    subtitles
        .cached(
            cache_key,
            opensubtitles::search(client, key, &search, languages),
        )
        .await
}

/// The show's IMDb id for an episode (an episode's own id is not the
/// show's), else the movie's, from the page or the info lookup.
async fn show_or_movie_imdb(query: &Query, resolver: &Resolver<'_>) -> Option<String> {
    match (&query.imdb, query.episode) {
        (Some(imdb), None) => Some(imdb.clone()),
        _ => resolver.get().await.imdb_id.clone(),
    }
}

async fn subdl_search(
    subtitles: &Subtitles,
    client: &reqwest::Client,
    key: &str,
    query: &Query,
    resolver: &Resolver<'_>,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let episode = query.episode.filter(|_| !query.movie);
    let search = subdl::Search {
        movie: episode.is_none(),
        imdb: show_or_movie_imdb(query, resolver).await,
        name: if episode.is_some() {
            query.show.clone()
        } else {
            query.title.clone()
        },
        episode,
    };
    let Some(pairs) = subdl::params(&search, languages) else {
        return Ok(Vec::new());
    };
    let cache_key = format!(
        "subdl:{}",
        pairs
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&")
    );
    subtitles
        .cached(cache_key, subdl::search(client, key, &search, languages))
        .await
}

async fn subsource_search(
    subtitles: &Subtitles,
    client: &reqwest::Client,
    key: &str,
    query: &Query,
    resolver: &Resolver<'_>,
    languages: &[String],
) -> Result<Vec<Found>, SourceError> {
    let episode = query.episode.filter(|_| !query.movie);
    let search = subsource::Search {
        movie: episode.is_none(),
        imdb: show_or_movie_imdb(query, resolver).await,
        title: if episode.is_some() {
            query.show.clone()
        } else {
            query.title.clone()
        },
        year: query.year.filter(|_| episode.is_none()),
        episode,
    };
    let wanted = subsource::wanted(languages);
    if wanted.is_empty() || subsource::title_params(&search).is_none() {
        return Ok(Vec::new());
    }
    let (season, number) = episode.unwrap_or_default();
    let cache_key = format!(
        "subsource:{}:{}:{}:{season}x{number}:{}",
        search.imdb.as_deref().unwrap_or(""),
        crate::metadata::text::norm_title(search.title.as_deref().unwrap_or("")),
        search.year.unwrap_or_default(),
        wanted
            .iter()
            .map(|(code, _)| *code)
            .collect::<Vec<_>>()
            .join(",")
    );
    subtitles
        .cached(
            cache_key,
            subsource::search(client, key, &search, languages),
        )
        .await
}

pub async fn search_post(
    State(state): State<AppState>,
    Json(search): Json<SubtitleSearch>,
) -> Json<SubtitleResults> {
    Json(state.0.subtitles.search(&state.0.metadata, search).await)
}

#[derive(Deserialize)]
pub struct FileQuery {
    #[serde(default)]
    offset: Option<String>,
}

fn parse_offset(value: Option<&str>) -> Result<f64, ApiError> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(0.0);
    };
    match value.parse::<f64>() {
        Ok(offset) if offset.is_finite() && offset.abs() <= OFFSET_LIMIT => Ok(offset),
        _ => Err(fail(
            StatusCode::BAD_REQUEST,
            "The offset must be a number of seconds.",
        )),
    }
}

pub async fn file_get(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    UrlQuery(query): UrlQuery<FileQuery>,
    unpaired: Option<axum::Extension<crate::pairing::Unpaired>>,
) -> Result<Response, ApiError> {
    let offset = parse_offset(query.offset.as_deref())?;
    // A cast receiver that never paired gets only a file Portal already
    // fetched for a cast, never a new download on the user's accounts.
    if unpaired.is_some() && !state.0.subtitles.already_downloaded(&id) {
        return Err(unknown_subtitle());
    }
    let text = state.0.subtitles.file(&id, offset).await?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/vtt; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            // A Cast TV's player fetches it from its own origin.
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
        ],
        text,
    )
        .into_response())
}

pub async fn settings_get(State(state): State<AppState>) -> Json<SubtitleSettings> {
    Json(state.0.subtitles.public())
}

/// Loopback only. The JSON body forces a CORS preflight, which this service
/// never approves, so a page on another origin cannot send it either.
pub async fn settings_put(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(put): Json<SubtitleSettingsPut>,
) -> Result<Json<SubtitleSettings>, ApiError> {
    state.0.subtitles.apply(peer, put).await.map(Json)
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/mod.rs"]
mod tests;
