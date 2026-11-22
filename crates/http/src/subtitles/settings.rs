//! `%LOCALAPPDATA%\Portal\subtitles.json`: preferred languages, whether to
//! prefer hearing-impaired subtitles, the user's own OpenSubtitles API key,
//! username and password, and their SubDL and SubSource API keys. The keys
//! and password are stored encrypted for this Windows account (DPAPI), only
//! ever read back by this module, and never printed.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, PartialEq, Eq)]
pub struct Stored {
    pub languages: Vec<String>,
    pub hearing_impaired: bool,
    pub opensubtitles_key: Option<String>,
    pub opensubtitles_username: Option<String>,
    pub opensubtitles_password: Option<String>,
    pub subdl_key: Option<String>,
    pub subsource_key: Option<String>,
}

impl Stored {
    pub fn has_login(&self) -> bool {
        self.opensubtitles_username.is_some() && self.opensubtitles_password.is_some()
    }
}

/// Never prints the keys, the username or the password.
impl std::fmt::Debug for Stored {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stored")
            .field("languages", &self.languages)
            .field("hearing_impaired", &self.hearing_impaired)
            .field("opensubtitles", &self.opensubtitles_key.is_some())
            .field("opensubtitles_user", &self.has_login())
            .field("subdl", &self.subdl_key.is_some())
            .field("subsource", &self.subsource_key.is_some())
            .finish()
    }
}

/// The file as written.
#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    languages: Vec<String>,
    #[serde(default)]
    hearing_impaired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opensubtitles_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opensubtitles_username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    opensubtitles_password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subdl_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subsource_key: Option<String>,
}

fn clean(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// A missing or unreadable file reads as the defaults; an unreadable one is
/// not saved over. A secret that cannot
/// be decrypted (another Windows account's file) reads as missing.
pub fn load(path: &Path) -> Stored {
    let Some(text) = crate::paths::read_settings(path) else {
        return Stored::default();
    };
    let file: File = serde_json::from_str(&text).unwrap_or_else(|_| {
        crate::paths::set_aside(path);
        File::default()
    });
    Stored {
        languages: super::langs::normalize(&file.languages),
        hearing_impaired: file.hearing_impaired,
        opensubtitles_key: clean(file.opensubtitles_key.and_then(|text| reveal(&text))),
        opensubtitles_username: clean(file.opensubtitles_username),
        opensubtitles_password: file
            .opensubtitles_password
            .and_then(|text| reveal(&text))
            .filter(|password| !password.is_empty()),
        subdl_key: clean(file.subdl_key.and_then(|text| reveal(&text))),
        subsource_key: clean(file.subsource_key.and_then(|text| reveal(&text))),
    }
}

pub fn save(path: &Path, stored: &Stored) -> std::io::Result<()> {
    let conceal = |secret: &Option<String>| -> std::io::Result<Option<String>> {
        secret
            .as_deref()
            .map(|secret| {
                protect(secret).ok_or_else(|| std::io::Error::other("Could not encrypt a secret"))
            })
            .transpose()
    };
    let file = File {
        languages: stored.languages.clone(),
        hearing_impaired: stored.hearing_impaired,
        opensubtitles_key: conceal(&stored.opensubtitles_key)?,
        opensubtitles_username: stored.opensubtitles_username.clone(),
        opensubtitles_password: conceal(&stored.opensubtitles_password)?,
        subdl_key: conceal(&stored.subdl_key)?,
        subsource_key: conceal(&stored.subsource_key)?,
    };
    let mut body = serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?;
    body.push(b'\n');
    crate::paths::write_settings(path, &body)
}

/// The prefix of a sealed value: DPAPI on Windows, `sealed.rs` on Linux.
#[cfg(windows)]
pub(crate) const PROTECTED: &str = "dpapi:";
#[cfg(not(windows))]
pub(crate) const PROTECTED: &str = "sealed:";

#[cfg(not(windows))]
#[path = "sealed.rs"]
mod dpapi;

/// `dpapi:<hex>`: the secret encrypted for this Windows account (on Linux,
/// `sealed:<hex>`, see `sealed.rs`). The metadata settings keep the TMDB key
/// this way too.
pub(crate) fn protect(secret: &str) -> Option<String> {
    let sealed = dpapi::seal(secret.as_bytes())?;
    let mut out = String::from(PROTECTED);
    for byte in sealed {
        out.push_str(&format!("{byte:02x}"));
    }
    Some(out)
}

/// Whether a value from the file was written by `protect`.
pub(crate) fn is_protected(text: &str) -> bool {
    text.starts_with(PROTECTED)
}

/// A value from the file. One without the prefix was typed in by hand.
pub(crate) fn reveal(text: &str) -> Option<String> {
    let Some(hex) = text.strip_prefix(PROTECTED) else {
        return Some(text.to_string());
    };
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect();
    let opened = dpapi::open(&bytes?)?;
    String::from_utf8(opened).ok()
}

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn run(
        data: &[u8],
        call: impl FnOnce(*const CRYPT_INTEGER_BLOB, *mut CRYPT_INTEGER_BLOB) -> i32,
    ) -> Option<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(data.len()).ok()?,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        if call(&input, &mut output) == 0 || output.pbData.is_null() {
            return None;
        }
        // SAFETY: on success the API returns a buffer of `cbData` bytes that
        // the caller frees with LocalFree, once, after copying it.
        let bytes =
            unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
        unsafe {
            LocalFree(output.pbData as _);
        }
        Some(bytes)
    }

    pub fn seal(data: &[u8]) -> Option<Vec<u8>> {
        run(data, |input, output| unsafe {
            // SAFETY: both blobs outlive the call; no prompt, no entropy.
            CryptProtectData(
                input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        })
    }

    pub fn open(data: &[u8]) -> Option<Vec<u8>> {
        run(data, |input, output| unsafe {
            // SAFETY: as in `seal`; the description is not asked for.
            CryptUnprotectData(
                input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                output,
            )
        })
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/settings.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests-private/http/src/subtitles/settings_safety.rs"]
mod safety_tests;
