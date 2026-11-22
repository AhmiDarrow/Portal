//! `%LOCALAPPDATA%\Portal\prefs.json`: the player choices every screen shares,
//! so the dashboard, another browser, and a TV's cast page start the same way.
//! Volume is not here; it belongs to each device.
//!
//! `GET /api/prefs` answers the saved choices (or the defaults) and `stored`,
//! which is false until a choice has been saved. `PUT /api/prefs` changes only
//! the fields it names. Portal has no login, so any screen on the network may
//! change these, like the rest of the API. The JSON extractor refuses other
//! content types, so a cross-site form post cannot reach it, and the body
//! limit on the route keeps a request small.

use std::path::{Path, PathBuf};

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use portal_api::Quality;
use serde::{Deserialize, Deserializer, Serialize};

/// The largest body `PUT /api/prefs` reads.
pub const BODY_LIMIT: usize = 2048;
const LANGUAGE_LIMIT: usize = 16;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrefsBody {
    #[serde(flatten)]
    pub prefs: Prefs,
    /// False until a choice has been saved; the page then moves its own over.
    pub stored: bool,
}

/// A partial change. Fields left out stay as they are; unknown fields are ignored.
#[derive(Debug, Default, Deserialize)]
pub struct PrefsPut {
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    subtitles: Option<String>,
    /// `null` forgets the language; a missing field keeps it.
    #[serde(default, deserialize_with = "present")]
    subtitle_language: Option<Option<String>>,
    /// `null` forgets the language; a missing field keeps it.
    #[serde(default, deserialize_with = "present")]
    audio_language: Option<Option<String>>,
    #[serde(default)]
    autoplay: Option<bool>,
}

fn present<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Debug, PartialEq, Eq)]
pub enum PutError {
    Invalid(&'static str),
    Save,
}

pub struct PrefsStore {
    path: PathBuf,
    prefs: Prefs,
    stored: bool,
    /// The file is present and was not read. Saving would replace it with defaults.
    held: bool,
}

impl PrefsStore {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("prefs.json");
        // A file there that cannot be read is held by `read_settings` too,
        // so saving over it fails.
        let (prefs, stored, held) = match crate::paths::read_settings(&path) {
            Some(text) => match parse_file(&text) {
                Some(prefs) => (prefs, true, false),
                None => {
                    tracing::warn!("player preferences could not be read");
                    (Prefs::default(), false, true)
                }
            },
            None => (Prefs::default(), false, false),
        };
        Self {
            path,
            prefs,
            stored,
            held,
        }
    }

    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    pub fn body(&self) -> PrefsBody {
        PrefsBody {
            prefs: self.prefs.clone(),
            stored: self.stored,
        }
    }

    /// Validates the whole change before any of it applies, then saves.
    pub fn apply(&mut self, put: PrefsPut) -> Result<PrefsBody, PutError> {
        if self.held {
            tracing::warn!("player preferences were not saved: the file could not be read");
            return Err(PutError::Save);
        }
        let next = merged(&self.prefs, put)?;
        save(&self.path, &next).map_err(|err| {
            tracing::warn!("player preferences were not saved: {err}");
            PutError::Save
        })?;
        self.prefs = next;
        self.stored = true;
        Ok(self.body())
    }
}

fn merged(current: &Prefs, put: PrefsPut) -> Result<Prefs, PutError> {
    let mut next = current.clone();
    if let Some(raw) = put.quality {
        next.quality = quality_named(raw.trim())
            .ok_or(PutError::Invalid("That quality is not one Portal offers."))?;
    }
    if let Some(raw) = put.subtitles {
        next.subtitles = match raw.trim() {
            "off" => SubtitleMode::Off,
            "auto" => SubtitleMode::Auto,
            _ => return Err(PutError::Invalid("Subtitles are \"off\" or \"auto\".")),
        };
    }
    if let Some(language) = put.subtitle_language {
        next.subtitle_language = match language {
            None => None,
            Some(raw) if raw.trim().is_empty() => None,
            Some(raw) => Some(
                clean_language(&raw)
                    .ok_or(PutError::Invalid("That subtitle language is not valid."))?,
            ),
        };
    }
    if let Some(language) = put.audio_language {
        next.audio_language = match language {
            None => None,
            Some(raw) if raw.trim().is_empty() => None,
            Some(raw) => Some(
                clean_language(&raw)
                    .ok_or(PutError::Invalid("That audio language is not valid."))?,
            ),
        };
    }
    if let Some(autoplay) = put.autoplay {
        next.autoplay = autoplay;
    }
    Ok(next)
}

/// Unlike `Quality::parse`, an unknown name is an error rather than "auto".
fn quality_named(name: &str) -> Option<Quality> {
    let quality = Quality::parse(name);
    (quality.as_str() == name).then_some(quality)
}

/// "en", "pt-BR", "eng": letters, then an optional region or script.
fn clean_language(raw: &str) -> Option<String> {
    let text = raw.trim().replace('_', "-");
    if text.is_empty() || text.len() > LANGUAGE_LIMIT {
        return None;
    }
    let mut parts = text.split('-');
    let primary = parts.next()?;
    if !(2..=3).contains(&primary.len()) || !primary.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }
    let mut out = primary.to_ascii_lowercase();
    for part in parts {
        if !(2..=8).contains(&part.len()) || !part.chars().all(|ch| ch.is_ascii_alphanumeric()) {
            return None;
        }
        out.push('-');
        out.push_str(part);
    }
    Some(out)
}

/// A whole document. A file that is not a JSON object was not read.
fn parse_file(text: &str) -> Option<Prefs> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    value.as_object()?;
    Some(parse(text))
}

/// A damaged file reads as the defaults; a bad field reads as its default.
fn parse(text: &str) -> Prefs {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Prefs::default();
    };
    let mut prefs = Prefs::default();
    if let Some(name) = value.get("quality").and_then(|item| item.as_str()) {
        prefs.quality = quality_named(name).unwrap_or_default();
    }
    if value.get("subtitles").and_then(|item| item.as_str()) == Some("auto") {
        prefs.subtitles = SubtitleMode::Auto;
    }
    prefs.subtitle_language = value
        .get("subtitle_language")
        .and_then(|item| item.as_str())
        .and_then(clean_language);
    prefs.audio_language = value
        .get("audio_language")
        .and_then(|item| item.as_str())
        .and_then(clean_language);
    if let Some(autoplay) = value.get("autoplay").and_then(|item| item.as_bool()) {
        prefs.autoplay = autoplay;
    }
    prefs
}

fn save(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    let mut body = serde_json::to_vec_pretty(prefs).map_err(std::io::Error::other)?;
    body.push(b'\n');
    crate::paths::write_settings(path, &body)
}

fn lock(state: &crate::AppState) -> std::sync::MutexGuard<'_, PrefsStore> {
    state
        .0
        .prefs
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// What a server-side decision needs, e.g. whether to play the next episode.
pub fn current(state: &crate::AppState) -> Prefs {
    lock(state).prefs().clone()
}

pub async fn prefs_get(State(state): State<crate::AppState>) -> Json<PrefsBody> {
    Json(lock(&state).body())
}

pub async fn prefs_put(
    State(state): State<crate::AppState>,
    Json(put): Json<PrefsPut>,
) -> Result<Json<PrefsBody>, crate::ApiError> {
    match lock(&state).apply(put) {
        Ok(body) => Ok(Json(body)),
        Err(PutError::Invalid(detail)) => Err(crate::fail(StatusCode::BAD_REQUEST, detail)),
        Err(PutError::Save) => Err(crate::fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not save your player choices.",
        )),
    }
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/prefs.rs"]
mod tests;
