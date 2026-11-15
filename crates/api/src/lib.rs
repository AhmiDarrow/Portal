//! JSON shapes shared by the Portal service and the dashboard.
//! Field names are snake_case on the wire.

use serde::{Deserialize, Serialize};

pub const WATCHED_AT: f64 = 0.9;
pub const RESUME_AFTER_SECONDS: f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineState {
    Running,
    Starting,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub state: EngineState,
    pub pid: Option<u32>,
    pub version: Option<String>,
    pub started_by_us: bool,
    pub window_visible: bool,
    pub webserver: bool,
    pub error: Option<String>,
}

impl Status {
    pub fn stopped(error: Option<String>) -> Self {
        Self {
            state: EngineState::Stopped,
            pid: None,
            version: None,
            started_by_us: false,
            window_visible: false,
            webserver: false,
            error,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Open,
    Play,
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TitleKind {
    Movie,
    Tv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayKind {
    Movie,
    Episode,
    Song,
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Quality {
    #[serde(rename = "auto")]
    #[default]
    Auto,
    #[serde(rename = "source")]
    Source,
    #[serde(rename = "2160")]
    P2160,
    #[serde(rename = "1080")]
    P1080,
    #[serde(rename = "720")]
    P720,
    #[serde(rename = "480")]
    P480,
}

impl Quality {
    pub fn parse(text: &str) -> Self {
        match text {
            "source" => Self::Source,
            "2160" => Self::P2160,
            "1080" => Self::P1080,
            "720" => Self::P720,
            "480" => Self::P480,
            _ => Self::Auto,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Source => "source",
            Self::P2160 => "2160",
            Self::P1080 => "1080",
            Self::P720 => "720",
            Self::P480 => "480",
        }
    }

    pub fn cap_height(self) -> Option<i64> {
        match self {
            Self::Auto => Some(1080),
            Self::Source => None,
            Self::P2160 => Some(2160),
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
            Self::P480 => Some(480),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub addon_id: Option<String>,
    pub name: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AddonCard {
    pub id: String,
    pub name: String,
    pub path: String,
    pub art: Option<String>,
    /// A copy under the Kodi profile. One that ships with Kodi is left alone.
    #[serde(default)]
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AddonList {
    pub items: Vec<AddonCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoCard {
    pub id: String,
    pub name: String,
    /// A copy under the Kodi profile. One that ships with Kodi is left alone.
    #[serde(default)]
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoList {
    pub items: Vec<RepoCard>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoAddon {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub installed: bool,
    /// Installed under the Kodi profile, so it can be removed.
    #[serde(default)]
    pub removable: bool,
    /// `video`, or `module` for a Python module other add-ons use.
    #[serde(default = "video_kind")]
    pub kind: String,
}

fn video_kind() -> String {
    "video".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RepoDetail {
    pub id: String,
    pub name: String,
    pub items: Vec<RepoAddon>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledAddon {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstallResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub wrote: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoveResult {
    pub id: String,
    pub name: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applies: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowseItem {
    pub id: String,
    pub label: String,
    pub path: String,
    pub kind: ItemKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title_kind: Option<TitleKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_addon: Option<String>,
    /// Video info labels the add-on set on this item. Absent when it set none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<MediaInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowsePage {
    pub path: String,
    pub items: Vec<BrowseItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Card {
    pub id: String,
    pub kind: TitleKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardList {
    pub items: Vec<Card>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Home {
    pub continue_watching: Vec<Card>,
    pub watchlist: Vec<Card>,
    pub bookmarks: Vec<Card>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Episode {
    pub season: u32,
    pub episode: u32,
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    pub position: f64,
    pub duration: f64,
    pub finished: bool,
    pub resumable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Season {
    pub number: u32,
    pub episodes: Vec<Episode>,
}

/// An add-on folder that lists one season of a show, as Portal last saw it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SeasonFolder {
    pub season: u32,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EpisodeRef {
    pub season: u32,
    pub episode: u32,
    pub name: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TitleDetail {
    pub id: String,
    pub kind: TitleKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    pub bookmarked: bool,
    pub watchlisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_seconds: Option<f64>,
    pub watched: bool,
    pub seasons: Vec<Season>,
    /// The show's season folders in the add-on, lowest season first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub season_folders: Vec<SeasonFolder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<EpisodeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressReport {
    pub id: String,
    pub kind: TitleKind,
    pub name: String,
    #[serde(default)]
    pub art: Option<String>,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
    #[serde(default)]
    pub episode_name: Option<String>,
    #[serde(default)]
    pub file: Option<String>,
    pub position: f64,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEntry {
    pub id: String,
    pub kind: TitleKind,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    pub at: i64,
    pub position: f64,
    pub duration: f64,
    pub finished: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Index in the stored history, oldest first. The list itself is newest first.
    pub slot: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryList {
    pub items: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListPut {
    #[serde(default)]
    pub kind: Option<TitleKind>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub art: Option<String>,
    #[serde(default)]
    pub year: Option<u32>,
    /// An IMDb id (`tt` and digits). Anything else is ignored.
    #[serde(default)]
    pub imdb_id: Option<String>,
}

/// `POST /api/saved` body.
#[derive(Debug, Clone, Deserialize)]
pub struct SavedQuery {
    #[serde(default)]
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SavedState {
    pub watchlist: bool,
    pub bookmarks: bool,
}

/// `POST /api/saved` answer: one entry per requested id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SavedMap {
    pub items: std::collections::BTreeMap<String, SavedState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayRequest {
    pub kind: PlayKind,
    pub id: String,
    pub file: String,
    pub title: String,
    #[serde(default)]
    pub quality: Quality,
    #[serde(default)]
    pub season: Option<u32>,
    #[serde(default)]
    pub episode: Option<u32>,
    #[serde(default)]
    pub art: Option<String>,
    #[serde(default)]
    pub show_title: Option<String>,
    #[serde(default)]
    pub show_id: Option<String>,
    /// Seconds to seek before the first HLS segment. Values of 60 or below are ignored.
    #[serde(default)]
    pub resume_seconds: Option<f64>,
    /// When true, a source list the add-on shows is handed to the viewer
    /// instead of Portal choosing a copy for the quality.
    #[serde(default)]
    pub manual_links: bool,
    /// Language to prefer when the file has more than one audio track, e.g. "en".
    /// Absent means a saved player choice, then the file's own default track.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayAccepted {
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Working,
    Prompt,
    Browse,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtitleTrack {
    pub id: u32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// True for a forced track, which only covers foreign-language parts.
    #[serde(default)]
    pub forced: bool,
    pub url: String,
}

/// One audio stream in the playing file. `index` is the ffprobe stream index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudioTrack {
    pub index: u32,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NextItem {
    pub kind: PlayKind,
    pub id: String,
    pub file: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayJob {
    pub id: String,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtitles: Vec<SubtitleTrack>,
    /// True while embedded subtitles are still being read after the video
    /// became ready. `subtitles` fills in once this turns false.
    #[serde(default)]
    pub subtitles_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<NextItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    /// The playing file's name without its extension, e.g.
    /// "Show.S01E01.720p", when the stream's address ends in a
    /// video file name. Only used to rank online subtitles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    /// Audio tracks in the file. Empty until the stream has been read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio: Vec<AudioTrack>,
    /// The ffprobe index of the track that is playing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_index: Option<u32>,
    /// The dialog Kodi is showing, when this response already read it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<Prompt>,
    /// True when `prompt` was read for this response, including when Kodi is
    /// not asking anything. The page then does not read the dialog again.
    #[serde(default, skip_serializing_if = "skip_false")]
    pub prompt_known: bool,
}

fn skip_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    Bool,
    Text,
    Number,
    Select,
    Action,
    Header,
    Secret,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingsField {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub kind: FieldKind,
    pub label: String,
    pub value: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<SelectOption>,
    pub secret: bool,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub action: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingsCategory {
    pub label: String,
    pub fields: Vec<SettingsField>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingsDoc {
    pub addon_id: String,
    pub categories: Vec<SettingsCategory>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SettingWrite {
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptKind {
    Text,
    Select,
    Yesno,
    Ok,
    Progress,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Prompt {
    pub id: String,
    pub kind: PromptKind,
    pub title: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    pub cancellable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptBody {
    pub prompt: Option<Prompt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptAction {
    Submit,
    Cancel,
    Yes,
    No,
    Ok,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptAnswer {
    pub id: String,
    pub action: PromptAction,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub index: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WindowRequest {
    pub visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OkBody {
    pub ok: bool,
}

/// Where an update stands. `Ready` means the installer is downloaded and its
/// checksum matched; `Installing` means it was started and will stop Portal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateState {
    Idle,
    Checking,
    Downloading,
    Ready,
    Installing,
    Failed,
}

/// `GET /api/update`. `latest` is the newest release's version, or null when
/// the repository has no usable release. `available` is true only when that
/// release is newer than `current` and carries a Windows installer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    pub state: UpdateState,
    /// 0..1 while downloading, when the size is known.
    pub progress: Option<f64>,
    pub error: Option<String>,
    pub release_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Movie,
    Tv,
    Episode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CastMember {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InfoLink {
    pub label: String,
    pub url: String,
}

/// What Portal knows about a movie, show, or episode. Every field is optional
/// and left out of the JSON when empty. `poster`, `fanart` and cast `thumb`
/// use the same form as `art`: a Kodi image path or an http(s) URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MediaInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    /// `YYYY-MM-DD`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub premiered: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tagline: Option<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub genres: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpaa: Option<String>,
    /// 0 to 10.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub votes: Option<u64>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub studios: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub directors: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub writers: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub countries: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub cast: Vec<CastMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trailer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fanart: Option<String>,
    /// Where the data came from, e.g. "Kodi add-on", "TMDB", "TVmaze", "Wikipedia".
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub sources: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub links: Vec<InfoLink>,
}

/// The most cast members a `MediaInfo` carries.
pub const CAST_LIMIT: usize = 12;

/// `GET /api/metadata` and the answer to `PUT /api/metadata`. The TMDB key
/// itself is never sent back.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataSettings {
    pub online: bool,
    pub tmdb: bool,
}

/// `PUT /api/metadata`. A missing `tmdb_key` keeps the stored key; `null`
/// clears it.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MetadataPut {
    #[serde(default)]
    pub online: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub tmdb_key: Option<Option<String>>,
}

/// `POST /api/subtitles/search`. For an episode, `title` may be the
/// episode's name and `show_title` the show's; ids may be the show's or the
/// episode's. `release` is the playing file's name (`PlayJob::release`) and
/// never leaves Portal. `languages` overrides the saved ones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SubtitleSearch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<MediaKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tvdb_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub languages: Vec<String>,
}

/// Why a subtitle ranks where it does: the playing file's hash, its release
/// name, or only the title.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleMatch {
    Hash,
    Release,
    Title,
}

/// One online subtitle. `id` is opaque: pass it to
/// `GET /api/subtitles/file/{id}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SubtitleCandidate {
    pub id: String,
    /// "OpenSubtitles", "Gestdown", "TVsubtitles",
    /// "Subtitle Cat", "SubDL" or "SubSource".
    pub source: String,
    /// A language code such as "en" or "pt-BR".
    pub language: String,
    /// The language's English name, e.g. "Portuguese (Brazil)".
    pub language_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub release: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hearing_impaired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f64>,
    /// Higher is better; only the order means anything.
    pub score: u32,
    #[serde(rename = "match")]
    pub matched: SubtitleMatch,
}

/// The answer to `POST /api/subtitles/search`, best first. `notes` explain
/// a source that could not answer, e.g. a used-up daily quota.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SubtitleResults {
    #[serde(default, deserialize_with = "null_default")]
    pub items: Vec<SubtitleCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub notes: Vec<String>,
}

/// `GET /api/subtitles/settings` and the answer to its `PUT`. The API keys
/// and the OpenSubtitles password are never sent back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SubtitleSettings {
    /// Preferred languages, best first. Empty until the user picks some.
    #[serde(default, deserialize_with = "null_default")]
    pub languages: Vec<String>,
    #[serde(default)]
    pub hearing_impaired: bool,
    /// An OpenSubtitles API key is saved.
    #[serde(default)]
    pub opensubtitles: bool,
    /// An OpenSubtitles username and password are saved.
    #[serde(default)]
    pub opensubtitles_user: bool,
    /// A SubDL API key is saved.
    #[serde(default)]
    pub subdl: bool,
    /// A SubSource API key is saved.
    #[serde(default)]
    pub subsource: bool,
}

/// `PUT /api/subtitles/settings`. A missing field keeps what is saved;
/// `null` (or a blank string) removes a key, username or password.
#[derive(Clone, Deserialize, Default)]
pub struct SubtitleSettingsPut {
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub hearing_impaired: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub opensubtitles_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub opensubtitles_username: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub opensubtitles_password: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub subdl_key: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    pub subsource_key: Option<Option<String>>,
}

/// Never prints the keys or the password.
impl std::fmt::Debug for SubtitleSettingsPut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = |value: &Option<Option<String>>| match value {
            None => "unchanged",
            Some(None) => "cleared",
            Some(Some(_)) => "set",
        };
        f.debug_struct("SubtitleSettingsPut")
            .field("languages", &self.languages)
            .field("hearing_impaired", &self.hearing_impaired)
            .field("opensubtitles_key", &state(&self.opensubtitles_key))
            .field(
                "opensubtitles_username",
                &state(&self.opensubtitles_username),
            )
            .field(
                "opensubtitles_password",
                &state(&self.opensubtitles_password),
            )
            .field("subdl_key", &state(&self.subdl_key))
            .field("subsource_key", &state(&self.subsource_key))
            .finish()
    }
}

/// Reads `null` like a missing field.
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// Tells a field sent as `null` apart from a field left out.
fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

/// The IMDb id inside `text` (`tt` and 5 to 10 digits), e.g. from
/// "tt0000001" or "https://www.imdb.com/title/tt0000001/".
pub fn imdb_id(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(offset) = text[start..].find("tt") {
        let at = start + offset;
        let before_ok = at == 0 || !bytes[at - 1].is_ascii_alphanumeric();
        let digits = bytes[at + 2..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        let after = at + 2 + digits;
        let after_ok = after == bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok && (5..=10).contains(&digits) {
            return Some(text[at..after].to_string());
        }
        start = at + 1;
    }
    None
}

#[cfg(test)]
#[path = "../../../tests-private/api/src/lib.rs"]
mod tests;
