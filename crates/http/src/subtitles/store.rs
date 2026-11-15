//! What Portal keeps between requests: each source's search answers in
//! memory (hours when found, minutes when a source failed), and converted
//! WebVTT files in memory and in `%LOCALAPPDATA%\Portal\subtitles\`, both
//! bounded.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use super::rank::Found;

pub const FOUND_TTL: Duration = Duration::from_secs(6 * 60 * 60);
pub const EMPTY_TTL: Duration = Duration::from_secs(60 * 60);
pub const FAILED_TTL: Duration = Duration::from_secs(10 * 60);
/// A source's note ("asked Portal to slow down. Try again in a minute")
/// must not outlive the minute it tells the user to wait.
pub const NOTE_TTL: Duration = Duration::from_secs(60);
const SEARCH_CAPACITY: usize = 300;

/// A source's answer: what it listed, or the note explaining why it could
/// not (`None` for a plain failure).
pub type Answer = Result<Vec<Found>, Option<String>>;

struct Searched {
    at: Instant,
    ttl: Duration,
    answer: Answer,
}

#[derive(Default)]
pub struct Searches {
    entries: Mutex<HashMap<String, Searched>>,
}

impl Searches {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Searched>> {
        self.entries
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    pub fn get(&self, key: &str) -> Option<Answer> {
        self.lock()
            .get(key)
            .filter(|entry| entry.at.elapsed() < entry.ttl)
            .map(|entry| entry.answer.clone())
    }

    pub fn put(&self, key: &str, answer: Answer) {
        let ttl = ttl_for(&answer);
        let mut entries = self.lock();
        entries.retain(|_, entry| entry.at.elapsed() < entry.ttl);
        while entries.len() >= SEARCH_CAPACITY {
            let oldest = entries
                .iter()
                .min_by_key(|(_, entry)| entry.at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(oldest) => entries.remove(&oldest),
                None => break,
            };
        }
        entries.insert(
            key.to_string(),
            Searched {
                at: Instant::now(),
                ttl,
                answer,
            },
        );
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

fn ttl_for(answer: &Answer) -> Duration {
    match answer {
        Ok(found) if !found.is_empty() => FOUND_TTL,
        Ok(_) => EMPTY_TTL,
        Err(Some(_)) => NOTE_TTL,
        Err(None) => FAILED_TTL,
    }
}

const MEMORY_FILES: usize = 24;
const DISK_FILES: usize = 400;
const DISK_BYTES: u64 = 64 << 20;

pub struct Files {
    dir: Option<PathBuf>,
    memory: Mutex<VecDeque<(String, Arc<String>)>>,
}

impl Files {
    /// `None` keeps files in memory only.
    pub fn open(dir: Option<&Path>) -> Self {
        Self {
            dir: dir.map(Path::to_path_buf),
            memory: Mutex::new(VecDeque::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<(String, Arc<String>)>> {
        self.memory
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn path(&self, id: &str) -> Option<PathBuf> {
        let digest = Sha256::digest(id.as_bytes());
        let name: String = digest
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.dir.as_ref().map(|dir| dir.join(format!("{name}.vtt")))
    }

    pub fn get(&self, id: &str) -> Option<Arc<String>> {
        {
            let mut memory = self.lock();
            if let Some(index) = memory.iter().position(|(key, _)| key == id) {
                let entry = memory.remove(index)?;
                let text = entry.1.clone();
                memory.push_back(entry);
                return Some(text);
            }
        }
        let path = self.path(id)?;
        let text = std::fs::read_to_string(&path).ok()?;
        if !text.starts_with("WEBVTT") {
            return None;
        }
        let text = Arc::new(text);
        self.remember(id, text.clone());
        Some(text)
    }

    fn remember(&self, id: &str, text: Arc<String>) {
        let mut memory = self.lock();
        memory.retain(|(key, _)| key != id);
        memory.push_back((id.to_string(), text));
        while memory.len() > MEMORY_FILES {
            memory.pop_front();
        }
    }

    pub fn put(&self, id: &str, text: Arc<String>) {
        self.remember(id, text.clone());
        let (Some(dir), Some(path)) = (&self.dir, self.path(id)) else {
            return;
        };
        let written = std::fs::create_dir_all(dir).and_then(|()| {
            let part = path.with_extension("vtt.part");
            std::fs::write(&part, text.as_bytes()).and_then(|()| std::fs::rename(&part, &path))
        });
        if let Err(err) = written {
            tracing::debug!(error = %err, "Could not keep a subtitle file");
            return;
        }
        prune(dir, DISK_FILES, DISK_BYTES);
    }
}

/// Oldest files first, until at most `files` files and `bytes` bytes remain.
fn prune(dir: &Path, files: usize, bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(std::time::SystemTime, u64, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("vtt") {
                return None;
            }
            let meta = entry.metadata().ok()?;
            Some((meta.modified().ok()?, meta.len(), path))
        })
        .collect();
    kept.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let mut total = 0u64;
    for (index, (_, size, path)) in kept.iter().enumerate() {
        total += size;
        if index >= files || total > bytes {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/store.rs"]
mod tests;
