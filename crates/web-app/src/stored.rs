//! The page's own localStorage items the old UI wrote (`portal-theme`, `portal-fonts`).
//! `/` and `/next/` are one origin, so both pages read the same choice.
//!
//! ENGINE GAP: RemedyUI reads these (`webstorage::browser_local_storage`) but has no way to
//! write a raw item: `Signal::persist` keeps its own versioned JSON under
//! `remedy:<app>:<key>` (remedy-ui-reactive/src/persist.rs, `Persist`), and
//! remedy-ui-services-webstorage is read-only (src/browser.rs). Until it can, choices made
//! in this page apply until it reloads.

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
