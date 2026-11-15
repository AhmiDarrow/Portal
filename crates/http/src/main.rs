//! Portal's local service. Port defaults to 8490 (override with PORTAL_PORT).
//! Bind host is loopback or LAN from cast.json.
//!
//! A Windows GUI program, same as the tray launcher, so starting the service
//! never opens a command prompt. The log is `%LOCALAPPDATA%\Portal\logs\portal.log`.

#![cfg_attr(windows, windows_subsystem = "windows")]

use std::collections::{HashMap, VecDeque};
use std::future::IntoFuture;
use std::io::SeekFrom;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncSeekExt};

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use portal_api::{
    AddonList, BrowsePage, CardList, HistoryList, Home, InstallResult, JobState, ListPut, OkBody,
    PlayAccepted, PlayJob, PlayKind, PlayRequest, ProgressReport, Prompt, PromptAction,
    PromptAnswer, PromptBody, PromptKind, RemoveResult, RepoDetail, RepoList, SavedMap, SavedQuery,
    SettingWrite, SettingsDoc, Source, Status, TitleDetail, TitleKind, WindowRequest,
};
use portal_engine::{
    browse, pack_identity, pick_source, picks_source, play_refusal, plugin_link, run_plugin_builtin,
    search, settings_addon, settings_name_hint, source_cap, video_addons, Addons,
    EngineError, Player, ProcessConfig, Prompts, RemoveOutcome,
    Settings, Supervisor,
};
use portal_library::{PlayRoute, Store, StoreError, TitleFacts};
use serde::Deserialize;
use serde_json::json;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

mod app;
mod backup;
mod cast;
mod guard;
mod job;
mod metadata;
mod packs;
mod pairing;
mod paths;
mod prefs;
mod subtitles;
mod update;

struct Inner {
    supervisor: Supervisor,
    store: Mutex<Store>,
    player: Player,
    settings: Settings,
    addons: Addons,
    prompts: Prompts,
    pairing: pairing::Pairing,
    cast: Mutex<cast::CastHub>,
    update: update::Updater,
    metadata: metadata::Metadata,
    subtitles: subtitles::Subtitles,
    prefs: Mutex<prefs::PrefsStore>,
    /// Plays whose copy Portal is choosing from the add-on's source list, with
    /// the picture height each is for.
    picking: Mutex<HashMap<String, u32>>,
    /// Whole-season files already chosen, so the next episode prefers the same file.
    packs: Mutex<packs::SeasonPacks>,
    /// Flips to true once when Portal starts shutting down.
    stopping: tokio::sync::watch::Sender<bool>,
}

#[derive(Clone)]
struct AppState(Arc<Inner>);

#[derive(Deserialize)]
struct PathQuery {
    path: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

#[derive(Deserialize)]
struct ImageQuery {
    path: String,
}

#[derive(serde::Serialize)]
struct ActionBody {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    browse_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings_addon: Option<String>,
    /// True when a RunPlugin action was started; the page should watch `/api/prompt`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    ran: bool,
}

struct CachedBrowse {
    at: Instant,
    page: BrowsePage,
}

const BROWSE_CACHE_TTL: Duration = Duration::from_secs(45);
const BROWSE_CACHE_CAP: usize = 24;

static BROWSE_CACHE: Mutex<VecDeque<(String, CachedBrowse)>> = Mutex::new(VecDeque::new());

fn cached_browse(path: &str) -> Option<BrowsePage> {
    let mut cache = BROWSE_CACHE.lock().unwrap_or_else(|err| err.into_inner());
    cache.retain(|(_, entry)| entry.at.elapsed() < BROWSE_CACHE_TTL);
    cache
        .iter()
        .find(|(key, _)| key == path)
        .map(|(_, entry)| entry.page.clone())
}

fn store_browse(path: &str, page: &BrowsePage) {
    if page.items.is_empty() {
        return;
    }
    if settings_addon(path, "").is_some() || settings_addon(&page.path, "").is_some() {
        return;
    }
    let mut cache = BROWSE_CACHE.lock().unwrap_or_else(|err| err.into_inner());
    cache.retain(|(key, entry)| key != path && entry.at.elapsed() < BROWSE_CACHE_TTL);
    cache.push_back((
        path.to_string(),
        CachedBrowse {
            at: Instant::now(),
            page: page.clone(),
        },
    ));
    while cache.len() > BROWSE_CACHE_CAP {
        cache.pop_front();
    }
}

/// A 5xx line names the method and path. The query is left out.
async fn note_server_error(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let response = next.run(request).await;
    if response.status().is_server_error() {
        tracing::error!(
            method = %method,
            path = %path,
            status = %response.status(),
            "request failed"
        );
    }
    response
}

fn main() {
    use std::io::IsTerminal;
    tracing_subscriber::fmt()
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    job::contain_children();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(serve());
    // Dropping a runtime waits for every blocking task, and the update task
    // blocks on the installer, which in turn waits for this process to exit.
    // Serving is over, so leave whatever is still running behind.
    runtime.shutdown_timeout(Duration::from_millis(500));
    if let Err(err) = result {
        tracing::error!("{err}");
        std::process::exit(1);
    }
}

async fn serve() -> Result<(), Box<dyn std::error::Error>> {
    let portal_dir = paths::portal_dir();
    std::fs::create_dir_all(&portal_dir)?;
    std::fs::create_dir_all(portal_dir.join("hls"))?;

    let kodi_exe = paths::kodi_exe();
    let profile = std::env::var_os("APPDATA")
        .map(|dir| PathBuf::from(dir).join("Kodi"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming\Kodi"));
    let programs_addons = kodi_exe
        .parent()
        .map(|dir| dir.join("addons"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\Kodi\addons"));

    let supervisor = Supervisor::start(ProcessConfig {
        host: "127.0.0.1".to_string(),
        port: 9090,
        kodi_exe,
        profile: profile.clone(),
        state_path: portal_dir.join("engine.json"),
    });
    let prompts = Prompts::with_addon_roots(
        supervisor.dialogs(),
        vec![profile.clone(), programs_addons.clone()],
    );
    let player = Player::new(paths::ffmpeg(), paths::ffprobe(), portal_dir.join("hls"));
    player.set_hardware(app::hardware_saved(&portal_dir));
    if let Some(encoder) = std::env::var("PORTAL_ENCODER")
        .ok()
        .and_then(|name| portal_engine::Encoder::named(&name))
    {
        tracing::info!("Encoder named by PORTAL_ENCODER: {}", encoder.name());
        player.assume_hardware(encoder);
    }
    backup::on_version_change(&portal_dir, env!("CARGO_PKG_VERSION"));
    let store = Store::open(portal_dir.join("library.json"))?;
    let cast_hub = cast::CastHub::open(&portal_dir);
    let bind = cast_hub.bind_addr();
    let cast_url = cast_hub.suggested_url();
    let state = AppState(Arc::new(Inner {
        supervisor,
        store: Mutex::new(store),
        player,
        settings: Settings::new(profile.clone(), programs_addons.clone()),
        addons: Addons::new(profile, programs_addons, portal_dir.join("staging")),
        prompts,
        pairing: pairing::Pairing::open(&portal_dir),
        cast: Mutex::new(cast_hub),
        update: update::Updater::new(&portal_dir),
        metadata: metadata::Metadata::open(&portal_dir),
        subtitles: subtitles::Subtitles::open(&portal_dir),
        prefs: Mutex::new(prefs::PrefsStore::open(&portal_dir)),
        picking: Mutex::new(HashMap::new()),
        packs: Mutex::new(packs::SeasonPacks::open(&portal_dir)),
        stopping: tokio::sync::watch::Sender::new(false),
    }));

    let mut app = Router::new()
        .route("/api/ping", get(ping))
        .route("/api/shutdown", post(shutdown))
        .route("/api/status", get(status))
        .route("/api/update", get(update::update_get))
        .route("/api/update/check", post(update::update_check))
        .route("/api/update/install", post(update::update_install))
        .route("/api/app", get(app::app_get))
        .route("/api/app/autostart", post(app::app_autostart))
        .route("/api/app/encoder", post(app::app_encoder))
        .route("/api/app/restart", post(app::app_restart))
        .route("/api/app/quit", post(app::app_quit))
        .route("/api/app/logs", post(app::app_logs))
        .route("/api/engine/restart", post(engine_restart))
        .route("/api/engine/quit", post(engine_quit))
        .route("/api/engine/window", post(engine_window))
        .route("/api/source", get(get_source).put(put_source))
        .route("/api/home", get(home))
        .route("/api/browse", get(browse_path))
        .route("/api/search", get(search_path))
        .route("/api/title/{id}", get(title))
        .route("/api/watchlist", get(watchlist))
        .route("/api/watchlist/{id}", post_put_delete_watch())
        .route("/api/bookmarks", get(bookmarks))
        .route("/api/bookmarks/{id}", post_put_delete_book())
        .route("/api/saved", post(saved))
        .route("/api/info", post(metadata::info_post))
        .route(
            "/api/metadata",
            get(metadata::metadata_get).put(metadata::metadata_put),
        )
        .route("/api/subtitles/search", post(subtitles::search_post))
        .route("/api/subtitles/file/{id}", get(subtitles::file_get))
        .route(
            "/api/subtitles/settings",
            get(subtitles::settings_get).put(subtitles::settings_put),
        )
        .route(
            "/api/prefs",
            get(prefs::prefs_get)
                .put(prefs::prefs_put)
                .layer(DefaultBodyLimit::max(prefs::BODY_LIMIT)),
        )
        .route("/api/history", get(history).delete(history_clear))
        .route(
            "/api/history/{slot}",
            axum::routing::put(history_put).delete(history_delete),
        )
        .route("/api/progress", post(progress))
        .route("/api/addons", get(addons))
        .route("/api/addons/{id}", delete(addon_remove))
        .route("/api/addons/{id}/settings", get(settings_form))
        .route(
            "/api/addons/{id}/settings/{setting_id}",
            axum::routing::put(settings_write),
        )
        .route(
            "/api/addons/{id}/actions/{setting_id}",
            post(settings_action),
        )
        .route("/api/run", post(run_plugin))
        .route(
            "/api/incoming",
            post(incoming_file).layer(DefaultBodyLimit::max(INCOMING_LIMIT)),
        )
        .route("/api/prompt", get(prompt_get).post(prompt_post))
        .route("/api/play", post(play_start))
        .route("/api/play/{id}", get(play_job).delete(play_stop))
        .route("/api/play/{id}/seek", post(play_seek))
        .route("/api/play/{id}/quality", post(play_quality))
        .route("/api/play/{id}/audio", post(play_audio))
        .route("/api/cast", get(cast::cast_get).put(cast::cast_put))
        .route("/api/cast/presence", post(cast::cast_presence))
        .route("/api/cast/receivers", get(cast::cast_receivers))
        .route(
            "/api/cast/session",
            get(cast::cast_session_get)
                .put(cast::cast_session_put)
                .delete(cast::cast_session_delete),
        )
        .route("/api/cast/session/place", post(cast::cast_session_place))
        .route(
            "/api/cast/session/command",
            post(cast::cast_session_command),
        )
        .route("/api/cast/devices", get(cast::cast_devices))
        .route("/api/cast/device", get(cast::cast_device_get))
        .route("/api/cast/devices/{id}/play", post(cast::cast_device_play))
        .route("/api/cast/devices/{id}/stop", post(cast::cast_device_stop))
        .route(
            "/api/cast/devices/{id}/pause",
            post(cast::cast_device_pause),
        )
        .route("/api/cast/devices/{id}/seek", post(cast::cast_device_seek))
        .route(
            "/api/cast/devices/{id}/volume",
            post(cast::cast_device_volume),
        )
        .route(
            "/api/cast/devices/{id}/subtitles",
            post(cast::cast_device_subtitles),
        )
        .route("/api/repos", get(repos_list).post(repos_install))
        .route("/api/repos/{id}", get(repo_detail).delete(repo_remove))
        .route(
            "/api/repos/{id}/addons/{addon_id}",
            post(repo_install_addon),
        )
        .route("/api/image", get(image))
        .route("/api/pair/me", get(pairing::me))
        .route("/api/pair/start", post(pairing::start))
        .route("/api/pair/status", get(pairing::status))
        .route("/api/pair/approve", post(pairing::approve))
        .route("/api/devices", get(pairing::devices))
        .route("/api/devices/{id}", delete(pairing::device_remove))
        .route("/media/{session}/{name}", get(media))
        .route("/vendor/hls.min.js", get(hls_script))
        .layer(axum::middleware::from_fn(note_server_error))
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let dist = paths::web_dist();
    if dist.join("index.html").is_file() {
        app = app.fallback_service(ServeDir::new(&dist).append_index_html_on_directories(true));
    } else {
        app = app.fallback(get(dashboard_missing));
    }
    let app = app
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            pairing::require,
        ))
        .layer(axum::middleware::from_fn(guard::same_site));

    let listener = tokio::net::TcpListener::bind(&bind).await?;
    // This process owns the port, so no other Portal is serving segments.
    // Play jobs do not survive a restart; earlier output is only disk use.
    clear_dir(&portal_dir.join("hls"));
    tracing::info!("Portal listening on http://{bind}");
    tracing::info!("Dashboard {}", dist.display());
    if let Some(url) = cast_url {
        tracing::info!("Cast url {url}");
    }

    update::spawn_automatic(state.clone());
    spawn_stray_watch(state.clone());
    spawn_stuck_close_watch(state.clone());

    let stop = state.clone();
    let mut asked = state.0.stopping.subscribe();
    let signal = async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = asked.wait_for(|stopping| *stopping) => {}
        }
        tracing::info!("Portal is shutting down");
        stop.0.stopping.send_replace(true);
    };
    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(signal)
    .into_future();
    // A player holding a stream open would keep graceful shutdown waiting.
    // Once the signal fires, open connections get a short grace period.
    let mut stopped = state.0.stopping.subscribe();
    tokio::select! {
        result = server => result?,
        _ = async {
            let _ = stopped.wait_for(|stopping| *stopping).await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => {}
    }

    // Only a Kodi this process started is quit; an attached one is left alone.
    let _ = tokio::time::timeout(Duration::from_secs(10), state.0.supervisor.quit()).await;
    Ok(())
}

fn clear_dir(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

const SHUTDOWN_GRACE: Duration = Duration::from_secs(4);

/// Identifies this service to the desktop launcher. The pid lets a stop wait
/// for the process to exit (it quits Kodi after the port closes).
async fn ping() -> Json<serde_json::Value> {
    Json(json!({
        "app": "portal",
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    }))
}

/// Loopback only, and only with the launcher's header: a custom header forces
/// a CORS preflight, which this service never approves, so a page on another
/// origin cannot send it.
fn launcher_only(peer: SocketAddr, headers: &HeaderMap) -> bool {
    peer.ip().is_loopback() && headers.contains_key("x-portal-launcher")
}

/// Stops the service. See `launcher_only`.
async fn shutdown(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Result<Json<OkBody>, ApiError> {
    if !launcher_only(peer, &headers) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Shutdown is only accepted from this PC.",
        ));
    }
    state.0.stopping.send_replace(true);
    Ok(Json(OkBody { ok: true }))
}

fn post_put_delete_watch() -> axum::routing::MethodRouter<AppState> {
    axum::routing::put(watch_put).delete(watch_delete)
}

fn post_put_delete_book() -> axum::routing::MethodRouter<AppState> {
    axum::routing::put(book_put).delete(book_delete)
}

fn lock_store(state: &AppState) -> std::sync::MutexGuard<'_, Store> {
    state
        .0
        .store
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// Runs a library change that may rewrite library.json off the async
/// threads, as progress does, so other requests are not held up behind
/// the disk.
async fn write_store<T: Send + 'static>(
    state: &AppState,
    write: impl FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
) -> Result<T, ApiError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || write(&mut lock_store(&state)))
        .await
        .map_err(|_| {
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The library was not saved.",
            )
        })?
        .map_err(from_store)
}

/// Reads the library off the async threads. Home and a title walk a file
/// that can be several megabytes, and a save of that file must not stall
/// the runtime while it waits for the disk.
async fn read_store<T: Send + 'static>(
    state: &AppState,
    read: impl FnOnce(&Store) -> T + Send + 'static,
) -> Result<T, ApiError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || read(&lock_store(&state)))
        .await
        .map_err(|_| {
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "The library could not be read.",
            )
        })
}

type ApiError = (StatusCode, Json<serde_json::Value>);

fn fail(code: StatusCode, detail: impl Into<String>) -> ApiError {
    (code, Json(json!({ "detail": detail.into() })))
}

fn from_engine(err: EngineError) -> ApiError {
    let code = match &err {
        EngineError::BadRequest(_) => StatusCode::BAD_REQUEST,
        EngineError::NotFound => StatusCode::NOT_FOUND,
        EngineError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::CONFLICT,
    };
    fail(code, err.public_message())
}

fn from_store(err: StoreError) -> ApiError {
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        portal_engine::public_error(&err.to_string()),
    )
}

async fn dashboard_missing() -> impl IntoResponse {
    (
        StatusCode::OK,
        "Portal's API is up. Build the dashboard with npm install and npm run build inside web/.",
    )
}

async fn status(State(state): State<AppState>) -> Json<Status> {
    Json(state.0.supervisor.status().await)
}

async fn engine_restart(State(state): State<AppState>) -> Result<Json<Status>, ApiError> {
    state
        .0
        .supervisor
        .restart()
        .await
        .map(Json)
        .map_err(from_engine)
}

async fn engine_quit(State(state): State<AppState>) -> Result<Json<Status>, ApiError> {
    state
        .0
        .supervisor
        .quit()
        .await
        .map(Json)
        .map_err(from_engine)
}

async fn engine_window(
    State(state): State<AppState>,
    Json(body): Json<WindowRequest>,
) -> Result<Json<Status>, ApiError> {
    state
        .0
        .supervisor
        .set_window(body.visible)
        .await
        .map(Json)
        .map_err(from_engine)
}

async fn get_source(State(state): State<AppState>) -> Json<Source> {
    Json(lock_store(&state).source())
}

async fn put_source(
    State(state): State<AppState>,
    Json(source): Json<Source>,
) -> Result<Json<OkBody>, ApiError> {
    if let Some(path) = &source.path {
        if !path.to_ascii_lowercase().starts_with("plugin://") {
            return Err(fail(StatusCode::BAD_REQUEST, "Choose a video add-on."));
        }
    }
    write_store(&state, move |store| store.set_source(source)).await?;
    Ok(Json(OkBody { ok: true }))
}

async fn home(State(state): State<AppState>) -> Result<Json<Home>, ApiError> {
    read_store(&state, |store| Home {
        continue_watching: store.home_continue(),
        watchlist: store.watchlist(),
        bookmarks: store.bookmarks(),
    })
    .await
    .map(Json)
}

async fn watchlist(State(state): State<AppState>) -> Result<Json<CardList>, ApiError> {
    read_store(&state, |store| CardList {
        items: store.watchlist(),
    })
    .await
    .map(Json)
}

async fn bookmarks(State(state): State<AppState>) -> Result<Json<CardList>, ApiError> {
    read_store(&state, |store| CardList {
        items: store.bookmarks(),
    })
    .await
    .map(Json)
}

async fn history(State(state): State<AppState>) -> Result<Json<HistoryList>, ApiError> {
    read_store(&state, |store| HistoryList {
        items: store.history(),
    })
    .await
    .map(Json)
}

async fn history_clear(State(state): State<AppState>) -> Result<Json<OkBody>, ApiError> {
    write_store(&state, |store| store.clear_history()).await?;
    Ok(Json(OkBody { ok: true }))
}

#[derive(Debug, Deserialize)]
struct HistoryName {
    #[serde(default)]
    name: String,
    #[serde(flatten)]
    title: HistoryTitle,
}

/// The title the page showed at a slot. Rows shift while something plays,
/// so the slot alone can point at a neighbour.
#[derive(Debug, Default, Deserialize)]
struct HistoryTitle {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    season: Option<u32>,
    #[serde(default)]
    episode: Option<u32>,
}

fn history_row(store: &Store, slot: usize, title: &HistoryTitle) -> Option<usize> {
    match title.id.as_deref() {
        Some(id) => store.history_slot(slot, id, title.season, title.episode),
        None => Some(slot),
    }
}

async fn history_put(
    State(state): State<AppState>,
    UrlPath(slot): UrlPath<usize>,
    Json(body): Json<HistoryName>,
) -> Result<Json<OkBody>, ApiError> {
    let name = body.name.trim();
    if name.is_empty() || name.chars().count() > 200 {
        return Err(fail(StatusCode::BAD_REQUEST, "Enter a name."));
    }
    let name = name.to_string();
    let saved = write_store(&state, move |store| {
        match history_row(store, slot, &body.title) {
            Some(row) => store.rename_history(row, &name),
            None => Ok(false),
        }
    })
    .await?;
    if !saved {
        return Err(fail(StatusCode::NOT_FOUND, "That history row is gone."));
    }
    Ok(Json(OkBody { ok: true }))
}

async fn history_delete(
    State(state): State<AppState>,
    UrlPath(slot): UrlPath<usize>,
    Query(title): Query<HistoryTitle>,
) -> Result<Json<OkBody>, ApiError> {
    let removed = write_store(&state, move |store| {
        match history_row(store, slot, &title) {
            Some(row) => store.delete_history(row),
            None => Ok(false),
        }
    })
    .await?;
    if !removed {
        return Err(fail(StatusCode::NOT_FOUND, "That history row is gone."));
    }
    Ok(Json(OkBody { ok: true }))
}

async fn title(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<TitleDetail>, ApiError> {
    read_store(&state, move |store| store.title(&id))
        .await?
        .map(Json)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "This title is not in Portal yet."))
}

async fn watch_put(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<ListPut>,
) -> Result<Json<OkBody>, ApiError> {
    save_list(&state, id, body, true).await
}

async fn book_put(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<ListPut>,
) -> Result<Json<OkBody>, ApiError> {
    save_list(&state, id, body, false).await
}

async fn save_list(
    state: &AppState,
    id: String,
    body: ListPut,
    watch: bool,
) -> Result<Json<OkBody>, ApiError> {
    if id.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Missing a title."));
    }
    let kind = body.kind.unwrap_or(TitleKind::Movie);
    // Prefer a real display name; never fall back to a plugin:// id as the card title.
    let name = list_put_name(body.name.as_deref())?.to_string();
    let facts = TitleFacts::new(body.year, body.imdb_id.as_deref());
    write_store(state, move |store| {
        if watch {
            store.put_watchlist_with(&id, kind, &name, body.art, facts)
        } else {
            store.put_bookmark_with(&id, kind, &name, body.art, facts)
        }
    })
    .await?;
    Ok(Json(OkBody { ok: true }))
}

/// `None` keeps the store fallback. An explicit empty/too-long name is rejected.
fn list_put_name(name: Option<&str>) -> Result<&str, ApiError> {
    match name {
        None => Ok(""),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.chars().count() > 200 {
                return Err(fail(StatusCode::BAD_REQUEST, "Enter a name."));
            }
            Ok(trimmed)
        }
    }
}

/// The most ids one `POST /api/saved` may ask about.
const SAVED_LIMIT: usize = 1000;

/// Watchlist and bookmark state for each id, saved or not.
async fn saved(
    State(state): State<AppState>,
    Json(query): Json<SavedQuery>,
) -> Result<Json<SavedMap>, ApiError> {
    if query.ids.len() > SAVED_LIMIT {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "Ask about fewer titles at once.",
        ));
    }
    let ids = query.ids;
    read_store(&state, move |store| saved_map(store, &ids))
        .await
        .map(Json)
}

fn saved_map(store: &Store, ids: &[String]) -> SavedMap {
    let mut map = SavedMap::default();
    for id in ids.iter().filter(|id| !id.trim().is_empty()) {
        map.items.insert(id.clone(), store.saved_state(id));
    }
    map
}

async fn watch_delete(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<OkBody>, ApiError> {
    write_store(&state, move |store| store.delete_watchlist(&id)).await?;
    Ok(Json(OkBody { ok: true }))
}

async fn book_delete(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<OkBody>, ApiError> {
    write_store(&state, move |store| store.delete_bookmark(&id)).await?;
    Ok(Json(OkBody { ok: true }))
}

async fn browse_path(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<PathQuery>,
) -> Result<Json<BrowsePage>, ApiError> {
    if query.path.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Missing a folder."));
    }
    if remote_request(peer, &headers) && !remote_may_open(&state, &query.path) {
        return Err(fail(StatusCode::FORBIDDEN, REMOTE_ADDON_ONLY));
    }
    if let Some(page) = cached_browse(&query.path) {
        return Ok(Json(observe_page(&state, page).await?));
    }
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    let page = browse(&state.0.supervisor.browse(), &query.path)
        .await
        .map_err(from_engine)?;
    let page = observe_page(&state, page).await?;
    store_browse(&query.path, &page);
    Ok(Json(page))
}

async fn search_path(
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> Result<Json<BrowsePage>, ApiError> {
    let root = {
        let store = lock_store(&state);
        store.source().path
    };
    let root = root.ok_or_else(|| fail(StatusCode::BAD_REQUEST, "Choose a video add-on first."))?;
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    let page = search(
        &state.0.supervisor.browse(),
        &state.0.supervisor.dialogs(),
        &state.0.supervisor.guisettings_path(),
        &root,
        &query.q,
    )
    .await
    .map_err(from_engine)?;
    Ok(Json(observe_page(&state, page).await?))
}

/// The add-on settings form a row opens: its own form through `openSettings`,
/// or another installed add-on named by the row's action or label.
fn settings_target(settings: &Settings, file: &str, label: &str) -> Option<String> {
    settings_addon(file, label)
        .or_else(|| settings_name_hint(file, label).and_then(|name| settings.addon_named(&name)))
}

/// Notes a listed page's titles in the library, which may save it.
async fn observe_page(state: &AppState, mut page: BrowsePage) -> Result<BrowsePage, ApiError> {
    for item in page.items.iter_mut() {
        if item.settings_addon.is_none() {
            item.settings_addon = settings_target(&state.0.settings, &item.path, &item.label);
        }
    }
    write_store(state, move |store| {
        store.observe(&page.path, &page.items).map(|()| page)
    })
    .await
}

async fn addons(State(state): State<AppState>) -> Result<Json<AddonList>, ApiError> {
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    // Use dialogs RPC, not browse: a long plugin GetDirectory holds browse_client.
    let mut list = video_addons(&state.0.supervisor.dialogs())
        .await
        .map_err(from_engine)?;
    for item in &mut list.items {
        item.removable = state.0.addons.video_removable(&item.id);
    }
    Ok(Json(list))
}

#[derive(Deserialize)]
struct ProgressBody {
    #[serde(flatten)]
    report: ProgressReport,
    /// Pause, stop, seek, and leaving the page ask to write the file now.
    /// A steady playhead omits it.
    #[serde(default)]
    flush: bool,
}

async fn progress(
    State(state): State<AppState>,
    Json(body): Json<ProgressBody>,
) -> Result<Json<OkBody>, ApiError> {
    let ProgressBody { report, flush } = body;
    if report.id.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Missing a title."));
    }
    // Saving rewrites the library file; it runs off the async threads so
    // other requests are not held up behind the disk.
    tokio::task::spawn_blocking(move || lock_store(&state).write_progress(&report, flush))
        .await
        .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Progress was not saved."))?
        .map_err(from_store)?;
    Ok(Json(OkBody { ok: true }))
}

async fn settings_form(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<SettingsDoc>, ApiError> {
    state.0.settings.form(&id).map(Json).map_err(from_engine)
}

async fn settings_write(
    State(state): State<AppState>,
    UrlPath((id, setting_id)): UrlPath<(String, String)>,
    Json(body): Json<SettingWrite>,
) -> Result<Json<OkBody>, ApiError> {
    state
        .0
        .settings
        .write_value(&id, &setting_id, &body.value)
        .map_err(from_engine)?;
    Ok(Json(OkBody { ok: true }))
}

async fn settings_action(
    State(state): State<AppState>,
    UrlPath((id, setting_id)): UrlPath<(String, String)>,
) -> Result<Json<ActionBody>, ApiError> {
    let builtin = state
        .0
        .settings
        .action_builtin(&id, &setting_id)
        .map_err(from_engine)?;
    if let Some(addon) = settings_target(&state.0.settings, &builtin, "") {
        return Ok(Json(ActionBody {
            ok: true,
            browse_path: None,
            settings_addon: Some(addon),
            ran: false,
        }));
    }
    let Some(target) = plugin_target(&builtin) else {
        return Ok(Json(ActionBody {
            ok: true,
            browse_path: None,
            settings_addon: None,
            ran: false,
        }));
    };
    if builtin.trim_start().starts_with("ActivateWindow(") {
        return Ok(Json(ActionBody {
            ok: true,
            browse_path: Some(target),
            settings_addon: None,
            ran: false,
        }));
    }
    // RunPlugin rows run the builtin the add-on stored. ExecuteAddon would open
    // a video plugin as a directory and cover its dialog.
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    note_run(&id);
    state
        .0
        .supervisor
        .run_settings_builtin(&builtin)
        .await
        .map_err(from_engine)?;
    Ok(Json(ActionBody {
        ok: true,
        browse_path: None,
        settings_addon: None,
        ran: true,
    }))
}

#[derive(Deserialize)]
struct RunBody {
    file: String,
}

/// Run an add-on row that is not a video, as Kodi does when one is picked:
/// `RunPlugin`. Playback stays on `/api/play`.
async fn run_plugin(
    State(state): State<AppState>,
    Json(body): Json<RunBody>,
) -> Result<Json<ActionBody>, ApiError> {
    let file = body.file.trim();
    if file.len() > 2048 || !plugin_link(file) || play_refusal(file, "").is_some() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "That item is an add-on action, not a video.",
        ));
    }
    let builtin = if file.to_ascii_lowercase().contains("runplugin(") {
        file.to_string()
    } else {
        run_plugin_builtin(file)
    };
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    // The confirm has to be created while Kodi is on screen. A dialog that
    // opens on a hidden player ignores the later Yes click.
    let _ = state.0.supervisor.set_window(true).await;
    if let Some(addon) = plugin_addon(&builtin) {
        note_run(&addon);
    }
    state
        .0
        .supervisor
        .run_settings_builtin(&builtin)
        .await
        .map_err(from_engine)?;
    Ok(Json(ActionBody {
        ok: true,
        browse_path: None,
        settings_addon: None,
        ran: true,
    }))
}

const INCOMING_LIMIT: usize = 32 * 1024 * 1024;

/// A file the viewer chose for an add-on dialog. The path is one Kodi can read.
/// The original name is not kept.
async fn incoming_file(
    mut multipart: axum::extract::Multipart,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut saved: Option<std::path::PathBuf> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| fail(StatusCode::BAD_REQUEST, "That file could not be read."))?
    {
        if field.name() != Some("file") {
            continue;
        }
        let ext = incoming_extension(field.file_name().unwrap_or(""));
        let bytes = field
            .bytes()
            .await
            .map_err(|_| fail(StatusCode::BAD_REQUEST, "That file could not be read."))?;
        if bytes.is_empty() || bytes.len() > INCOMING_LIMIT {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                "That file could not be read.",
            ));
        }
        if ext == "zip" && !is_zip(&bytes) {
            return Err(fail(StatusCode::BAD_REQUEST, "Choose a zip file."));
        }
        let dir = paths::portal_dir().join("incoming");
        std::fs::create_dir_all(&dir)
            .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "The file was not saved."))?;
        let path = dir.join(format!("addon-file.{ext}"));
        std::fs::write(&path, &bytes)
            .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "The file was not saved."))?;
        saved = Some(path);
    }
    let Some(path) = saved else {
        return Err(fail(StatusCode::BAD_REQUEST, "Choose a file."));
    };
    Ok(Json(serde_json::json!({
        "path": path.display().to_string(),
    })))
}

fn incoming_extension(name: &str) -> &'static str {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "zip" => "zip",
        "xml" => "xml",
        "json" => "json",
        "txt" => "txt",
        _ => "zip",
    }
}

fn is_zip(bytes: &[u8]) -> bool {
    bytes.len() >= 4
        && bytes[0] == b'P'
        && bytes[1] == b'K'
        && matches!(bytes[2], 0x03 | 0x05 | 0x07)
        && matches!(bytes[3], 0x04 | 0x06 | 0x08)
}

async fn prompt_get(State(state): State<AppState>) -> Result<Json<PromptBody>, ApiError> {
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    let prompt = state.0.prompts.read().await.map_err(from_engine)?;
    let prompt = prompt.map(|prompt| picking_view(&state, prompt));
    let settings_addon = state
        .0
        .prompts
        .take_opened_settings()
        .and_then(|heading| opened_settings_addon(&state.0.settings, &heading));
    Ok(Json(PromptBody {
        prompt,
        settings_addon,
    }))
}

/// The add-on a run or a settings button was started for, a moment ago.
static LAST_RUN: std::sync::Mutex<Option<(String, Instant)>> = std::sync::Mutex::new(None);
const LAST_RUN_KEPT: Duration = Duration::from_secs(60);

fn note_run(addon_id: &str) {
    *LAST_RUN.lock().unwrap_or_else(|err| err.into_inner()) =
        Some((addon_id.to_string(), Instant::now()));
}

/// The add-on id in a plugin link, bare or inside a builtin.
fn plugin_addon(link: &str) -> Option<String> {
    let at = link.to_ascii_lowercase().find("plugin://")?;
    let host: String = link[at + "plugin://".len()..]
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-'))
        .collect();
    (!host.is_empty()).then_some(host)
}

/// Which add-on a settings window belongs to. Kodi heads it with its word
/// for "Settings", a dash and the add-on's name, and a name can hold a dash
/// too, so every tail of the heading is tried. A heading that names no
/// installed add-on belongs to the add-on that was just run.
fn opened_settings_addon(settings: &Settings, heading: &str) -> Option<String> {
    let named = heading
        .match_indices(" - ")
        .find_map(|(at, dash)| settings.addon_named(&heading[at + dash.len()..]));
    named.or_else(|| {
        let (addon, at) = LAST_RUN
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()?;
        (at.elapsed() <= LAST_RUN_KEPT && settings.addon_installed(&addon)).then_some(addon)
    })
}

/// A source list Portal is answering itself shows as a step, not as a list
/// of a hundred rows that is gone a moment later.
fn picking_view(state: &AppState, prompt: Prompt) -> Prompt {
    if prompt.kind != PromptKind::Select {
        return prompt;
    }
    let caps: Vec<u32> = lock_picking(state).values().copied().collect();
    let Some(cap) = caps
        .into_iter()
        .find(|cap| pick_source(&prompt.options, *cap, &[], None).is_some())
    else {
        return prompt;
    };
    Prompt {
        id: prompt.id,
        kind: PromptKind::Progress,
        title: format!("Choosing a {cap}p copy"),
        message: String::new(),
        options: Vec::new(),
        cancellable: false,
        progress: None,
    }
}

fn lock_picking(state: &AppState) -> std::sync::MutexGuard<'_, HashMap<String, u32>> {
    state
        .0
        .picking
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn lock_packs(state: &AppState) -> std::sync::MutexGuard<'_, packs::SeasonPacks> {
    state.0.packs.lock().unwrap_or_else(|err| err.into_inner())
}

/// Copies Portal tries from one source list before the viewer is asked. A
/// list that comes back after a pick means that copy did not play.
const SOURCE_PICKS: usize = 4;
/// How long a play may take to show its source list and resolve.
const SOURCE_PICK_WINDOW: Duration = Duration::from_secs(240);
/// How long to wait after a row was chosen, so a slow resolve still counts.
const SOURCE_PACK_WAIT: Duration = Duration::from_secs(600);
/// How long a picked row's list may stay open before the pick counts as lost.
const SOURCE_PICK_RETRY: Duration = Duration::from_secs(10);

/// The prompt id without its showing number. Answering a list stamps a new
/// number while the same window is still open, so the number cannot say
/// whether the list went away.
fn prompt_showing_base(id: &str) -> &str {
    match id.rsplit_once('-') {
        Some((base, number))
            if !base.is_empty()
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            base
        }
        _ => id,
    }
}

/// True while the list just answered is still the one on screen. A second
/// Select then lands on the resolve window, which treats it as cancel.
fn hold_source_pick(
    answered_base: &str,
    prompt_id: &str,
    list_left: bool,
    since_answer: Duration,
) -> bool {
    !answered_base.is_empty()
        && !list_left
        && prompt_showing_base(prompt_id) == answered_base
        && since_answer < SOURCE_PICK_RETRY
}

/// A list that was not ready is asked again on the next look. Any other miss
/// means the window is gone or must not be touched, so the pick waits.
fn source_pick_waits(message: &str) -> bool {
    !message.contains("not ready")
}

struct SourceAttempts {
    tried: Vec<String>,
    /// The row answered while its list was still up. It counts as tried once
    /// that list goes away, which is when the copy did not play.
    pending: Option<String>,
    base: String,
    at: Instant,
    list_left: bool,
    warned: String,
    /// The list whose answers have been failing, and when that run started.
    /// A new list starts the clock again. A flicker to no prompt does not.
    failed_base: String,
    failed_at: Option<Instant>,
}

impl SourceAttempts {
    fn new() -> Self {
        Self {
            tried: Vec::new(),
            pending: None,
            base: String::new(),
            at: Instant::now(),
            list_left: true,
            warned: String::new(),
            failed_base: String::new(),
            failed_at: None,
        }
    }

    fn note_list_left(&mut self) {
        if self.base.is_empty() {
            return;
        }
        self.list_left = true;
        if let Some(row) = self.pending.take() {
            if !self.tried.contains(&row) {
                self.tried.push(row);
            }
        }
    }

    fn note_failure(&mut self, base: &str, at: Instant) {
        if self.failed_base != base {
            self.failed_base = base.to_string();
            self.failed_at = Some(at);
        }
    }

    fn failures_expired(&self, now: Instant) -> bool {
        self.failed_at.is_some_and(|at| {
            !self.failed_base.is_empty() && now.saturating_duration_since(at) >= SOURCE_PICK_RETRY
        })
    }

    fn clear_failure(&mut self) {
        self.failed_base.clear();
        self.failed_at = None;
    }
}

/// The show and season of an episode play, so a season file picked for it
/// is preferred for the next episode.
#[derive(Clone)]
struct SeasonWatch {
    show: String,
    season: u32,
}

fn season_watch(request: &PlayRequest) -> Option<SeasonWatch> {
    if request.kind != PlayKind::Episode {
        return None;
    }
    let season = request.season?;
    let show = request
        .show_id
        .clone()
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| request.id.clone());
    Some(SeasonWatch { show, season })
}

/// Answers the add-on's source list for a play with the copy that suits its
/// quality, so a 1080p play never pulls a 4K file. A list that names no
/// resolution, or one that keeps coming back, is left to the viewer.
///
/// For an episode, the task also stays until the play succeeds or fails, and
/// remembers a season file so the next episode of that season prefers it.
/// `cap` is absent when Portal is not choosing (the viewer is, or the add-on
/// resolves on its own). The season file is still remembered from the row that played.
fn spawn_source_pick(
    state: AppState,
    job_id: String,
    cap: Option<u32>,
    watch: Option<SeasonWatch>,
) {
    if let Some(height) = cap {
        lock_picking(&state).insert(job_id.clone(), height);
    }
    if let Some(item) = &watch {
        lock_packs(&state).begin(&job_id, &item.show, item.season);
    }
    tokio::spawn(async move {
        let started = tokio::time::Instant::now();
        // An episode keeps watching long enough for a manual pick to count.
        // A movie only needs the usual source-list window.
        let deadline = started
            + if watch.is_some() {
                SOURCE_PACK_WAIT
            } else {
                SOURCE_PICK_WINDOW
            };
        let mut choosing = cap.is_some();
        let mut attempts = SourceAttempts::new();
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            let Some(job) = state.0.player.job(&job_id) else {
                lock_packs(&state).settle(&job_id, false);
                break;
            };
            match job.state {
                JobState::Ready => {
                    lock_packs(&state).settle(&job_id, true);
                    break;
                }
                JobState::Failed | JobState::Browse => {
                    lock_packs(&state).settle(&job_id, false);
                    break;
                }
                JobState::Working | JobState::Prompt => {}
            }
            let now = tokio::time::Instant::now();
            if choosing && now >= started + SOURCE_PICK_WINDOW {
                if attempts.tried.is_empty() && attempts.pending.is_none() {
                    tracing::warn!(job = %job_id, "No source list to pick from in time");
                }
                choosing = false;
                lock_picking(&state).remove(&job_id);
            }
            if now >= deadline {
                lock_packs(&state).settle(&job_id, false);
                break;
            }
            if !choosing {
                continue;
            }
            let Some(cap) = cap else {
                choosing = false;
                continue;
            };
            let prompt = match state.0.prompts.read().await {
                Ok(prompt) => prompt,
                Err(_) => continue,
            };
            let Some(prompt) = prompt else {
                attempts.note_list_left();
                continue;
            };
            if prompt.kind != PromptKind::Select {
                attempts.note_list_left();
                continue;
            }
            // The same list still open means the last Select has not been
            // taken. Pressing another one cancels the resolve that follows.
            if hold_source_pick(
                &attempts.base,
                &prompt.id,
                attempts.list_left,
                attempts.at.elapsed(),
            ) {
                continue;
            }
            if attempts.tried.len() >= SOURCE_PICKS {
                tracing::warn!(job = %job_id, "No copy played after {SOURCE_PICKS} picks; left to the viewer");
                choosing = false;
                lock_picking(&state).remove(&job_id);
                continue;
            }
            let preferred = watch
                .as_ref()
                .and_then(|item| lock_packs(&state).preferred(&item.show, item.season));
            let Some(index) =
                pick_source(&prompt.options, cap, &attempts.tried, preferred.as_deref())
            else {
                // Rows read before the add-on fills them are only "Item 1",
                // "Item 2"..., so look again until they name a resolution.
                if attempts.tried.is_empty() {
                    continue;
                }
                choosing = false;
                lock_picking(&state).remove(&job_id);
                continue;
            };
            let row = prompt.options[index].clone();
            let answer = PromptAnswer {
                id: prompt.id.clone(),
                action: PromptAction::Submit,
                text: None,
                index: i32::try_from(index).ok(),
            };
            match state.0.prompts.answer(&answer).await {
                Ok(()) => {
                    if pack_identity(&row).is_some() {
                        tracing::info!(
                            "picked season file {} of {} for {cap}p",
                            index + 1,
                            prompt.options.len()
                        );
                    } else {
                        tracing::info!(
                            "picked source {} of {} for {cap}p",
                            index + 1,
                            prompt.options.len()
                        );
                    }
                    lock_packs(&state).note(&job_id, &row);
                    attempts.base = prompt_showing_base(&prompt.id).to_string();
                    attempts.at = Instant::now();
                    attempts.list_left = false;
                    attempts.pending = Some(row);
                    attempts.clear_failure();
                }
                Err(err) => {
                    let message = err.public_message();
                    let base = prompt_showing_base(&prompt.id).to_string();
                    attempts.note_failure(&base, Instant::now());
                    // The same list refusing the pick for the hold window is
                    // left on screen. Masking it until the play deadline hides
                    // a list the viewer could have answered.
                    if attempts.failures_expired(Instant::now()) {
                        tracing::warn!(job = %job_id, "source pick: {message}; left to the viewer");
                        choosing = false;
                        lock_picking(&state).remove(&job_id);
                        continue;
                    }
                    if attempts.warned != base {
                        tracing::warn!(job = %job_id, "source pick: {message}");
                        attempts.warned = base.clone();
                    }
                    if source_pick_waits(&message) {
                        attempts.base = base;
                        attempts.at = Instant::now();
                        attempts.list_left = false;
                        // A failed retry must not forget a row already chosen.
                    }
                }
            }
        }
        lock_picking(&state).remove(&job_id);
    });
}

async fn prompt_post(
    State(state): State<AppState>,
    Json(answer): Json<PromptAnswer>,
) -> Result<Json<OkBody>, ApiError> {
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    let chosen = chosen_row(&state, &answer).await;
    let showing = progress_cancel(&state, &answer).await;
    let reveal = if showing {
        state.0.supervisor.reveal_for_input()
    } else {
        None
    };
    let result = state
        .0
        .prompts
        .answer_with(&answer, &|| {
            if let Some(window) = reveal.as_ref() {
                window.assert_shown();
            }
        })
        .await;
    drop(reveal);
    result.map_err(from_engine)?;
    if let Some(row) = chosen {
        let live = |id: &str| {
            matches!(
                state.0.player.job(id).map(|job| job.state),
                Some(JobState::Working | JobState::Prompt)
            )
        };
        lock_packs(&state).note_latest(&row, live);
    }
    Ok(Json(OkBody { ok: true }))
}

/// The source row a Select names, read before the answer closes the list.
async fn chosen_row(state: &AppState, answer: &PromptAnswer) -> Option<String> {
    if answer.action != PromptAction::Submit {
        return None;
    }
    let index = usize::try_from(answer.index?).ok()?;
    let prompt = state.0.prompts.read().await.ok()??;
    if prompt.kind != PromptKind::Select || prompt.id != answer.id {
        return None;
    }
    prompt.options.get(index).cloned()
}

async fn progress_cancel(state: &AppState, answer: &PromptAnswer) -> bool {
    if answer.action != PromptAction::Cancel {
        return false;
    }
    matches!(
        state.0.prompts.read().await,
        Ok(Some(prompt)) if prompt.kind == PromptKind::Progress && prompt.id == answer.id
    )
}

async fn play_start(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<PlayRequest>,
) -> Result<Json<PlayAccepted>, ApiError> {
    if remote_request(peer, &headers) {
        // The show is browsed for the next episode, so it gets the same check.
        let show = request.show_id.as_deref().unwrap_or(&request.id);
        let show_ok = !show.trim().to_ascii_lowercase().starts_with("plugin://")
            || remote_may_open(&state, show);
        if !remote_may_open(&state, &request.file) || !show_ok {
            return Err(fail(StatusCode::FORBIDDEN, REMOTE_ADDON_ONLY));
        }
    }
    let id = start_play_job(&state, request).await?;
    Ok(Json(PlayAccepted { id }))
}

/// Starts a play job, with the next episode it leads to. Also used when a
/// device cast moves on to the next episode by itself.
async fn start_play_job(state: &AppState, mut request: PlayRequest) -> Result<String, ApiError> {
    if request.file.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Missing a file."));
    }
    if request.id.trim().is_empty() {
        return Err(fail(StatusCode::BAD_REQUEST, "Missing a title id."));
    }
    if let Some(detail) = play_refusal(&request.file, &request.title) {
        return Err(fail(StatusCode::BAD_REQUEST, detail));
    }
    if settings_target(&state.0.settings, &request.file, &request.title).is_some() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "That item opens add-on settings.",
        ));
    }
    // A file saved from one add-on still plays after the source is switched
    // unless the library's copy on the chosen add-on is used instead.
    // Finding that copy walks the library, so it stays off the async threads.
    let state_for_route = state.clone();
    let routed = tokio::task::spawn_blocking(move || {
        let store = lock_store(&state_for_route);
        match store.route_play(&request) {
            PlayRoute::Keep => {}
            PlayRoute::Retarget { id, file, show_id } => {
                tracing::info!("Playing the copy on the chosen add-on");
                request.file = file;
                request.id = id;
                if show_id.is_some() {
                    request.show_id = show_id;
                }
            }
            PlayRoute::Missing { addon } => return Err(addon),
        }
        let show = (request.kind == PlayKind::Episode).then(|| {
            request
                .show_id
                .clone()
                .unwrap_or_else(|| request.id.clone())
        });
        let next = show.as_ref().and_then(|show_id| {
            store.next_after(TitleKind::Tv, show_id, request.season, request.episode)
        });
        // A season's last episode: the next season's episodes are learned once
        // this one plays, so Next and autoplay can carry on into it.
        let finale = match (&show, next.is_none(), request.season, request.episode) {
            (Some(show), true, Some(season), Some(episode)) if episode > 0 => {
                Some((show.clone(), season, episode))
            }
            _ => None,
        };
        Ok((request, next, finale))
    })
    .await
    .map_err(|_| {
        fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "The library could not be read.",
        )
    })?
    .map_err(|addon| {
        fail(
            StatusCode::BAD_REQUEST,
            format!("Search for this in {addon}, then play it from there."),
        )
    })?;
    let (mut request, next, finale) = routed;
    let sent = request
        .audio_language
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if sent.is_none() {
        request.audio_language = prefs::current(state).audio_language;
    }
    state.0.supervisor.ensure().await.map_err(from_engine)?;
    let pick = (!request.manual_links && picks_source(request.quality))
        .then(|| source_cap(request.quality));
    let watch = season_watch(&request);
    let id = state
        .0
        .player
        .start(request, state.0.supervisor.player_rpc(), next)
        .map_err(from_engine)?;
    if pick.is_some() || watch.is_some() {
        spawn_source_pick(state.clone(), id.clone(), pick, watch);
    }
    if let Some((show, season, episode)) = finale {
        spawn_next_season(state.clone(), id.clone(), show, season, episode);
    }
    Ok(id)
}

/// How long a finale's job may take to start before the next season is not looked up.
const NEXT_SEASON_WAIT: Duration = Duration::from_secs(300);

/// Lists the season after `season` once the job plays, and gives the job
/// that season's first episode. Waits for the stream first, so the listing
/// never competes with the add-on resolving this episode.
fn spawn_next_season(state: AppState, job_id: String, show: String, season: u32, episode: u32) {
    tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + NEXT_SEASON_WAIT;
        loop {
            match state.0.player.job(&job_id).map(|job| job.state) {
                Some(JobState::Ready) => break,
                Some(JobState::Working | JobState::Prompt)
                    if tokio::time::Instant::now() < deadline => {}
                _ => return,
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        let mut folder = lock_store(&state).season_folder_after(&show, season);
        if folder.is_none() && show.starts_with("plugin://") {
            // Played from Continue or search: the show's season list was never opened.
            if let Err(err) = list_folder(&state, &show).await {
                tracing::warn!(job = %job_id, error = %err.public_message(), "Could not list the show's seasons");
                return;
            }
            folder = lock_store(&state).season_folder_after(&show, season);
        }
        let Some(folder) = folder else {
            return;
        };
        if let Err(err) = list_folder(&state, &folder).await {
            tracing::warn!(job = %job_id, error = %err.public_message(), "Could not list the next season");
            return;
        }
        let next = lock_store(&state).next_after(TitleKind::Tv, &show, Some(season), Some(episode));
        if let Some(next) = next {
            let code = format!(
                "S{:02}E{:02}",
                next.season.unwrap_or(0),
                next.episode.unwrap_or(0)
            );
            if state.0.player.set_next(&job_id, next) {
                tracing::info!(job = %job_id, next = %code, "Found the next season");
            }
        }
    });
}

/// Lists an add-on folder as Browse would, so the library learns what it holds.
async fn list_folder(state: &AppState, path: &str) -> Result<(), EngineError> {
    if let Some(page) = cached_browse(path) {
        let _ = observe_page(state, page).await;
        return Ok(());
    }
    state.0.supervisor.ensure().await?;
    let page = browse(&state.0.supervisor.browse(), path).await?;
    if let Ok(page) = observe_page(state, page).await {
        store_browse(path, &page);
    }
    Ok(())
}

async fn play_job(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<PlayJob>, ApiError> {
    let mut job = state
        .0
        .player
        .job(&id)
        .ok_or_else(|| fail(StatusCode::NOT_FOUND, "That item is gone."))?;
    if job.state == JobState::Working {
        if let Ok(found) = state.0.prompts.read().await {
            job.prompt_known = true;
            if let Some(prompt) = found {
                // A source list Portal answers itself is still working.
                let view = picking_view(&state, prompt);
                if view.kind != PromptKind::Progress {
                    job.state = JobState::Prompt;
                }
                job.prompt = Some(view);
            }
        }
    }
    Ok(Json(job))
}

async fn play_stop(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<OkBody>, ApiError> {
    state.0.player.stop(&id).await.map_err(from_engine)?;
    Ok(Json(OkBody { ok: true }))
}

#[derive(Deserialize)]
struct SeekBody {
    seconds: f64,
}

async fn play_seek(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<SeekBody>,
) -> Result<Json<PlayJob>, ApiError> {
    let job = state
        .0
        .player
        .seek(&id, body.seconds)
        .await
        .map_err(from_engine)?;
    Ok(Json(job))
}

#[derive(Deserialize)]
struct QualityBody {
    quality: portal_api::Quality,
    seconds: f64,
}

async fn play_quality(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<QualityBody>,
) -> Result<Json<PlayJob>, ApiError> {
    let job = state
        .0
        .player
        .set_quality(&id, body.quality, body.seconds)
        .await
        .map_err(from_engine)?;
    Ok(Json(job))
}

#[derive(Deserialize)]
struct AudioBody {
    index: u32,
    seconds: f64,
}

async fn play_audio(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
    Json(body): Json<AudioBody>,
) -> Result<Json<PlayJob>, ApiError> {
    let job = state
        .0
        .player
        .set_audio(&id, body.index, body.seconds)
        .await
        .map_err(from_engine)?;
    Ok(Json(job))
}

#[derive(Deserialize)]
struct ZipInstall {
    url: String,
    #[serde(default)]
    name: Option<String>,
}

async fn repos_list(State(state): State<AppState>) -> Result<Json<RepoList>, ApiError> {
    state
        .0
        .addons
        .installed_repos()
        .map(Json)
        .map_err(from_engine)
}

async fn repo_detail(
    State(state): State<AppState>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<RepoDetail>, ApiError> {
    state
        .0
        .addons
        .repo_videos(&id)
        .await
        .map(Json)
        .map_err(from_engine)
}

/// How often Portal checks that its hidden Kodi isn't playing on its own.
const STRAY_EVERY: Duration = Duration::from_secs(3);

/// Stops playback the hidden Kodi started by itself. An add-on can go on
/// searching after Portal gave up on a play (closed, replaced, timed out) and
/// then start the stream inside Kodi: heard, not seen, and only a Kodi restart
/// stopped it. Only a Kodi that Portal started and keeps hidden is touched,
/// never one someone opened to use directly.
fn spawn_stray_watch(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(STRAY_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if *state.0.stopping.borrow() {
                return;
            }
            let status = state.0.supervisor.status().await;
            if status.state != portal_api::EngineState::Running
                || !status.started_by_us
                || status.window_visible
            {
                continue;
            }
            let client = state.0.supervisor.player_rpc();
            if state.0.player.stop_stray_playback(&client).await {
                tracing::info!("stopped playback Kodi started on its own");
            }
        }
    });
}

/// How often Portal checks that its hidden Kodi isn't stuck closing a dialog.
const STUCK_CLOSE_EVERY: Duration = Duration::from_secs(5);
/// Reveals for one stuck close before Portal stops putting Kodi on screen.
const STUCK_CLOSE_TRIES: u8 = 3;

/// A modal dialog that starts closing while Kodi is hidden never finishes:
/// the close animation only runs on screen, and until it ends Kodi ignores
/// every action, Portal's and a controller's alike. Kodi stayed that way for a
/// day once. Seen on two checks in a row, Kodi is shown without focus until
/// the dialog is gone.
fn spawn_stuck_close_watch(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(STUCK_CLOSE_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut seen = false;
        let mut tries = 0u8;
        loop {
            tick.tick().await;
            if *state.0.stopping.borrow() {
                return;
            }
            let status = state.0.supervisor.status().await;
            let stuck = status.state == portal_api::EngineState::Running
                && !status.window_visible
                && state.0.prompts.modal_close_stuck().await;
            if !stuck {
                seen = false;
                tries = 0;
                continue;
            }
            if !seen {
                seen = true;
                continue;
            }
            if tries >= STUCK_CLOSE_TRIES {
                continue;
            }
            tries += 1;
            if state.0.prompts.finish_modal_close().await {
                tracing::info!("finished a dialog close hidden Kodi was stuck on");
                seen = false;
                tries = 0;
            }
        }
    });
}

const REMOTE_ADDON_ONLY: &str =
    "Other devices can only open what the add-ons list. Files on this PC open only here.";

/// A request from another device, or one a proxy on this PC passed along.
fn remote_request(peer: SocketAddr, headers: &HeaderMap) -> bool {
    !from_this_pc(peer) || pairing::forwarded(headers)
}

/// What another device may browse or play: an installed add-on's own
/// folders and items, never a disk, a share, a web address, or an add-on's
/// settings form.
fn remote_may_open(state: &AppState, path: &str) -> bool {
    let path = path.trim();
    let Some(rest) = path
        .get(..9)
        .filter(|scheme| scheme.eq_ignore_ascii_case("plugin://"))
        .map(|_| &path[9..])
    else {
        return false;
    };
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let installed = !host.is_empty()
        && state.0.settings.addon_installed(host);
    installed && settings_target(&state.0.settings, path, "").is_none()
}

/// The caller is this PC: loopback, or one of its own addresses (a dashboard
/// opened by the PC's LAN address). Only a local address can be bound.
pub(crate) fn from_this_pc(peer: SocketAddr) -> bool {
    // An IPv4 address can arrive written as IPv6 (`::ffff:127.0.0.1`).
    let ip = peer.ip().to_canonical();
    ip.is_loopback() || (!ip.is_unspecified() && std::net::UdpSocket::bind((ip, 0)).is_ok())
}

/// An add-on runs code as the user, so a zip from any address is only
/// installed when asked from this PC, not from another device on the network.
async fn repos_install(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<ZipInstall>,
) -> Result<Json<InstallResult>, ApiError> {
    if !from_this_pc(peer) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Repositories can only be added on the PC running Portal.",
        ));
    }
    let outcome = state
        .0
        .addons
        .install_zip(&body.url, body.name.as_deref())
        .await
        .map_err(from_engine)?;
    Ok(Json(finish_install(&state, outcome).await))
}

async fn repo_remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<RemoveResult>, ApiError> {
    remove_installed(&state, peer, &id, true).await.map(Json)
}

async fn addon_remove(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath(id): UrlPath<String>,
) -> Result<Json<RemoveResult>, ApiError> {
    remove_installed(&state, peer, &id, false).await.map(Json)
}

async fn remove_installed(
    state: &AppState,
    peer: SocketAddr,
    id: &str,
    repository: bool,
) -> Result<RemoveResult, ApiError> {
    if !from_this_pc(peer) {
        return Err(fail(
            StatusCode::FORBIDDEN,
            "Add-ons can only be removed on the PC running Portal.",
        ));
    }
    if state.0.player.resolving() {
        return Err(fail(
            StatusCode::CONFLICT,
            "Stop playback before removing this add-on.",
        ));
    }
    if state.0.supervisor.browse().connected().await {
        let _ = state
            .0
            .supervisor
            .browse()
            .call(
                "Addons.SetAddonEnabled",
                json!({ "addonid": id, "enabled": false }),
                Duration::from_secs(10),
            )
            .await;
    }
    let outcome = match if repository {
        state.0.addons.remove_repository(id)
    } else {
        state.0.addons.remove_video(id)
    } {
        Ok(outcome) => outcome,
        Err(err) if removal_busy(&err) => {
            if state.0.supervisor.quit().await.is_err() {
                return Err(from_engine(err));
            }
            if repository {
                state.0.addons.remove_repository(id)
            } else {
                state.0.addons.remove_video(id)
            }
            .map_err(from_engine)?
        }
        Err(err) => return Err(from_engine(err)),
    };
    if !repository {
        let id = outcome.id.clone();
        let _ = write_store(state, move |store| {
            let current = store.source();
            if source_uses(&current, &id) {
                store.set_source(Source {
                    addon_id: None,
                    name: None,
                    path: None,
                })?;
            }
            Ok(())
        })
        .await;
    }
    Ok(finish_remove(state, outcome).await)
}

fn source_uses(source: &Source, id: &str) -> bool {
    if source.addon_id.as_deref() == Some(id) {
        return true;
    }
    let Some(path) = source.path.as_deref() else {
        return false;
    };
    let path = path.to_ascii_lowercase();
    let id = id.to_ascii_lowercase();
    path == format!("plugin://{id}") || path.starts_with(&format!("plugin://{id}/"))
}

fn removal_busy(err: &EngineError) -> bool {
    matches!(err, EngineError::Message(text) if text.contains("in use"))
}

async fn finish_remove(state: &AppState, outcome: RemoveOutcome) -> RemoveResult {
    let applies = match state.0.supervisor.restart().await {
        Ok(_) => "now",
        Err(_) => "next-start",
    };
    tracing::info!(
        "{}",
        if applies == "now" {
            "Add-on removed"
        } else {
            "Add-on is removed on the next start"
        }
    );
    RemoveResult {
        id: outcome.id,
        name: outcome.name,
        kind: outcome.kind,
        applies: Some(applies.to_string()),
    }
}

async fn repo_install_addon(
    State(state): State<AppState>,
    UrlPath((id, addon_id)): UrlPath<(String, String)>,
) -> Result<Json<InstallResult>, ApiError> {
    let outcome = state
        .0
        .addons
        .install_from_repo(&id, &addon_id)
        .await
        .map_err(from_engine)?;
    Ok(Json(finish_install(&state, outcome).await))
}

async fn finish_install(state: &AppState, outcome: portal_engine::InstallOutcome) -> InstallResult {
    let applies = if outcome.wrote {
        let applies = if state.0.player.resolving() {
            "next-start"
        } else {
            match state.0.supervisor.restart().await {
                Ok(_) => "now",
                Err(_) => "next-start",
            }
        };
        tracing::info!(
            "{}",
            if applies == "now" {
                "Add-on installed"
            } else {
                "Add-on installs on the next start"
            }
        );
        Some(applies.to_string())
    } else {
        None
    };
    // Disk installs land disabled until Kodi enables them; enable after restart
    // (or immediately when the package was already present) so /api/addons lists it.
    let ready = matches!(applies.as_deref(), Some("now")) || !outcome.wrote;
    if ready {
        enable_installed_addon(state, &outcome.id).await;
    }
    InstallResult {
        id: outcome.id,
        name: outcome.name,
        kind: outcome.kind,
        wrote: outcome.wrote,
        applies,
    }
}

async fn enable_installed_addon(state: &AppState, addon_id: &str) {
    match state
        .0
        .supervisor
        .browse()
        .call(
            "Addons.SetAddonEnabled",
            json!({ "addonid": addon_id, "enabled": true }),
            Duration::from_secs(10),
        )
        .await
    {
        Ok(_) => tracing::info!(addon_id, "Add-on enabled"),
        Err(err) => tracing::warn!(addon_id, error = %err, "Could not enable add-on"),
    }
}

async fn image(
    State(state): State<AppState>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, ApiError> {
    let (bytes, content_type) = state
        .0
        .supervisor
        .image(&query.path)
        .await
        .map_err(from_engine)?;
    Ok(image_response(bytes, &content_type))
}

async fn media(
    State(state): State<AppState>,
    UrlPath((session, name)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    match load_media(&state, &session, &name, &headers).await {
        Ok(response) => response,
        Err(err) => cors_error(err),
    }
}

async fn load_media(
    state: &AppState,
    session: &str,
    name: &str,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(response) = whole_title_file(state, session, name).await? {
        return Ok(response);
    }
    let path = state
        .0
        .player
        .session_file(session, name)
        .map_err(from_engine)?;
    let mut file = open_media_file(state, session, name, &path).await?;
    let len = file
        .metadata()
        .await
        .map_err(|_| fail(StatusCode::NOT_FOUND, "That segment is gone."))?
        .len();
    let range = headers
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());
    let span = media_span(range, len);
    let body = match span {
        MediaSpan::Full { len } => open_body(file, len),
        MediaSpan::Partial { start, end, .. } => {
            file.seek(SeekFrom::Start(start)).await.map_err(|_| {
                fail(
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    "That range is not valid.",
                )
            })?;
            open_body(file, end - start + 1)
        }
        MediaSpan::Unsatisfiable { .. } => Body::empty(),
    };
    Ok(media_response(media_type(name), span, body))
}

/// A file of a whole-title stream that is made when it is asked for: the
/// start file, a piece, or a subtitle on the title's own clock. None for
/// any other file.
async fn whole_title_file(
    state: &AppState,
    session: &str,
    name: &str,
) -> Result<Option<Response>, ApiError> {
    let made = name == portal_engine::CAST_INIT || name.ends_with(".m4s");
    let subtitle = name
        .strip_prefix("sub")
        .and_then(|rest| rest.strip_suffix(".vtt"))
        .and_then(|digits| digits.parse::<u32>().ok());
    if !(made || subtitle.is_some()) || !state.0.player.is_cast_session(session) {
        return Ok(None);
    }
    let bytes = match subtitle {
        Some(id) => state
            .0
            .player
            .cast_subtitle(session, id)
            .map(String::into_bytes)
            .ok_or_else(|| fail(StatusCode::NOT_FOUND, "That subtitle is gone."))?,
        None => state
            .0
            .player
            .cast_file(session, name)
            .await
            .map_err(from_engine)?,
    };
    let span = MediaSpan::Full {
        len: bytes.len() as u64,
    };
    Ok(Some(media_response(
        media_type(name),
        span,
        Body::from(bytes),
    )))
}

/// A growing playlist names a segment, then ffmpeg renames it into place.
/// Wait briefly for that file while the encode is still running.
const SEGMENT_WAIT: Duration = Duration::from_millis(2500);
const SEGMENT_POLL: Duration = Duration::from_millis(80);

async fn open_media_file(
    state: &AppState,
    session: &str,
    name: &str,
    path: &std::path::Path,
) -> Result<tokio::fs::File, ApiError> {
    let deadline = Instant::now() + SEGMENT_WAIT;
    loop {
        if let Ok(file) = tokio::fs::File::open(path).await {
            return Ok(file);
        }
        let still_writing = name.ends_with(".ts") && state.0.player.session_producing(session);
        if !still_writing || Instant::now() >= deadline {
            return Err(fail(StatusCode::NOT_FOUND, "That segment is gone."));
        }
        tokio::time::sleep(SEGMENT_POLL).await;
    }
}

fn open_body(file: tokio::fs::File, len: u64) -> Body {
    if len == 0 {
        return Body::empty();
    }
    // Seek already happened. Stream this span instead of reading the whole file.
    Body::new(tower_http::services::fs::AsyncReadBody::with_capacity(
        file.take(len),
        64 * 1024,
    ))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaSpan {
    Full { len: u64 },
    Partial { start: u64, end: u64, total: u64 },
    Unsatisfiable { total: u64 },
}

fn media_span(range_header: Option<&str>, len: u64) -> MediaSpan {
    let Some(header) = range_header
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return MediaSpan::Full { len };
    };
    match parse_byte_range(header, len) {
        ByteRange::Absent => MediaSpan::Full { len },
        ByteRange::Span { start, end } => MediaSpan::Partial {
            start,
            end,
            total: len,
        },
        ByteRange::Unsatisfiable => MediaSpan::Unsatisfiable { total: len },
    }
}

enum ByteRange {
    Absent,
    Span { start: u64, end: u64 },
    Unsatisfiable,
}

/// One byte range. A unit we do not understand is ignored. Extra ranges are dropped.
fn parse_byte_range(header: &str, len: u64) -> ByteRange {
    let header = header.trim();
    let Some(spec) = header
        .get(..6)
        .filter(|unit| unit.eq_ignore_ascii_case("bytes="))
        .map(|_| &header[6..])
    else {
        return ByteRange::Absent;
    };
    let spec = spec.split(',').next().unwrap_or(spec).trim();
    if spec.is_empty() || len == 0 {
        return ByteRange::Unsatisfiable;
    }
    let Some((start_text, end_text)) = spec.split_once('-') else {
        return ByteRange::Unsatisfiable;
    };
    let start_text = start_text.trim();
    let end_text = end_text.trim();
    if start_text.is_empty() {
        let Ok(suffix) = end_text.parse::<u64>() else {
            return ByteRange::Unsatisfiable;
        };
        if suffix == 0 {
            return ByteRange::Unsatisfiable;
        }
        let take = suffix.min(len);
        return ByteRange::Span {
            start: len - take,
            end: len - 1,
        };
    }
    let Ok(start) = start_text.parse::<u64>() else {
        return ByteRange::Unsatisfiable;
    };
    if start >= len {
        return ByteRange::Unsatisfiable;
    }
    let end = if end_text.is_empty() {
        len - 1
    } else {
        let Ok(end) = end_text.parse::<u64>() else {
            return ByteRange::Unsatisfiable;
        };
        end.min(len - 1)
    };
    if end < start {
        return ByteRange::Unsatisfiable;
    }
    ByteRange::Span { start, end }
}

fn media_response(content_type: &str, span: MediaSpan, body: Body) -> Response {
    let mut builder = Response::builder()
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Accept-Ranges, Content-Length, Content-Range, Content-Type",
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    builder = match span {
        MediaSpan::Full { len } => builder
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, len.to_string()),
        MediaSpan::Partial { start, end, total } => builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, content_type)
            .header(header::CONTENT_LENGTH, (end - start + 1).to_string())
            .header(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            ),
        MediaSpan::Unsatisfiable { total } => builder
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}")),
    };
    builder
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn cors_error(err: ApiError) -> Response {
    let mut response = err.into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    response
}

async fn hls_script() -> Result<Response, ApiError> {
    let path = paths::vendor_hls();
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| fail(StatusCode::NOT_FOUND, "The player script is not installed."))?;
    Ok(bytes_response(bytes, "text/javascript; charset=utf-8"))
}

fn bytes_response(bytes: Vec<u8>, content_type: &str) -> Response {
    cached_body(bytes, content_type, "no-store")
}

fn image_response(bytes: Vec<u8>, content_type: &str) -> Response {
    // Only pictures go back out, and the browser must not guess another type.
    if !content_type.to_ascii_lowercase().starts_with("image/") {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let mut response = cached_body(bytes, content_type, "private, max-age=3600");
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        axum::http::HeaderValue::from_static("nosniff"),
    );
    response
}

fn cached_body(bytes: Vec<u8>, content_type: &str, cache_control: &str) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, cache_control)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn media_type(name: &str) -> &'static str {
    if name.ends_with(".m3u8") {
        "application/vnd.apple.mpegurl"
    } else if name.ends_with(".vtt") {
        "text/vtt; charset=utf-8"
    } else if name.ends_with(".mp4") || name.ends_with(".m4v") || name.ends_with(".m4s") {
        "video/mp4"
    } else {
        "video/mp2t"
    }
}

/// Pull a plugin:// URL out of RunPlugin(...) or ActivateWindow(...).
fn plugin_target(builtin: &str) -> Option<String> {
    let idx = builtin.to_ascii_lowercase().find("plugin://")?;
    let rest = &builtin[idx..];
    let end = rest.find(['"', '\'', ')']).unwrap_or(rest.len());
    let url = rest[..end].trim().trim_end_matches(',').to_string();
    if url.len() > "plugin://".len() {
        Some(url)
    } else {
        None
    }
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/main.rs"]
mod tests;
