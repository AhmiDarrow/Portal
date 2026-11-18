//! Whole-season files already chosen, so the next episode of that season
//! picks the same file from the add-on's source list instead of a new ranking.
//!
//! `%LOCALAPPDATA%\Portal\packs.json` is a small map of `show` + season to the
//! file's identity. A failed play leaves it alone. A successful
//! single-episode file clears that season's entry.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use portal_engine::{pack_identity, pack_named, row_height};

const CAP: usize = 64;
const SHOW_LIMIT: usize = 512;
const IDENTITY_LIMIT: usize = 180;

struct Noted {
    key: String,
    row: String,
}

pub struct SeasonPacks {
    path: PathBuf,
    saved: HashMap<String, String>,
    order: VecDeque<String>,
    /// Job id to show+season, newest last.
    open: VecDeque<String>,
    keys: HashMap<String, String>,
    noted: HashMap<String, Noted>,
}

impl SeasonPacks {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("packs.json");
        let (saved, order) = load(&path);
        Self {
            path,
            saved,
            order,
            open: VecDeque::new(),
            keys: HashMap::new(),
            noted: HashMap::new(),
        }
    }

    pub fn begin(&mut self, job: &str, show: &str, season: u32) {
        if show.is_empty() || job.is_empty() {
            return;
        }
        let key = pack_key(show, season);
        self.open.retain(|id| id != job);
        self.open.push_back(job.to_string());
        self.keys.insert(job.to_string(), key);
    }

    pub fn preferred(&self, show: &str, season: u32) -> Option<String> {
        self.saved.get(&pack_key(show, season)).cloned()
    }

    /// The row Portal itself answered for this play. A dialog that is not a
    /// source list, such as a language picker, is ignored.
    pub fn note(&mut self, job: &str, row: &str) {
        if row_height(row).is_none() && pack_identity(row).is_none() {
            return;
        }
        let Some(key) = self.keys.get(job).cloned() else {
            return;
        };
        self.noted.insert(
            job.to_string(),
            Noted {
                key,
                row: row.to_string(),
            },
        );
    }

    /// The row the viewer submitted, on the newest play still resolving.
    pub fn note_latest(&mut self, row: &str, live: impl Fn(&str) -> bool) {
        let Some(job) = self.open.iter().rev().find(|id| live(id)).cloned() else {
            return;
        };
        self.note(&job, row);
    }

    /// `played` is true when the job reached a stream. Anything else drops the
    /// note and leaves the saved file as it was.
    pub fn settle(&mut self, job: &str, played: bool) {
        self.open.retain(|id| id != job);
        self.keys.remove(job);
        let Some(noted) = self.noted.remove(job) else {
            return;
        };
        if !played {
            return;
        }
        match pack_identity(&noted.row) {
            Some(id) => self.remember(noted.key, id),
            // The remembered file, in a row laid out so its name reads
            // differently: still that file, so it stays remembered.
            None if self
                .saved
                .get(&noted.key)
                .is_some_and(|id| pack_named(&noted.row, id)) => {}
            None => self.forget(&noted.key),
        }
    }

    fn remember(&mut self, key: String, id: String) {
        if self.saved.get(&key) == Some(&id) {
            return;
        }
        self.saved.insert(key.clone(), id);
        self.order.retain(|item| item != &key);
        self.order.push_back(key);
        while self.order.len() > CAP {
            if let Some(old) = self.order.pop_front() {
                self.saved.remove(&old);
            }
        }
        self.write();
        tracing::info!("remembered the season file for later episodes");
    }

    fn forget(&mut self, key: &str) {
        if self.saved.remove(key).is_none() {
            return;
        }
        self.order.retain(|item| item != key);
        self.write();
        tracing::info!("a single episode file played, so the next episode picks a source again");
    }

    fn write(&self) {
        if let Err(err) = save(&self.path, &self.order, &self.saved) {
            tracing::warn!("season files could not be saved: {err}");
        }
    }

}

fn pack_key(show: &str, season: u32) -> String {
    format!("{}\t{season}", show_key(show))
}

/// A short key that stays the same for every episode of one show. An add-on's
/// show id is often a whole `plugin://` link carrying art and metadata, far
/// longer than a saved key and liable to change between listings. Its IMDb,
/// TMDb or TVDb id is used when it names one; any other long id is hashed.
fn show_key(show: &str) -> String {
    let show = show.trim();
    for (name, prefix) in [("imdb", "tt"), ("tmdb", ""), ("tvdb", "")] {
        let Some(id) = link_param(show, name) else {
            continue;
        };
        let Some(digits) = id.strip_prefix(prefix) else {
            continue;
        };
        if !digits.is_empty() && digits.len() <= 12 && digits.bytes().all(|b| b.is_ascii_digit()) {
            return format!("{name}:{id}");
        }
    }
    if show.len() <= SHOW_LIMIT && !show.contains(['\n', '\r']) {
        return show.to_string();
    }
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(show.as_bytes());
    let hex: String = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("id:{hex}")
}

/// A query parameter of a link, as written. The ids this reads are plain
/// ASCII, so the value is not percent-decoded.
fn link_param<'a>(link: &'a str, name: &str) -> Option<&'a str> {
    let (_, query) = link.split_once('?')?;
    let query = query.split('#').next().unwrap_or(query);
    query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        let value = value.trim();
        (key.eq_ignore_ascii_case(name) && !value.is_empty()).then_some(value)
    })
}

fn valid_entry(key: &str, id: &str) -> bool {
    let Some((show, season)) = key.rsplit_once('\t') else {
        return false;
    };
    !show.is_empty()
        && show.len() <= SHOW_LIMIT
        && !show.contains(['\n', '\r'])
        && season.parse::<u32>().is_ok()
        && (8..=IDENTITY_LIMIT).contains(&id.len())
        && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '.')
}

fn load(path: &Path) -> (HashMap<String, String>, VecDeque<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (HashMap::new(), VecDeque::new());
        }
        Err(err) => {
            tracing::warn!("season files could not be read: {err}");
            return (HashMap::new(), VecDeque::new());
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!("season files could not be read: {err}");
            return (HashMap::new(), VecDeque::new());
        }
    };
    let Some(map) = value.as_object() else {
        tracing::warn!("season files could not be read");
        return (HashMap::new(), VecDeque::new());
    };
    let mut saved = HashMap::new();
    let mut order = VecDeque::new();
    for (key, item) in map {
        let Some(id) = item.as_str() else {
            continue;
        };
        if !valid_entry(key, id) || saved.contains_key(key) {
            continue;
        }
        saved.insert(key.clone(), id.to_string());
        order.push_back(key.clone());
    }
    while order.len() > CAP {
        if let Some(old) = order.pop_front() {
            saved.remove(&old);
        }
    }
    (saved, order)
}

fn save(
    path: &Path,
    order: &VecDeque<String>,
    saved: &HashMap<String, String>,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut map = serde_json::Map::new();
    for key in order {
        if let Some(id) = saved.get(key) {
            map.insert(key.clone(), serde_json::Value::String(id.clone()));
        }
    }
    let mut body = serde_json::to_vec_pretty(&map).map_err(std::io::Error::other)?;
    body.push(b'\n');
    let part = path.with_extension("json.part");
    std::fs::write(&part, &body)?;
    std::fs::rename(&part, path)
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/packs.rs"]
mod tests;
