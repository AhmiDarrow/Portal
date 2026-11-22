//! Portal's HTTP API, typed with `portal-api`: one function per route, as `web/src/api.ts`
//! sends it, each returning a [`Call`] that [`Api::run`] sends (or [`Api::resource`] keeps
//! fetched for a key).
//!
//! ```ignore
//! ctx.api.run(api::home(), &Cancel::new(), move |out| match out {
//!     Ok(home) => shelves.set(home),
//!     Err(e) => notice.set(e.detail),
//! });
//! let page = ctx.api.resource(move || path.get(), |p| api::browse(p));
//! ```
//!
//! Requests go to the page's own origin with its cookies (`Http::new("")`), so the server's
//! guard and pairing see what they see from the old page. A failed request is an
//! [`ApiError`] whose `detail` is the server's own sentence; a 401 with `{"pair": true}`
//! also sets [`Api::pair_needed`], which shows the pairing screen.

use std::marker::PhantomData;

use portal_api::*;
use remedy_ui::net::{Http, NetError, Request};
use remedy_ui::reactive::{waker, Cancel, Resource, Signal, Waker};
use remedy_ui::App;
use serde::de::DeserializeOwned;

/// One route with its answer type.
#[derive(Clone, Debug)]
pub struct Call<T> {
    req: Request,
    _answer: PhantomData<fn() -> T>,
}

impl<T> Call<T> {
    fn new(req: Request) -> Self {
        Call { req, _answer: PhantomData }
    }

    /// Lets the request finish while the page closes (the last progress save, a stop).
    pub fn keepalive(mut self) -> Self {
        self.req = self.req.keepalive();
        self
    }

    /// Waits as long as the server takes, as the old page's `fetch` does: starting Kodi or
    /// resolving a stream can take minutes.
    pub fn no_timeout(mut self) -> Self {
        self.req = self.req.timeout(None);
        self
    }

    /// The request as it will be sent.
    pub fn request(&self) -> &Request {
        &self.req
    }
}

/// Why a call gave no answer, in words the page can show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiError {
    /// The HTTP status, when the server answered.
    pub status: Option<u16>,
    /// The server's `detail`, or a sentence for what went wrong.
    pub detail: String,
    /// The 401 that asks this device to pair (`{"pair": true}`).
    pub pair: bool,
}

impl From<NetError> for ApiError {
    fn from(e: NetError) -> Self {
        match e {
            NetError::Http(h) => ApiError::answered(h.status, &h.body),
            other => ApiError { status: None, detail: other.to_string(), pair: false },
        }
    }
}

impl ApiError {
    /// An error answer: its `detail`, else "Request failed (status)", as api.ts says it.
    pub fn answered(status: u16, body: &str) -> ApiError {
        let body = serde_json::from_str::<ErrorBody>(body).ok();
        let detail = body
            .as_ref()
            .map(|b| b.detail.trim())
            .filter(|d| !d.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Request failed ({status})"));
        // This device is not paired (or was removed): show the pairing number.
        let pair = status == 401 && body.and_then(|b| b.pair) == Some(true);
        ApiError { status: Some(status), detail, pair }
    }
}

/// What a page says when a call fails (main.ts `detail`): "Portal is not answering." when
/// nothing answered, else the server's sentence.
pub fn shown_detail(e: &ApiError) -> String {
    if e.status.is_none() {
        return "Portal is not answering.".into();
    }
    match e.detail.trim() {
        "" => "Portal could not load this page.".into(),
        d => d.to_owned(),
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

/// The client every screen shares (in [`crate::Ctx`]).
#[derive(Clone)]
pub struct Api {
    http: Http,
    app: App,
    waker: Option<Waker>,
    pair: Signal<bool>,
}

impl Api {
    /// A same-origin client. Made on the UI thread.
    pub fn new(app: &App) -> Api {
        Api { http: Http::new(""), app: app.clone(), waker: waker().ok(), pair: Signal::new(false) }
    }

    /// True once a call was refused because this device is not paired.
    pub fn pair_needed(&self) -> Signal<bool> {
        self.pair
    }

    /// Sends `call`; `done` runs on the UI thread with the answer.
    pub fn run<T: DeserializeOwned + Send + 'static>(
        &self,
        call: Call<T>,
        cancel: &Cancel,
        done: impl FnOnce(Result<T, ApiError>) + 'static,
    ) {
        let pair = self.pair;
        let c = self.app.completion(move |out: Result<T, ApiError>| {
            if out.as_ref().is_err_and(|e| e.pair) {
                pair.set(true);
            }
            done(out);
        });
        self.http.send_json::<T>(call.req, cancel, move |out| c.complete(out.map_err(ApiError::from)));
    }

    /// Sends `call` and hands back the body as text (a subtitle file).
    pub fn run_text(&self, call: Call<String>, cancel: &Cancel, done: impl FnOnce(Result<String, ApiError>) + 'static) {
        let pair = self.pair;
        let c = self.app.completion(move |out: Result<String, ApiError>| {
            if out.as_ref().is_err_and(|e| e.pair) {
                pair.set(true);
            }
            done(out);
        });
        self.http.send(call.req, cancel, move |out| c.complete(out.map(|r| r.text()).map_err(ApiError::from)));
    }

    /// The answer to `call(key)`, fetched again whenever the key `source` returns changes
    /// (the call for the old key is cancelled).
    #[track_caller]
    pub fn resource<K, T>(
        &self,
        source: impl Fn() -> K + 'static,
        call: impl Fn(&K) -> Call<T> + 'static,
    ) -> Resource<T, ApiError>
    where
        K: PartialEq + Clone + 'static,
        T: DeserializeOwned + Send + 'static,
    {
        let (http, pair, waker) = (self.http.clone(), self.pair, self.waker.clone());
        Resource::with_reply(source, move |key, cancel, reply| {
            let waker = waker.clone();
            http.send_json::<T>(call(&key).req, &cancel, move |out| {
                let out = out.map_err(ApiError::from);
                if let (true, Some(w)) = (out.as_ref().is_err_and(|e| e.pair), &waker) {
                    let _ = w.post(move || pair.set(true));
                }
                reply.send(out);
            });
        })
    }
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` percent-encoded.
pub fn encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn get<T>(path: impl Into<String>) -> Call<T> {
    Call::new(Request::get(path))
}

fn post<T>(path: impl Into<String>) -> Call<T> {
    Call::new(Request::post(path))
}

fn post_json<T>(path: impl Into<String>, body: &impl serde::Serialize) -> Call<T> {
    Call::new(Request::post(path).json(body))
}

fn put_json<T>(path: impl Into<String>, body: &impl serde::Serialize) -> Call<T> {
    Call::new(Request::put(path).json(body))
}

fn delete<T>(path: impl Into<String>) -> Call<T> {
    Call::new(Request::delete(path))
}

/// `watchlist` or `bookmarks`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    Watchlist,
    Bookmarks,
}

impl List {
    pub fn name(self) -> &'static str {
        match self {
            List::Watchlist => "watchlist",
            List::Bookmarks => "bookmarks",
        }
    }
}

// ---- Status, updates, engine ----

/// No time limit, as the old page's `fetch`: a busy Kodi answers late, not "offline".
pub fn status() -> Call<Status> {
    get("/api/status").no_timeout()
}
pub fn update() -> Call<UpdateInfo> {
    get("/api/update")
}
pub fn check_update() -> Call<UpdateInfo> {
    post("/api/update/check")
}
/// The service accepts this only from this PC; the header keeps other origins out.
pub fn install_update() -> Call<UpdateInfo> {
    Call::new(Request::post("/api/update/install").header("x-portal-launcher", "1"))
}
pub fn engine_restart() -> Call<Status> {
    post("/api/engine/restart")
}
pub fn engine_quit() -> Call<Status> {
    post("/api/engine/quit")
}
pub fn engine_window(visible: bool) -> Call<Status> {
    post_json("/api/engine/window", &serde_json::json!({ "visible": visible }))
}

// ---- Library ----

pub fn source() -> Call<Source> {
    get("/api/source")
}
pub fn set_source(source: &Source) -> Call<OkBody> {
    put_json("/api/source", source)
}
pub fn home() -> Call<Home> {
    get("/api/home")
}
pub fn browse(path: &str) -> Call<BrowsePage> {
    get(format!("/api/browse?path={}", encode(path)))
}
pub fn search(q: &str) -> Call<BrowsePage> {
    get(format!("/api/search?q={}", encode(q)))
}
pub fn title(id: &str) -> Call<TitleDetail> {
    get(format!("/api/title/{}", encode(id)))
}
pub fn list(list: List) -> Call<CardList> {
    get(format!("/api/{}", list.name()))
}
pub fn put_list(list: List, id: &str, body: &ListPut) -> Call<OkBody> {
    put_json(format!("/api/{}/{}", list.name(), encode(id)), body)
}
pub fn delete_list(list: List, id: &str) -> Call<OkBody> {
    delete(format!("/api/{}/{}", list.name(), encode(id)))
}
/// Watchlist and bookmark state for many titles in one call.
pub fn saved(ids: &[String]) -> Call<SavedMap> {
    post_json("/api/saved", &serde_json::json!({ "ids": ids }))
}
/// Merges what is known with online lookups; can take a few seconds.
pub fn info(seed: &MediaInfo) -> Call<MediaInfo> {
    post_json("/api/info", seed)
}
pub fn metadata() -> Call<MetadataSettings> {
    get("/api/metadata")
}
pub fn set_metadata(body: &MetadataPut) -> Call<MetadataSettings> {
    put_json("/api/metadata", body)
}
pub fn history() -> Call<HistoryList> {
    get("/api/history")
}
pub fn update_history(slot: usize, body: &HistoryName) -> Call<OkBody> {
    put_json(format!("/api/history/{slot}"), body)
}
pub fn delete_history(slot: usize, row: &HistoryTitle) -> Call<OkBody> {
    let mut q = Vec::new();
    if let Some(id) = &row.id {
        q.push(format!("id={}", encode(id)));
    }
    if let Some(s) = row.season {
        q.push(format!("season={s}"));
    }
    if let Some(e) = row.episode {
        q.push(format!("episode={e}"));
    }
    let query = if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) };
    delete(format!("/api/history/{slot}{query}"))
}
pub fn clear_history() -> Call<OkBody> {
    delete("/api/history")
}
pub fn progress(body: &ProgressBody) -> Call<OkBody> {
    post_json("/api/progress", body)
}

// ---- Subtitles ----

/// Online subtitles for what is playing; can take a few seconds.
pub fn search_subtitles(body: &SubtitleSearch) -> Call<SubtitleResults> {
    post_json("/api/subtitles/search", body)
}
/// The subtitle's WebVTT text, shifted by `offset` seconds (send with [`Api::run_text`]).
pub fn subtitle_file(id: &str, offset: f64) -> Call<String> {
    get(subtitle_file_url(id, offset))
}
/// WebVTT for an online subtitle, lined up with a stream that starts `offset` seconds in.
pub fn subtitle_file_url(id: &str, offset: f64) -> String {
    let at = if offset.is_finite() && offset > 0.0 { (offset * 1000.0).round() / 1000.0 } else { 0.0 };
    format!("/api/subtitles/file/{}?offset={at}", encode(id))
}
/// A text file Portal serves, such as a video's own subtitle track (send with [`Api::run_text`]).
pub fn text_file(url: &str) -> Call<String> {
    get(url.to_owned())
}
pub fn subtitle_settings() -> Call<SubtitleSettings> {
    get("/api/subtitles/settings")
}
pub fn set_subtitle_settings(body: &SubtitleSettingsPut) -> Call<SubtitleSettings> {
    put_json("/api/subtitles/settings", body)
}

// ---- Add-ons, settings, prompts ----

pub fn addons() -> Call<AddonList> {
    get("/api/addons")
}
pub fn settings(addon: &str) -> Call<SettingsDoc> {
    get(format!("/api/addons/{}/settings", encode(addon)))
}
pub fn write_setting(addon: &str, setting: &str, value: &str) -> Call<OkBody> {
    put_json(
        format!("/api/addons/{}/settings/{}", encode(addon), encode(setting)),
        &serde_json::json!({ "value": value }),
    )
}
pub fn action(addon: &str, setting: &str) -> Call<ActionBody> {
    post(format!("/api/addons/{}/actions/{}", encode(addon), encode(setting)))
}
pub fn run_plugin(file: &str) -> Call<ActionBody> {
    post_json("/api/run", &RunBody { file: file.into() })
}
/// Saves a file where Kodi can read it and answers that path.
pub fn upload_incoming(name: &str, content_type: &str, bytes: Vec<u8>) -> Call<IncomingFile> {
    let form = remedy_ui::net::Multipart::new().file("file", name, content_type, bytes);
    Call::new(Request::post("/api/incoming").multipart(&form))
}
pub fn prompt() -> Call<PromptBody> {
    get("/api/prompt").no_timeout()
}
pub fn answer_prompt(body: &PromptAnswer) -> Call<OkBody> {
    post_json("/api/prompt", body).no_timeout()
}
pub fn repos() -> Call<RepoList> {
    get("/api/repos")
}
pub fn repo(id: &str) -> Call<RepoDetail> {
    get(format!("/api/repos/{}", encode(id)))
}
pub fn install_zip(url: &str, name: Option<&str>) -> Call<InstallResult> {
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(str::to_owned);
    post_json("/api/repos", &ZipInstall { url: url.into(), name })
}
pub fn install_addon(repo: &str, addon: &str) -> Call<InstallResult> {
    post(format!("/api/repos/{}/addons/{}", encode(repo), encode(addon)))
}
pub fn remove_repo(id: &str) -> Call<RemoveResult> {
    delete(format!("/api/repos/{}", encode(id)))
}
pub fn addon_updates(fresh: bool) -> Call<AddonUpdates> {
    get(if fresh { "/api/addons/updates?fresh=true" } else { "/api/addons/updates" })
}
pub fn update_addon(id: &str) -> Call<InstallResult> {
    post(format!("/api/addons/{}/update", encode(id)))
}
pub fn remove_addon(id: &str) -> Call<RemoveResult> {
    delete(format!("/api/addons/{}", encode(id)))
}

// ---- Pairing ----

pub fn pair_me() -> Call<PairMe> {
    get("/api/pair/me")
}
pub fn pair_start() -> Call<PairStarted> {
    post_json("/api/pair/start", &serde_json::json!({}))
}
pub fn pair_status(ticket: &str) -> Call<PairStatus> {
    get(format!("/api/pair/status?ticket={}", encode(ticket)))
}
pub fn pair_approve(code: &str) -> Call<PairApproved> {
    post_json("/api/pair/approve", &PairApprove { code: code.into() })
}
pub fn devices() -> Call<Vec<PairedDevice>> {
    get("/api/devices")
}
pub fn remove_device(id: &str) -> Call<OkBody> {
    delete(format!("/api/devices/{}", encode(id)))
}

// ---- Playback ----

pub fn play(body: &PlayRequest) -> Call<PlayAccepted> {
    post_json("/api/play", body).no_timeout()
}
pub fn job(id: &str) -> Call<PlayJob> {
    get(format!("/api/play/{}", encode(id))).no_timeout()
}
pub fn early(id: &str, body: &PlayRequest) -> Call<OkBody> {
    post_json(format!("/api/play/{}/early", encode(id)), body)
}
pub fn seek(id: &str, seconds: f64) -> Call<PlayJob> {
    post_json(format!("/api/play/{}/seek", encode(id)), &SeekBody { seconds }).no_timeout()
}
/// Restarts the job's stream at `seconds` in a new quality; the add-on is not asked again.
pub fn set_quality(id: &str, quality: Quality, seconds: f64) -> Call<PlayJob> {
    post_json(format!("/api/play/{}/quality", encode(id)), &QualityBody { quality, seconds }).no_timeout()
}
/// Restarts the job's stream at `seconds` on another audio track.
pub fn set_audio(id: &str, index: u32, seconds: f64) -> Call<PlayJob> {
    post_json(format!("/api/play/{}/audio", encode(id)), &AudioBody { index, seconds }).no_timeout()
}
pub fn stop(id: &str) -> Call<OkBody> {
    delete(format!("/api/play/{}", encode(id)))
}
pub fn prefs() -> Call<PrefsBody> {
    get("/api/prefs")
}
pub fn set_prefs(body: &PrefsPut) -> Call<PrefsBody> {
    put_json("/api/prefs", body)
}

// ---- The PC's own menu ----

pub fn app_info() -> Call<AppView> {
    get("/api/app")
}
pub fn app_autostart(on: bool) -> Call<AppView> {
    post_json("/api/app/autostart", &OnBody { on })
}
pub fn app_encoder(on: bool) -> Call<AppView> {
    post_json("/api/app/encoder", &OnBody { on })
}
pub fn app_restart() -> Call<OkBody> {
    post("/api/app/restart")
}
pub fn app_quit() -> Call<OkBody> {
    post("/api/app/quit")
}
pub fn app_logs() -> Call<OkBody> {
    post("/api/app/logs")
}

// ---- Casting ----

pub fn cast_info() -> Call<CastBody> {
    get("/api/cast")
}
pub fn set_cast_lan(lan: bool) -> Call<CastBody> {
    put_json("/api/cast", &CastLanPut { lan })
}
pub fn cast_devices() -> Call<CastDeviceList> {
    get("/api/cast/devices")
}
pub fn cast_device() -> Call<CastDeviceView> {
    get("/api/cast/device")
}
pub fn cast_device_play(id: &str, body: &CastPlayBody) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/play", encode(id)), body)
}
pub fn cast_device_resume(id: &str) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/play", encode(id)), &serde_json::json!({}))
}
pub fn cast_device_stop(id: &str) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/stop", encode(id)), &serde_json::json!({}))
}
pub fn cast_device_pause(id: &str) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/pause", encode(id)), &serde_json::json!({}))
}
/// Shows `subtitle` on the cast, or none for `None`.
pub fn cast_device_subtitles(id: &str, subtitle: Option<&CastSubtitle>) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/subtitles", encode(id)), &subtitle)
}
/// Moves the cast to a place in the title, in seconds.
pub fn cast_device_seek(id: &str, seconds: f64) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/seek", encode(id)), &SeekBody { seconds })
}
/// Sets the TV's own volume (0 to 1) and mute; either may be left out.
pub fn cast_device_volume(id: &str, change: &CastVolumeBody) -> Call<OkBody> {
    post_json(format!("/api/cast/devices/{}/volume", encode(id)), change)
}
// The TV-browser receiver routes (`/api/cast/presence`, `/receivers`, `/session*`) are not
// here: the receiver page (`#/cast`) stays on the old UI (port plan, Decisions).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_like_encode_uri_component() {
        assert_eq!(encode("plugin://plugin.video.x/?a=1&b=two words"), "plugin%3A%2F%2Fplugin.video.x%2F%3Fa%3D1%26b%3Dtwo%20words");
        assert_eq!(encode("A-z_0.9!~*'()"), "A-z_0.9!~*'()");
        assert_eq!(encode("é"), "%C3%A9");
    }

    #[test]
    fn routes_build_api_ts_paths() {
        let path = |r: &Request| format!("{r:?}");
        assert!(path(browse("a/b").request()).contains("\"/api/browse?path=a%2Fb\""));
        assert!(path(delete_history(3, &HistoryTitle { id: Some("t 1".into()), season: Some(2), episode: None }).request())
            .contains("\"/api/history/3?id=t%201&season=2\""));
        assert!(path(addon_updates(true).request()).contains("\"/api/addons/updates?fresh=true\""));
        assert_eq!(subtitle_file_url("x", 0.0), "/api/subtitles/file/x?offset=0");
        assert_eq!(subtitle_file_url("x", -1.5), "/api/subtitles/file/x?offset=0");
        assert_eq!(subtitle_file_url("x", 12.34567), "/api/subtitles/file/x?offset=12.346");
    }

    #[test]
    fn errors_keep_the_detail_and_spot_pairing() {
        let e = ApiError::answered(401, r#"{"detail":"Pair first.","pair":true}"#);
        assert_eq!((e.detail.as_str(), e.pair), ("Pair first.", true));
        let e = ApiError::answered(403, r#"{"detail":"PC only."}"#);
        assert_eq!((e.detail.as_str(), e.pair), ("PC only.", false));
        let e = ApiError::answered(502, "<html>bad gateway</html>");
        assert_eq!((e.detail.as_str(), e.pair), ("Request failed (502)", false));
    }
}
