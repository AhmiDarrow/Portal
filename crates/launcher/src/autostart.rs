//! "Start with Windows" (and with the session on Linux): on Windows the `Portal` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, which the installer's "Start Portal
//! in the background when I sign in" option writes too; on Linux an XDG autostart entry
//! (`~/.config/autostart/io.github.ahmidarrow.portal.desktop`). Either way it starts the
//! tray without opening a browser.

use remedy_ui::desktop::autostart::{self, Entry};

/// The name the Run value and the autostart entry show.
pub const NAME: &str = "Portal";
/// The autostart entry's file name on Linux.
pub const ID: &str = "io.github.ahmidarrow.portal";
/// What the tray is started with at sign-in.
pub const ARGS: &[&str] = &["--no-browser"];

/// This launcher, started at sign-in from the Run key (not a Startup-folder shortcut), as the
/// installer sets it.
pub fn entry() -> Result<Entry, String> {
    Entry::current(NAME, ID, ARGS)
        .map(|e| e.windows_run_key(true))
        .map_err(|err| format!("Portal could not find itself: {err}"))
}

pub fn enabled() -> bool {
    entry().is_ok_and(|e| autostart::enabled(&e))
}

pub fn set(on: bool) -> Result<(), String> {
    autostart::set(&entry()?, on)
        .map_err(|err| format!("Portal could not change its sign-in setting ({err})."))
}

#[cfg(test)]
#[path = "../../../tests-private/launcher/src/autostart.rs"]
mod tests;
