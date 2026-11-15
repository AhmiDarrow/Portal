//! What the sources found, kept in memory and in
//! `%LOCALAPPDATA%\Portal\metadata-cache.json`. Entries are keyed by the best
//! identity the page sent. Found details last 7 days, a clean "not found"
//! one day, and a lookup where a source failed 10 minutes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use portal_api::{MediaInfo, MediaKind};
use serde::{Deserialize, Serialize};

use super::text::norm_title;

pub const FOUND_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const MISSING_TTL: Duration = Duration::from_secs(24 * 60 * 60);
pub const FAILED_TTL: Duration = Duration::from_secs(10 * 60);
const CAPACITY: usize = 400;

/// The cache key for a cleaned seed: kind, then an IMDb, TMDB or TheTVDB id,
/// else the normalized title and year; episodes add their numbers. `None`
/// when there is nothing to look up by.
pub fn key(seed: &MediaInfo) -> Option<String> {
    let kind = match seed.kind {
        Some(MediaKind::Movie) => "movie",
        Some(MediaKind::Tv) => "tv",
        Some(MediaKind::Episode) => "episode",
        None => "any",
    };
    let episode = seed.kind == Some(MediaKind::Episode);
    let identity = if let Some(id) = &seed.imdb_id {
        format!("imdb:{id}")
    } else if let Some(id) = seed.tmdb_id.as_ref().filter(|_| !episode) {
        format!("tmdb:{id}")
    } else if let Some(id) = &seed.tvdb_id {
        format!("tvdb:{id}")
    } else {
        let title = if episode {
            seed.show_title.as_deref()
        } else {
            seed.title.as_deref()
        };
        let title = norm_title(title?);
        if title.is_empty() {
            return None;
        }
        let year = seed.year.map(|year| year.to_string()).unwrap_or_default();
        format!("title:{title}:{year}")
    };
    let numbers = if episode {
        let number = |value: Option<u32>| value.map(|n| n.to_string()).unwrap_or_default();
        format!(":s{}e{}", number(seed.season), number(seed.episode))
    } else {
        String::new()
    };
    Some(format!("{kind}:{identity}{numbers}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// Unix seconds when it was stored.
    at: u64,
    /// Seconds it stays fresh.
    ttl: u64,
    #[serde(default)]
    found: Vec<MediaInfo>,
}

impl Entry {
    fn fresh(&self, now: u64) -> bool {
        now >= self.at && now - self.at < self.ttl
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Doc {
    #[serde(default)]
    entries: BTreeMap<String, Entry>,
    /// Counts changes in memory only.
    #[serde(skip)]
    version: u64,
}

pub struct Cache {
    path: Option<PathBuf>,
    doc: Mutex<Doc>,
    /// Keeps two writes of the file from interleaving, and holds the number
    /// of the newest snapshot written so an older one never lands after it.
    writing: Arc<Mutex<u64>>,
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

impl Cache {
    /// Loads the file, dropping stale entries. `None` keeps it in memory only.
    pub fn open(path: Option<&Path>) -> Self {
        let mut doc: Doc = path
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let now = now();
        doc.entries.retain(|_, entry| entry.fresh(now));
        Self {
            path: path.map(Path::to_path_buf),
            doc: Mutex::new(doc),
            writing: Arc::new(Mutex::new(0)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Doc> {
        self.doc.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn get(&self, key: &str, now: u64) -> Option<Vec<MediaInfo>> {
        self.lock()
            .entries
            .get(key)
            .filter(|entry| entry.fresh(now))
            .map(|entry| entry.found.clone())
    }

    pub fn put(&self, key: &str, found: Vec<MediaInfo>, ttl: Duration, now: u64) {
        let text = {
            let mut doc = self.lock();
            doc.entries.retain(|_, entry| entry.fresh(now));
            doc.entries.insert(
                key.to_string(),
                Entry {
                    at: now,
                    ttl: ttl.as_secs(),
                    found,
                },
            );
            while doc.entries.len() > CAPACITY {
                let oldest = doc
                    .entries
                    .iter()
                    .min_by_key(|(_, entry)| entry.at)
                    .map(|(key, _)| key.clone());
                match oldest {
                    Some(oldest) => doc.entries.remove(&oldest),
                    None => break,
                };
            }
            snapshot(&mut doc)
        };
        if let Some(text) = text {
            self.write(text);
        }
    }

    pub fn clear(&self) {
        let text = {
            let mut doc = self.lock();
            doc.entries.clear();
            snapshot(&mut doc)
        };
        if let Some(text) = text {
            self.write(text);
        }
    }

    /// Saves off the async threads when there is a runtime, else right away.
    fn write(&self, (version, text): (u64, String)) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let writing = Arc::clone(&self.writing);
        let job = move || save(&path, &text, version, &writing);
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(job);
            }
            Err(_) => job(),
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

/// Numbers each snapshot while the document lock is held, so the numbers
/// follow the order of the changes.
fn snapshot(doc: &mut Doc) -> Option<(u64, String)> {
    doc.version += 1;
    let text = serde_json::to_string(&*doc).ok()?;
    Some((doc.version, text))
}

fn save(path: &Path, text: &str, version: u64, writing: &Mutex<u64>) {
    let mut written = writing.lock().unwrap_or_else(|poison| poison.into_inner());
    // A newer snapshot already reached the file.
    if *written >= version {
        return;
    }
    let part = path.with_extension("json.part");
    match std::fs::write(&part, text).and_then(|()| std::fs::rename(&part, path)) {
        Ok(()) => *written = version,
        Err(err) => tracing::debug!(error = %err, "Could not save the metadata cache"),
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/metadata/cache.rs"]
mod tests;
