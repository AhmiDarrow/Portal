//! Talking to the Portal service (`portal.exe`): finding it, starting it,
//! stopping it, and the few JSON calls the tray makes. Everything here
//! blocks, so the tray only calls it from worker threads.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const DEFAULT_PORT: u16 = 8490;
const START_WAIT: Duration = Duration::from_secs(45);
const STOP_WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(200);
/// Replies from the status routes are immediate.
const QUICK: Duration = Duration::from_secs(3);
/// Restarting Kodi waits for it to come back up.
const ENGINE_WAIT: Duration = Duration::from_secs(90);
/// An update check asks GitHub.
const CHECK_WAIT: Duration = Duration::from_secs(60);
/// Starting an update may download the installer before it answers.
const INSTALL_WAIT: Duration = Duration::from_secs(600);
/// Routes that change the service itself want this header. A web page cannot
/// send it without a CORS preflight, which the service never approves.
const LAUNCHER_HEADER: &str = "X-Portal-Launcher: 1";

pub fn port() -> u16 {
    std::env::var("PORTAL_PORT")
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .filter(|port| *port > 0)
        .unwrap_or(DEFAULT_PORT)
}

pub fn dashboard_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/")
}

pub fn server_page_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/#/server")
}

pub fn port_taken_message(port: u16) -> String {
    format!(
        "Another program is using port {port}, so Portal cannot start.\n\n\
         Close that program, or set the PORTAL_PORT environment variable to a free port."
    )
}

/// What is listening on the port, if anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    Portal,
    Other,
    /// Something accepted the connection but did not reply in time. One
    /// silent check is a busy Portal; many in a row are a hung one (see
    /// `Model::on_health`).
    Silent,
    Nothing,
}

pub fn probe(port: u16) -> Probe {
    probe_pid(port).0
}

/// The probe, and the pid a Portal's ping named.
fn probe_pid(port: u16) -> (Probe, Option<u32>) {
    match ask_ping(port) {
        Ok(Some(response)) if is_portal_ping(&response) => (Probe::Portal, ping_pid(&response)),
        Ok(Some(_)) => (Probe::Other, None),
        Ok(None) => (Probe::Nothing, None),
        Err(err) if timed_out(&err) => (Probe::Silent, None),
        Err(_) => (Probe::Nothing, None),
    }
}

/// Asks `/api/ping`. `Ok(None)` when nothing accepted the connection.
fn ask_ping(port: u16) -> std::io::Result<Option<Response>> {
    // Windows retries a refused loopback connection for about two seconds,
    // so a connect that times out means nothing is listening.
    let Ok(stream) = connect(port) else {
        return Ok(None);
    };
    exchange(stream, port, "GET", "/api/ping", &[], None, QUICK).map(Some)
}

fn timed_out(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn is_portal_ping(response: &Response) -> bool {
    response.status == 200
        && response
            .json()
            .is_some_and(|body| body["app"].as_str() == Some("portal"))
}

/// A parsed HTTP reply.
#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

impl Response {
    fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn json(&self) -> Option<Value> {
        serde_json::from_str(&self.body).ok()
    }

    /// The service's `{"detail": ...}` message, or a generic one.
    fn problem(&self) -> String {
        self.json()
            .and_then(|body| body["detail"].as_str().map(str::to_string))
            .unwrap_or_else(|| format!("Portal answered with HTTP {}.", self.status))
    }
}

fn parse_response(raw: &str) -> Option<Response> {
    let (head, body) = raw.split_once("\r\n\r\n")?;
    let status = head
        .lines()
        .next()?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(Response {
        status,
        body: body.to_string(),
    })
}

/// One small HTTP/1.1 exchange over loopback. Portal's replies here are tiny.
fn request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[&str],
    json: Option<&str>,
    timeout: Duration,
) -> std::io::Result<Response> {
    exchange(connect(port)?, port, method, path, headers, json, timeout)
}

fn connect(port: u16) -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(600))
}

fn exchange(
    mut stream: TcpStream,
    port: u16,
    method: &str,
    path: &str,
    headers: &[&str],
    json: Option<&str>,
    timeout: Duration,
) -> std::io::Result<Response> {
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(QUICK))?;
    stream.write_all(request_text(port, method, path, headers, json).as_bytes())?;
    let mut raw = Vec::new();
    stream.take(256 * 1024).read_to_end(&mut raw)?;
    parse_response(&String::from_utf8_lossy(&raw))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "not an HTTP reply"))
}

fn request_text(
    port: u16,
    method: &str,
    path: &str,
    headers: &[&str],
    json: Option<&str>,
) -> String {
    let body = json.unwrap_or("");
    let mut text = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    if json.is_some() {
        text.push_str("Content-Type: application/json\r\n");
    }
    for header in headers {
        text.push_str(header);
        text.push_str("\r\n");
    }
    text.push_str("\r\n");
    text.push_str(body);
    text
}

/// A request whose reply must be 2xx; otherwise the service's message.
fn call(
    port: u16,
    method: &str,
    path: &str,
    headers: &[&str],
    json: Option<&str>,
    timeout: Duration,
) -> Result<Response, String> {
    let response = request(port, method, path, headers, json, timeout)
        .map_err(|err| format!("Portal did not answer: {err}"))?;
    if response.ok() {
        Ok(response)
    } else {
        Err(response.problem())
    }
}

/// What the tray shows about the service, read every few seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub probe: Probe,
    /// The service's process, from `/api/ping`.
    pub pid: Option<u32>,
    /// Kodi engine state from `/api/status`: running, starting or stopped.
    pub engine: Option<String>,
    /// Whether TVs on the network may connect, from `/api/cast`.
    pub lan: Option<bool>,
    /// What encodes video, from `/api/app/encoder`.
    pub encoder: Option<EncoderChoice>,
    pub update: Option<Update>,
}

/// Whether a graphics card may encode video, and which card the PC has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderChoice {
    /// Whether a graphics card may be used, as the user last chose.
    pub hardware: bool,
    /// nvidia, intel or amd, or cpu when the PC has no card that works.
    pub card: String,
}

/// The service's update state, from `/api/update`.
#[derive(Clone, Debug, PartialEq)]
pub struct Update {
    pub current: String,
    pub latest: Option<String>,
    pub available: bool,
    /// idle, checking, downloading, ready, installing or failed.
    pub state: String,
    pub progress: Option<f64>,
    pub error: Option<String>,
}

pub fn health(port: u16) -> Health {
    let (probe, pid) = probe_pid(port);
    if probe != Probe::Portal {
        return Health {
            probe,
            pid,
            engine: None,
            lan: None,
            encoder: None,
            update: None,
        };
    }
    let read = |path: &str| {
        request(port, "GET", path, &[], None, QUICK)
            .ok()
            .filter(Response::ok)
            .and_then(|response| response.json())
    };
    Health {
        probe,
        pid,
        engine: read("/api/status").and_then(|body| engine_state(&body)),
        lan: read("/api/cast").and_then(|body| body["lan"].as_bool()),
        encoder: read("/api/app/encoder").and_then(|body| parse_encoder(&body)),
        update: read("/api/update").and_then(|body| parse_update(&body)),
    }
}

fn engine_state(status: &Value) -> Option<String> {
    status["state"].as_str().map(str::to_string)
}

fn parse_update(body: &Value) -> Option<Update> {
    Some(Update {
        current: body["current"].as_str()?.to_string(),
        latest: body["latest"].as_str().map(str::to_string),
        available: body["available"].as_bool().unwrap_or(false),
        state: body["state"].as_str().unwrap_or("idle").to_string(),
        progress: body["progress"].as_f64(),
        error: body["error"].as_str().map(str::to_string),
    })
}

/// The reply to changing network access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanReply {
    pub lan: bool,
    /// The service listens on the old address until it restarts.
    pub restart_needed: bool,
}

fn parse_lan_reply(body: &Value) -> Option<LanReply> {
    Some(LanReply {
        lan: body["lan"].as_bool()?,
        restart_needed: body["restart_needed"].as_bool().unwrap_or(false),
    })
}

pub fn set_lan(port: u16, lan: bool) -> Result<LanReply, String> {
    let body = serde_json::json!({ "lan": lan }).to_string();
    let response = call(port, "PUT", "/api/cast", &[], Some(&body), QUICK)?;
    response
        .json()
        .and_then(|body| parse_lan_reply(&body))
        .ok_or_else(|| "Portal sent an unexpected reply.".to_string())
}

fn parse_encoder(body: &Value) -> Option<EncoderChoice> {
    Some(EncoderChoice {
        hardware: body["hardware"].as_bool()?,
        card: body["card"].as_str()?.to_string(),
    })
}

/// Lets a graphics card encode video, or keeps it on the processor.
pub fn set_encoder(port: u16, hardware: bool) -> Result<EncoderChoice, String> {
    let body = serde_json::json!({ "on": hardware }).to_string();
    let response = call(port, "POST", "/api/app/encoder", &[], Some(&body), QUICK)?;
    response
        .json()
        .and_then(|body| parse_encoder(&body["encoder"]))
        .ok_or_else(|| "Portal sent an unexpected reply.".to_string())
}

pub fn restart_engine(port: u16) -> Result<(), String> {
    call(port, "POST", "/api/engine/restart", &[], None, ENGINE_WAIT).map(|_| ())
}

/// Asks the service to look for a release now and waits for the answer.
pub fn check_update(port: u16) -> Result<Update, String> {
    let response = call(port, "POST", "/api/update/check", &[], None, CHECK_WAIT)?;
    let mut update = response
        .json()
        .and_then(|body| parse_update(&body))
        .ok_or_else(|| "Portal sent an unexpected reply.".to_string())?;
    let deadline = Instant::now() + CHECK_WAIT;
    while update.state == "checking" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(500));
        let response = call(port, "GET", "/api/update", &[], None, QUICK)?;
        if let Some(next) = response.json().and_then(|body| parse_update(&body)) {
            update = next;
        }
    }
    Ok(update)
}

/// Asks the service to download and run the new installer. The installer
/// then stops the tray and the service itself.
pub fn install_update(port: u16) -> Result<(), String> {
    install_update_within(port, INSTALL_WAIT)
}

/// A slow download can outlast `wait` and still finish and install, so a
/// reply that does not come in time is not a failure: the health poll reads
/// the service's update state and reports a failure from there.
fn install_update_within(port: u16, wait: Duration) -> Result<(), String> {
    // Only a reply that is late counts as still working, never a connect
    // that timed out (on Windows that means nothing is listening).
    let stream = connect(port).map_err(|err| format!("Portal did not answer: {err}"))?;
    let path = "/api/update/install";
    match exchange(stream, port, "POST", path, &[LAUNCHER_HEADER], None, wait) {
        Ok(response) if response.ok() => Ok(()),
        Ok(response) => Err(response.problem()),
        Err(err) if timed_out(&err) => Ok(()),
        Err(err) => Err(format!("Portal did not answer: {err}")),
    }
}

/// Makes sure a Portal answers on the port, starting one when nothing does.
pub fn ensure_running(port: u16) -> Result<(), String> {
    match probe(port) {
        Probe::Portal => Ok(()),
        Probe::Other => Err(port_taken_message(port)),
        // Busy, not gone: wait for it to answer rather than start another.
        Probe::Silent => wait_for_service(port, None),
        Probe::Nothing => {
            let mut child = spawn_service(port)?;
            wait_for_service(port, Some(&mut child))
        }
    }
}

/// The service another launcher is starting: waits for it to answer, and
/// never starts one of its own.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn await_running(port: u16) -> Result<(), String> {
    match probe(port) {
        Probe::Portal => Ok(()),
        Probe::Other => Err(port_taken_message(port)),
        Probe::Silent | Probe::Nothing => wait_for_service(port, None),
    }
}

/// Stops the Portal on the port and waits until its process has exited, so
/// its files can be replaced. `Ok(false)` means no Portal was running.
pub fn shutdown(port: u16) -> Result<bool, String> {
    let ping = match ask_ping(port) {
        Ok(Some(ping)) => ping,
        Err(err) if timed_out(&err) => {
            return Err(
                "Portal did not answer, so it may still be running. Try again, or close it \
                 from Task Manager (portal.exe)."
                    .into(),
            );
        }
        Ok(None) | Err(_) => return Ok(false),
    };
    if !is_portal_ping(&ping) {
        return Ok(false);
    }
    let pid = ping_pid(&ping);
    // Setup and uninstall stop only the Portal installed beside this
    // launcher, never another copy that happens to use the port.
    if !pid.is_some_and(in_our_folder) {
        return Ok(false);
    }
    call(
        port,
        "POST",
        "/api/shutdown",
        &[LAUNCHER_HEADER],
        None,
        QUICK,
    )
    .map_err(|err| format!("Portal did not accept the stop request: {err}"))?;
    let deadline = Instant::now() + STOP_WAIT;
    while Instant::now() < deadline {
        if probe(port) == Probe::Nothing {
            // The port closes first; the process then quits the Kodi it started.
            let exited = pid.is_none_or(|pid| {
                wait_for_exit(pid, deadline.saturating_duration_since(Instant::now()))
            });
            if exited {
                return Ok(true);
            }
            break;
        }
        std::thread::sleep(POLL);
    }
    Err("Portal is still running. Close it from Task Manager (portal.exe).".into())
}

/// True when process `pid` runs from the folder this launcher is in.
/// False when that cannot be read.
#[cfg(windows)]
pub fn in_our_folder(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let Some(ours) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))
    else {
        return false;
    };
    // SAFETY: the handle is checked for null, used once and closed; the
    // buffer length is passed in and updated by the call.
    let theirs = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 {
            return false;
        }
        std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len as usize]))
    };
    let Some(theirs) = theirs.parent() else {
        return false;
    };
    let normal = |path: &std::path::Path| {
        path.to_string_lossy()
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    normal(theirs) == normal(&ours)
}

#[cfg(not(windows))]
pub fn in_our_folder(_pid: u32) -> bool {
    true
}

fn ping_pid(response: &Response) -> Option<u32> {
    response
        .json()
        .and_then(|body| body["pid"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid != 0)
}

/// Waits up to `limit` for process `pid` to exit. True when it has (or is
/// already gone).
#[cfg(windows)]
fn wait_for_exit(pid: u32, limit: Duration) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };
    // SAFETY: the handle is checked for null, waited on, and closed here.
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return true;
        }
        let millis = u32::try_from(limit.as_millis()).unwrap_or(u32::MAX);
        let done = WaitForSingleObject(handle, millis) == WAIT_OBJECT_0;
        CloseHandle(handle);
        done
    }
}

#[cfg(not(windows))]
fn wait_for_exit(_pid: u32, _limit: Duration) -> bool {
    true
}

pub fn restart(port: u16) -> Result<(), String> {
    shutdown(port)?;
    ensure_running(port)
}

/// Ends Portal process `pid`, which holds the port but stopped answering,
/// and starts a new one.
pub fn restart_hung(port: u16, pid: u32) -> Result<(), String> {
    kill_hung(port, pid)?;
    ensure_running(port)
}

/// Kills `pid` unless it answers after all. Only the Portal installed beside
/// this launcher, as `shutdown` stops.
fn kill_hung(port: u16, pid: u32) -> Result<(), String> {
    if probe(port) == Probe::Portal {
        return Ok(());
    }
    if !in_our_folder(pid) {
        return Err(format!(
            "Portal (process {pid}) is not the one installed here."
        ));
    }
    terminate(pid)?;
    let deadline = Instant::now() + STOP_WAIT;
    while !port_free(port) {
        if Instant::now() >= deadline {
            return Err(format!("Port {port} is still held after Portal was ended."));
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

#[cfg(windows)]
fn terminate(pid: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };
    let millis = u32::try_from(STOP_WAIT.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: the handle is checked for null, used here and closed.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return Ok(()); // already gone
        }
        let ended = TerminateProcess(handle, 1) != 0
            && WaitForSingleObject(handle, millis) == WAIT_OBJECT_0;
        CloseHandle(handle);
        if ended {
            Ok(())
        } else {
            Err(format!("Portal (process {pid}) could not be ended."))
        }
    }
}

#[cfg(not(windows))]
fn terminate(pid: u32) -> Result<(), String> {
    let ended = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .is_ok_and(|status| status.success());
    if ended {
        Ok(())
    } else {
        Err(format!("Portal (process {pid}) could not be ended."))
    }
}

/// Adds a line to the service's log, which the next start keeps as
/// `portal.previous.log`.
pub fn note(line: &str) {
    if let Ok(mut log) = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(log_path())
    {
        let _ = writeln!(log, "portal-launcher: {line}");
    }
}

fn install_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .ok_or_else(|| "Portal could not find its install folder.".to_string())
}

/// `%LOCALAPPDATA%\Portal`, where the service keeps its data.
pub fn data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Portal")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

fn log_path() -> PathBuf {
    log_dir().join("portal.log")
}

/// The previous run's log is kept as `portal.previous.log`.
fn open_log() -> Option<File> {
    let dir = log_dir();
    fs::create_dir_all(&dir).ok()?;
    let current = dir.join("portal.log");
    if current.is_file() {
        let _ = fs::rename(&current, dir.join("portal.previous.log"));
    }
    File::create(current).ok()
}

/// Whether nothing holds the port. The log is only set aside then, so a live
/// server never loses its log to the rename.
fn port_free(port: u16) -> bool {
    TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).is_ok()
}

fn spawn_service(port: u16) -> Result<Child, String> {
    let exe = install_dir()?.join(service_name());
    if !exe.is_file() {
        return Err(format!(
            "{} is missing from the install folder. Reinstall Portal.",
            service_name()
        ));
    }
    if !port_free(port) {
        return Err(port_taken_message(port));
    }
    let (stdout, stderr) = match open_log().and_then(|log| Some((log.try_clone().ok()?, log))) {
        Some((out, err)) => (Stdio::from(out), Stdio::from(err)),
        None => (Stdio::null(), Stdio::null()),
    };
    let mut command = Command::new(&exe);
    command
        .current_dir(install_dir()?)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    hide_window(&mut command);
    command
        .spawn()
        .map_err(|err| format!("Portal could not start: {err}"))
}

fn service_name() -> &'static str {
    if cfg!(windows) {
        "portal.exe"
    } else {
        "portal"
    }
}

/// Waits for a Portal to answer. `child` is the one this launch started, if
/// any; it is killed when the wait gives up.
fn wait_for_service(port: u16, mut child: Option<&mut Child>) -> Result<(), String> {
    let deadline = Instant::now() + START_WAIT;
    while Instant::now() < deadline {
        if probe(port) == Probe::Portal {
            return Ok(());
        }
        if let Some(Ok(Some(_))) = child.as_mut().map(|child| child.try_wait()) {
            // A second launch can lose the race for the port to the first.
            // That is fine as long as a Portal is answering.
            std::thread::sleep(Duration::from_millis(500));
            if probe(port) == Probe::Portal {
                return Ok(());
            }
            return Err(format!(
                "Portal stopped while starting.\n\n{}\n\nFull log: {}",
                log_tail(),
                log_path().display()
            ));
        }
        std::thread::sleep(POLL);
    }
    if let Some(child) = child {
        // Otherwise it could come up later with no tray to stop it.
        let _ = child.kill();
        let _ = child.wait();
    }
    Err(format!(
        "Portal did not answer within {} seconds.\n\nLog: {}",
        START_WAIT.as_secs(),
        log_path().display()
    ))
}

fn log_tail() -> String {
    let text = fs::read_to_string(log_path()).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(8).collect();
    lines.into_iter().rev().collect::<Vec<_>>().join("\n")
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

#[cfg(test)]
#[path = "../../../tests-private/launcher/src/service.rs"]
mod tests;
