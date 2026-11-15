//! "Start with Windows": the `Portal` value under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. The installer's
//! "Start Portal in the background when I sign in" option writes the same value.

use std::path::Path;

pub const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const VALUE_NAME: &str = "Portal";

/// The command Windows runs at sign-in: the tray, without opening a browser.
pub fn run_command(exe: &Path) -> String {
    format!("\"{}\" --no-browser", exe.display())
}

#[cfg(windows)]
mod imp {
    use super::{run_command, RUN_KEY, VALUE_NAME};
    use crate::ui::wide;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    pub fn enabled() -> bool {
        let key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        let mut size = 0u32;
        // SAFETY: the strings are NUL-terminated and outlive the call; a null
        // data pointer asks only for the size.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        status == ERROR_SUCCESS
    }

    pub fn set(on: bool) -> Result<(), String> {
        let key = wide(RUN_KEY);
        let name = wide(VALUE_NAME);
        let status = if on {
            let exe = std::env::current_exe()
                .map_err(|err| format!("Portal could not find itself: {err}"))?;
            let data = wide(&run_command(&exe));
            // SAFETY: the strings are NUL-terminated and outlive the call; the
            // size is the data's length in bytes, including the NUL.
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            }
        } else {
            // SAFETY: the strings are NUL-terminated and outlive the call.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => ERROR_SUCCESS,
                other => other,
            }
        };
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(format!(
                "Portal could not change its sign-in setting (error {status})."
            ))
        }
    }
}

#[cfg(windows)]
pub use imp::{enabled, set};

#[cfg(test)]
#[path = "../../../tests-private/launcher/src/autostart.rs"]
mod tests;
