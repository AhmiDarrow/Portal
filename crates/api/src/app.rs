//! The tray's choices for the dashboard on this PC (`/api/app*`), and the small bodies the
//! other routes send and answer with.

use serde::{Deserialize, Serialize};

/// `GET /api/app`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppView {
    /// False when the tray program is missing, as in a source checkout.
    pub tray: bool,
    /// Whether Portal starts when the user signs in; absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autostart: Option<bool>,
    pub encoder: EncoderView,
}

/// What makes the picture when a video has to be encoded (`GET /api/app/encoder`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EncoderView {
    /// Whether a graphics card may be used, as the user last chose.
    pub hardware: bool,
    /// The card this PC has for it, or "cpu" when it has none that works.
    pub card: String,
}

/// A switch turned on or off (`POST /api/app/encoder`, `POST /api/app/autostart`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct OnBody {
    pub on: bool,
}

/// `GET /api/ping`: identifies this service to the desktop launcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ping {
    // In the order the server's json! map wrote them (alphabetical).
    pub app: String,
    pub pid: u32,
    pub version: String,
}

/// The body of every error answer. `pair` is true on the 401 that asks this device to pair.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<bool>,
}

/// `POST /api/incoming`: where the uploaded file was saved, a path Kodi can read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingFile {
    pub path: String,
}

/// What an add-on settings action or `POST /api/run` did.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionBody {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browse_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_addon: Option<String>,
    /// True when a RunPlugin action was started; the page should watch `/api/prompt`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ran: bool,
}

/// `POST /api/run`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunBody {
    pub file: String,
}

/// A place in seconds (`POST /api/play/{id}/seek`, `POST /api/cast/devices/{id}/seek`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct SeekBody {
    pub seconds: f64,
}

/// `POST /api/play/{id}/quality`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct QualityBody {
    pub quality: crate::Quality,
    pub seconds: f64,
}

/// `POST /api/play/{id}/audio`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct AudioBody {
    pub index: u32,
    pub seconds: f64,
}

/// `POST /api/repos`: install a repository or add-on zip from an address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ZipInstall {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Which history row a rename or removal means, checked against the slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryTitle {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub season: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode: Option<u32>,
}

/// `PUT /api/history/{slot}`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryName {
    #[serde(default)]
    pub name: String,
    #[serde(flatten)]
    pub title: HistoryTitle,
}

/// `POST /api/progress`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressBody {
    #[serde(flatten)]
    pub report: crate::ProgressReport,
    /// Pause, stop, seek, and leaving the page ask to write the file now. A steady
    /// playhead omits it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flush: bool,
}
