//! Casting (`/api/cast*`): to a Cast TV the server drives, and to a TV browser running
//! Portal's receiver page.

use serde::{Deserialize, Serialize};

/// What is being cast, so a page can show it and play it again.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastReplay {
    pub kind: String,
    pub id: String,
    pub file: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_id: Option<String>,
}

/// Where the browser TV is, in source seconds. The dashboard clock follows this.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastPlace {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub updated_ms: u64,
}

/// One instruction for the browser TV. `seq` rises so the TV applies it once.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastCommand {
    pub seq: u64,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

/// A browser cast that ended because its TV went quiet. The dashboard following it says so
/// and resumes from the TV's last reported place.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LostCast {
    pub receiver_id: String,
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<CastPlace>,
}

/// What the dashboard shows for a device cast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CastDeviceState {
    Starting,
    Playing,
    Paused,
    Buffering,
    Idle,
    /// Starting the next episode.
    Next,
    Ended,
    Stopped,
    Failed,
}

/// A subtitle a cast shows: a track in the video, or one from the online search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CastSubtitle {
    Embedded {
        id: u32,
        #[serde(default)]
        lang: Option<String>,
    },
    Online {
        id: String,
        #[serde(default)]
        lang: Option<String>,
    },
}

impl CastSubtitle {
    pub fn lang(&self) -> Option<&str> {
        match self {
            Self::Embedded { lang, .. } | Self::Online { lang, .. } => lang.as_deref(),
        }
    }
}

/// `GET /api/cast/device`: the device cast as its watch last saw it. Times are in the
/// source's seconds, like the dashboard's clock.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CastDeviceView {
    /// False once the cast ended; `state` and `note` then say how.
    pub active: bool,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub job_id: Option<String>,
    pub title: Option<String>,
    /// What is playing, so a page can show it and play it again.
    pub item: Option<CastReplay>,
    pub state: Option<CastDeviceState>,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    /// Where in the source the device's stream starts; device seeks are relative to it.
    pub origin: f64,
    pub note: Option<String>,
    pub updated_ms: u64,
    /// The subtitle the TV shows, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<CastSubtitle>,
    /// Server bookkeeping, never on the wire: the play job that subtitle belongs to;
    /// another job (the next episode) needs its own, found by language.
    #[serde(skip)]
    pub subtitle_job: Option<String>,
    /// Server bookkeeping, never on the wire: the language the user wants on the TV, kept
    /// across episodes while the next one's subtitle is still being found.
    #[serde(skip)]
    pub subtitle_lang: Option<String>,
    /// Portal is still finding or sending a subtitle in that language for this job.
    pub subtitle_pending: bool,
    /// The TV's own volume, 0 to 1, as its last status line said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}

/// A TV browser showing Portal's receiver page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastReceiver {
    pub id: String,
    pub name: String,
    pub seen_ms: u64,
}

/// `GET /api/cast/receivers`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastReceiverList {
    pub items: Vec<CastReceiver>,
}

/// `GET /api/cast/session`: the browser cast, if any.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CastSessionView {
    pub receiver_id: Option<String>,
    pub job_id: Option<String>,
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_id: Option<String>,
    /// Where the browser TV is. Absent until it reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<CastPlace>,
    /// The dashboard's latest play, pause, or seek. The TV applies `seq` once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<CastCommand>,
    /// With no session: the one that ended because its TV went quiet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost: Option<LostCast>,
}

/// `GET /api/cast`: casting settings and what is casting to a browser.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastBody {
    pub lan: bool,
    pub bind: String,
    pub url: Option<String>,
    pub receivers: Vec<CastReceiver>,
    pub session: Option<CastSessionView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restart_needed: Option<bool>,
}

/// A Cast TV found on the network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastDevice {
    pub id: String,
    pub name: String,
    pub host: String,
    pub model: String,
}

/// `GET /api/cast/devices`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastDeviceList {
    pub items: Vec<CastDevice>,
}

/// `PUT /api/cast`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastLanPut {
    pub lan: bool,
}

/// `POST /api/cast/presence`, from a TV browser's receiver page.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastPresencePut {
    pub id: String,
    pub name: String,
}

/// What a page sends so the item can be shown and played again.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReplayFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub art: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_id: Option<String>,
}

/// `PUT /api/cast/session`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CastSessionPut {
    pub receiver_id: String,
    pub job_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(flatten)]
    pub replay: ReplayFields,
}

/// `POST /api/cast/session/place`, from the TV browser.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CastPlacePut {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
}

/// `POST /api/cast/session/command`: play, pause or seek on the browser TV.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CastCommandPut {
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<f64>,
}

/// `POST /api/cast/devices/{id}/play`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CastPlayBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb: Option<String>,
    /// The play job being cast. With it, Portal watches the device and plays the next
    /// episode when this one finishes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
    /// Where in the stream the device starts, in seconds (the browser's place).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<f64>,
    /// The subtitle the dashboard shows, sent along to the TV.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<CastSubtitle>,
    #[serde(flatten)]
    pub replay: ReplayFields,
}

/// `POST /api/cast/devices/{id}/volume`: the TV's own volume, 0 to 1, and mute; either
/// may be left out.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct CastVolumeBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
}
