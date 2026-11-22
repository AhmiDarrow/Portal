//! Where Portal finds its files. An environment variable wins, then the
//! installed layout beside `portal.exe`, then the source tree (for `cargo run`),
//! then the working directory or PATH.
//!
//! Installed layout:
//!
//! ```text
//! portal.exe
//! web\index.html          the built dashboard
//! vendor\hls.min.js
//! ffmpeg\ffmpeg.exe       ffmpeg\ffprobe.exe
//! cast\cast_agent.py      python\python.exe
//! ```

use std::path::{Path, PathBuf};

/// Directory that holds the running `portal.exe`.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.parent().map(Path::to_path_buf)
}

/// Repository root when running from a checkout. Compiled in, so only used
/// after the path is confirmed to exist.
fn source_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

pub fn env_path(key: &str) -> Option<PathBuf> {
    let value = std::env::var_os(key)?;
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

/// First candidate that passes `exists`, else `fallback`.
fn first(candidates: Vec<PathBuf>, exists: fn(&Path) -> bool, fallback: PathBuf) -> PathBuf {
    candidates
        .into_iter()
        .find(|path| exists(path))
        .unwrap_or(fallback)
}

/// Settings files that are there but could not be read or set aside. Saving
/// over one would replace what it holds with defaults, so `write_settings`
/// refuses until Portal restarts.
static HELD: std::sync::Mutex<Vec<PathBuf>> = std::sync::Mutex::new(Vec::new());

fn hold(path: &Path) {
    let mut held = HELD.lock().unwrap_or_else(|err| err.into_inner());
    if !held.iter().any(|item| item == path) {
        held.push(path.to_path_buf());
    }
}

/// Reads a settings file. `None` when it is not there, or when it is there
/// but still cannot be read after a few tries (antivirus or a backup holding
/// it): Portal runs on defaults and never saves over it.
pub fn read_settings(path: &Path) -> Option<String> {
    let mut tries = 0;
    loop {
        match std::fs::read_to_string(path) {
            Ok(text) => return Some(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return None,
            Err(_) if tries < 4 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                tracing::error!(
                    "{} could not be read ({err}); Portal will not save over it until it restarts",
                    path.display()
                );
                hold(path);
                return None;
            }
        }
    }
}

/// Writes a settings file beside itself and swaps it in, so a crash never
/// leaves half a file. Refused for a file `read_settings` could not read.
pub fn write_settings(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let held = HELD.lock().unwrap_or_else(|err| err.into_inner());
    if held.iter().any(|item| item == path) {
        return Err(std::io::Error::other(format!(
            "{} could not be read when Portal started, so it is not saved over",
            path.display()
        )));
    }
    drop(held);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let part = path.with_extension("json.part");
    let mut file = std::fs::File::create(&part)?;
    file.write_all(body)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&part, path)
}

/// Moves a settings file that no longer parses to `<name>.corrupt-<ms>.json`
/// beside it, so the next save does not overwrite what it held.
pub fn set_aside(path: &std::path::Path) {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("settings");
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_millis())
        .unwrap_or(0);
    let aside = path.with_file_name(format!("{stem}.corrupt-{ms}.json"));
    match std::fs::rename(path, &aside) {
        Ok(()) => tracing::warn!("{} did not parse; kept it as {}", path.display(), aside.display()),
        Err(err) => {
            tracing::error!("{} did not parse and could not be kept aside: {err}", path.display());
            hold(path);
        }
    }
}

pub fn portal_dir() -> PathBuf {
    env_path("LOCALAPPDATA")
        .unwrap_or_else(std::env::temp_dir)
        .join("Portal")
}

pub fn web_dist() -> PathBuf {
    if let Some(dir) = env_path("PORTAL_WEB_DIST") {
        return dir;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = install_dir() {
        candidates.push(dir.join("web"));
    }
    candidates.push(source_root().join("web").join("dist"));
    first(
        candidates,
        |dir| dir.join("index.html").is_file(),
        PathBuf::from("web/dist"),
    )
}

/// The RemedyUI build of the dashboard, served at `/next/`: `PORTAL_WEB_NEXT_DIST`, else
/// `web-next` beside the program, else what `tools/build-web-app.ps1` writes in a checkout.
pub fn web_next_dist() -> PathBuf {
    if let Some(dir) = env_path("PORTAL_WEB_NEXT_DIST") {
        return dir;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = install_dir() {
        candidates.push(dir.join("web-next"));
    }
    candidates.push(source_root().join("target").join("remedy").join("web").join("portal-web"));
    first(
        candidates,
        |dir| dir.join("index.html").is_file(),
        PathBuf::from("web-next"),
    )
}

pub fn vendor_hls() -> PathBuf {
    if let Some(path) = env_path("PORTAL_HLS_JS") {
        return path;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = install_dir() {
        candidates.push(dir.join("vendor").join("hls.min.js"));
    }
    candidates.push(source_root().join("web").join("vendor").join("hls.min.js"));
    first(
        candidates,
        |path| path.is_file(),
        PathBuf::from("web/vendor/hls.min.js"),
    )
}

/// A bundled tool (`ffmpeg`, `ffprobe`) beside the install, else PATH.
fn bundled_tool(env_key: &str, name: &str) -> PathBuf {
    if let Some(path) = env_path(env_key) {
        return path;
    }
    if let Some(dir) = install_dir() {
        let path = dir.join("ffmpeg").join(format!("{name}.exe"));
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from(name)
}

pub fn ffmpeg() -> PathBuf {
    bundled_tool("FFMPEG", "ffmpeg")
}

pub fn ffprobe() -> PathBuf {
    bundled_tool("FFPROBE", "ffprobe")
}

pub fn cast_agent_script() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env_path("PORTAL_CAST_AGENT") {
        candidates.push(path);
    }
    if let Some(dir) = install_dir() {
        candidates.push(dir.join("cast").join("cast_agent.py"));
    }
    candidates.push(
        source_root()
            .join("tools")
            .join("cast")
            .join("cast_agent.py"),
    );
    candidates.push(PathBuf::from("tools/cast/cast_agent.py"));
    candidates.into_iter().find(|path| path.is_file())
}

pub fn cast_python() -> PathBuf {
    if let Some(path) = env_path("PORTAL_CAST_PYTHON") {
        return path;
    }
    if let Some(dir) = install_dir() {
        let python = dir.join("python").join("python.exe");
        if python.is_file() {
            return python;
        }
    }
    PathBuf::from("python")
}

/// `KODI_EXE`, then the folder Kodi's installer records under
/// `HKCU\Software\Kodi`, then the Program Files folders.
pub fn kodi_exe() -> PathBuf {
    if let Some(path) = env_path("KODI_EXE") {
        return path;
    }
    let mut candidates = Vec::new();
    if let Some(dir) = kodi_registry_dir() {
        candidates.push(dir.join("kodi.exe"));
    }
    for key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(root) = env_path(key) {
            candidates.push(root.join("Kodi").join("kodi.exe"));
        }
    }
    first(
        candidates,
        |path| path.is_file(),
        PathBuf::from(r"C:\Program Files\Kodi\kodi.exe"),
    )
}

#[cfg(windows)]
fn kodi_registry_dir() -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ,
    };

    let subkey: Vec<u16> = "Software\\Kodi\0".encode_utf16().collect();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut buffer = vec![0u16; 1024];
        let mut bytes = (buffer.len() * 2) as u32;
        // SAFETY: the subkey is NUL-terminated, a null value name reads the
        // default value, and `bytes` is the buffer's size in bytes.
        let status = unsafe {
            RegGetValueW(
                root,
                subkey.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            continue;
        }
        let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
        if len > 0 {
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buffer[..len])));
        }
    }
    None
}

#[cfg(not(windows))]
fn kodi_registry_dir() -> Option<PathBuf> {
    None
}

#[cfg(test)]
#[path = "../../../tests-private/http/src/paths.rs"]
mod tests;
