//! Reads one of the old page's own localStorage items (`portal-playback`, prefs.rs).
//! `/` and `/next/` are one origin, so both pages read the same item. Theme and fonts are
//! kept with `Signal::persist` and `Persist::plain` instead (shell.rs), which also writes.

/// The item `key`, in a browser page; natively there is none.
pub fn item(key: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        remedy_ui::webstorage::browser_local_storage().ok()?.get(key).map(str::to_owned)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}
