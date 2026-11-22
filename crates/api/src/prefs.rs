//! Playback choices shared by every device (`/api/prefs`), kept in `prefs.json`.

use serde::{Deserialize, Deserializer, Serialize};

use crate::Quality;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SubtitleMode {
    #[default]
    Off,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default)]
    pub quality: Quality,
    #[serde(default)]
    pub subtitles: SubtitleMode,
    /// The language of the last subtitle the user picked, e.g. "en" or "pt-BR".
    #[serde(default)]
    pub subtitle_language: Option<String>,
    /// The language of the last audio track the user picked.
    #[serde(default)]
    pub audio_language: Option<String>,
    #[serde(default = "autoplay_by_default")]
    pub autoplay: bool,
}

fn autoplay_by_default() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            quality: Quality::Auto,
            subtitles: SubtitleMode::Off,
            subtitle_language: None,
            audio_language: None,
            autoplay: true,
        }
    }
}

/// `GET /api/prefs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefsBody {
    #[serde(flatten)]
    pub prefs: Prefs,
    /// False until a choice has been saved; the page then moves its own over.
    pub stored: bool,
}

/// `PUT /api/prefs`: a partial change. Fields left out stay as they are; unknown fields are
/// ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefsPut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitles: Option<String>,
    /// `null` forgets the language; a missing field keeps it.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub subtitle_language: Option<Option<String>>,
    /// `null` forgets the language; a missing field keeps it.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub audio_language: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autoplay: Option<bool>,
}

fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}
