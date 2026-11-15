//! A copy of the saved data, taken when Portal starts on a version other than
//! the one that ran last. Installing an update never touches these files, but
//! the new version reads and rewrites them, so the copy keeps what the old one
//! left: watch history, Continue Watching, the watchlist, bookmarks and every
//! Portal setting. Accounts signed in through an add-on live in Kodi's add-on
//! data, which neither the installer nor Portal's update ever replaces.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Files in the Portal folder that hold what the user saved or chose.
const SAVED: &[&str] = &[
    "library.json",
    "prefs.json",
    "metadata.json",
    "subtitles.json",
    "cast.json",
    "packs.json",
    "devices.json",
];
/// Copies kept. The oldest goes once there are more, except that the newest
/// copy at least [`KEEP_OLD_DAYS`] old also stays, so quick releases in a row
/// cannot rotate out every copy from before a problem.
const KEEP: usize = 3;
const KEEP_OLD_DAYS: u64 = 7;
const MARKER: &str = "last-version";

/// Copies the saved files into `backups\<time>-<old version>` when `current`
/// differs from the version that ran last. The marker moves on only after
/// every copy is written, so a failed copy is tried again on the next start.
pub fn on_version_change(portal_dir: &Path, current: &str) {
    let marker = portal_dir.join(MARKER);
    let last = fs::read_to_string(&marker)
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    if last == current {
        return;
    }
    let present: Vec<&str> = SAVED
        .iter()
        .copied()
        .filter(|name| portal_dir.join(name).is_file())
        .collect();
    if !present.is_empty() {
        let from = if last.is_empty() { "earlier" } else { last.as_str() };
        let folder = backups_dir(portal_dir).join(format!("{:013}-{}", now_ms(), safe_name(from)));
        match copy_all(portal_dir, &folder, &present) {
            Ok(()) => tracing::info!(folder = %folder.display(), "Saved a copy of the library before this version"),
            Err(err) => {
                tracing::warn!("Could not copy the library before this version: {err}");
                return;
            }
        }
        prune(&backups_dir(portal_dir));
    }
    if let Err(err) = fs::write(&marker, current) {
        tracing::warn!("Could not note the running version: {err}");
    }
}

fn backups_dir(portal_dir: &Path) -> PathBuf {
    portal_dir.join("backups")
}

fn copy_all(portal_dir: &Path, folder: &Path, names: &[&str]) -> std::io::Result<()> {
    fs::create_dir_all(folder)?;
    for name in names {
        fs::copy(portal_dir.join(name), folder.join(name))?;
    }
    Ok(())
}

/// Removes the oldest copies beyond [`KEEP`]. Names start with a fixed-width
/// time, so they sort oldest first.
fn prune(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let extra = folders.len().saturating_sub(KEEP);
    let week_old = now_ms().saturating_sub(u128::from(KEEP_OLD_DAYS) * 24 * 60 * 60 * 1000);
    // Names start with the copy's time in milliseconds.
    let taken = |path: &PathBuf| -> Option<u128> {
        path.file_name()?.to_str()?.split('-').next()?.parse().ok()
    };
    let keep_old = folders
        .iter()
        .rev()
        .find(|path| taken(path).is_some_and(|at| at <= week_old))
        .cloned();
    for old in folders.into_iter().take(extra) {
        if Some(&old) != keep_old.as_ref() {
            let _ = fs::remove_dir_all(old);
        }
    }
}

fn safe_name(version: &str) -> String {
    version
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-'))
        .take(32)
        .collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/backup.rs"]
mod tests;
