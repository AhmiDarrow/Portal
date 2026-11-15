//! `%LOCALAPPDATA%\Portal\metadata.json`: whether online lookups run, and the
//! user's own TMDB key. The key is stored encrypted for this Windows account
//! (DPAPI), as the subtitle keys are, and only ever read back by this module.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::subtitles::{is_protected, protect, reveal};

#[derive(Clone, PartialEq, Eq)]
pub struct Stored {
    pub online: bool,
    pub tmdb_key: Option<String>,
}

fn online_by_default() -> bool {
    true
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            online: true,
            tmdb_key: None,
        }
    }
}

/// Never prints the key.
impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("online", &self.online)
            .field("tmdb", &self.tmdb_key.is_some())
            .finish()
    }
}

/// The file as written.
#[derive(Serialize, Deserialize)]
struct File {
    #[serde(default = "online_by_default")]
    online: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tmdb_key: Option<String>,
}

/// A missing or unreadable file reads as the defaults. A key that cannot be
/// decrypted (another Windows account's file) reads as missing. A key in
/// plain text, from an earlier version or typed in by hand, is encrypted in
/// place.
pub fn load(path: &Path) -> Stored {
    let (stored, plain) = read(path);
    if plain {
        if let Err(err) = save(path, &stored) {
            tracing::debug!(error = %err, "Could not encrypt the saved TMDB key");
        }
    }
    stored
}

/// The settings, and whether the file holds the key in plain text.
fn read(path: &Path) -> (Stored, bool) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return (Stored::default(), false);
    };
    let Ok(file) = serde_json::from_str::<File>(&text) else {
        crate::paths::set_aside(path);
        return (Stored::default(), false);
    };
    let plain = file
        .tmdb_key
        .as_deref()
        .is_some_and(|key| !is_protected(key));
    let tmdb_key = file
        .tmdb_key
        .and_then(|text| reveal(&text))
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty());
    let plain = plain && tmdb_key.is_some();
    let stored = Stored {
        online: file.online,
        tmdb_key,
    };
    (stored, plain)
}

pub fn save(path: &Path, stored: &Stored) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmdb_key = stored
        .tmdb_key
        .as_deref()
        .map(|key| protect(key).ok_or_else(|| std::io::Error::other("Could not encrypt a secret")))
        .transpose()?;
    let file = File {
        online: stored.online,
        tmdb_key,
    };
    let mut body = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    body.push(b'\n');
    let part = path.with_extension("json.part");
    std::fs::write(&part, body)?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/settings.rs"]
mod tests;
