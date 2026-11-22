//! Message boxes and opening things with the shell.

#[cfg(windows)]
pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Opens a URL in the default browser, or a folder in Explorer.
#[cfg(windows)]
pub fn open(target: &str) {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let verb = wide("open");
    let target = wide(target);
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        );
    }
}

#[cfg(not(windows))]
pub fn open(target: &str) {
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

#[cfg(windows)]
fn message_box(message: &str, style: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_SETFOREGROUND, MB_TOPMOST};
    let title = wide("Portal");
    let message = wide(message);
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            style | MB_SETFOREGROUND | MB_TOPMOST,
        )
    }
}

#[cfg(windows)]
pub fn alert(message: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK};
    message_box(message, MB_OK | MB_ICONINFORMATION);
}

#[cfg(not(windows))]
pub fn alert(message: &str) {
    eprintln!("Portal: {message}");
}

/// A Yes/No warning where No is the default. True when the user picks Yes.
#[cfg(windows)]
pub fn confirm(message: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
    };
    message_box(message, MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2) == IDYES
}
