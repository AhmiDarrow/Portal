//! The Kodi supervisor's Linux side: the same calls `process.rs` makes on
//! Windows, over `/proc`, signals, `flock` and X11.
//!
//! Kodi's window is found by its `_NET_WM_PID` (or, when Kodi sets none, its
//! `Kodi` window class) and hidden by withdrawing it, as a hidden Kodi is on
//! Windows. That works on an X11 desktop and, through XWayland, on a Wayland
//! one: on Wayland Portal starts Kodi with `--windowing=x11` so it has an X11
//! window to hide. With no display at all there is no window to manage.

use super::{EngineError, PlacedWindow, ALREADY_RUNNING, REVEAL_DEPTH, START_GUARD};
use super::{START_GUARD_EVERY, START_GUARD_FOR};
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConfigureWindowAux,
    ConnectionExt as _, EventMask, MapState, PropMode, UnmapNotifyEvent, Window,
    UNMAP_NOTIFY_EVENT,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

/// Kodi's own program names. The `kodi` launcher script runs one of these as
/// a child; the script itself runs as its shell, so it is not counted.
const KODI_NAMES: &[&str] = &["kodi.bin", "kodi", "kodi-x11", "kodi-wayland", "kodi-gbm"];

type XResult<T> = Result<T, Box<dyn std::error::Error>>;

// ---- processes -------------------------------------------------------------

/// The running program's file name, without the " (deleted)" a package
/// upgrade leaves on it. None for a process this user cannot look into.
fn exe_name(pid: u32) -> Option<String> {
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let name = exe.file_name()?.to_string_lossy().into_owned();
    Some(name.trim_end_matches(" (deleted)").to_string())
}

fn is_kodi(pid: u32) -> bool {
    exe_name(pid).is_some_and(|name| KODI_NAMES.contains(&name.as_str()))
}

fn own_uid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

fn uid_of(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(format!("/proc/{pid}")).ok().map(|meta| meta.uid())
}

/// Fields of `/proc/<pid>/stat` after the command name, which may hold spaces.
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &text[text.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_string).collect())
}

/// Gone, or a zombie waiting to be reaped.
fn ended(pid: u32) -> bool {
    stat_fields(pid).is_none_or(|fields| fields.first().is_some_and(|state| state == "Z"))
}

fn parent_of(pid: u32) -> Option<u32> {
    stat_fields(pid)?.get(1)?.parse().ok()
}

/// When a process started, in clock ticks since boot. None when it is gone.
pub(super) fn process_started(pid: u32) -> Option<u64> {
    // Field 22 of stat; the list here starts at field 3.
    stat_fields(pid)?.get(19)?.parse().ok()
}

/// This user's Kodi processes. Another user's Kodi is not ours to attach to,
/// show, hide or stop. None when `/proc` cannot be read.
#[cfg(not(test))]
pub(super) fn scan_kodi() -> Option<Vec<u32>> {
    let own = own_uid();
    let mut pids = Vec::new();
    for entry in fs::read_dir("/proc").ok()? {
        let Some(pid) = entry
            .ok()
            .and_then(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
        else {
            continue;
        };
        if uid_of(pid) == Some(own) && !ended(pid) && is_kodi(pid) {
            pids.push(pid);
        }
    }
    Some(pids)
}

/// Ends a process at once, as TerminateProcess does, and waits up to three
/// seconds for it to go.
fn kill_and_wait(pid: u32) {
    let Ok(target) = libc::pid_t::try_from(pid) else {
        return;
    };
    if target <= 0 {
        return;
    }
    // SAFETY: a plain signal to one process id.
    unsafe {
        libc::kill(target, libc::SIGKILL);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while !ended(pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn terminate_owned(pid: u32) {
    let pids = super::kodi_pids();
    if pids.len() != 1 || pids[0] != pid {
        return;
    }
    kill_and_wait(pid);
}

pub(super) fn stop_pid(pid: u32) {
    if pid != 0 {
        kill_and_wait(pid);
    }
}

/// One start at a time, across every Portal process of this user: an
/// exclusive `flock` on a file in the user's runtime folder.
pub(super) struct KodiStartLock(#[allow(dead_code)] fs::File);

impl KodiStartLock {
    pub(super) fn acquire() -> Result<Self, EngineError> {
        use std::os::unix::io::AsRawFd;
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_dir())
            .unwrap_or_else(std::env::temp_dir);
        let path = dir.join(format!("portal-kodi-start-{}.lock", own_uid()));
        let file = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|_| EngineError::Message("Could not start Kodi.".into()))?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            // SAFETY: the descriptor belongs to `file`, which is open.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                // Closing the file releases the lock.
                return Ok(Self(file));
            }
            if Instant::now() >= deadline {
                return Err(EngineError::Unavailable(ALREADY_RUNNING));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Starts Kodi in a session of its own, so it outlives Portal and is attached
/// again on the next launch, as a breakaway Kodi does on Windows. No input or
/// output is passed on. When `exe` is the `kodi` launcher script, the Kodi it
/// starts is the process returned.
pub(super) fn spawn_kodi(exe: &Path) -> Result<u32, EngineError> {
    let failed = || EngineError::Message("Could not start Kodi.".into());
    let mut command = Command::new(exe);
    // On Wayland, an X11 window (through XWayland) is one Portal can hide.
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && std::env::var_os("DISPLAY").is_some() {
        command.arg("--windowing=x11");
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| failed())?;
    let started = child.id();
    // Reaped here, so an ended Kodi does not stay behind as a zombie.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if is_kodi(started) {
            return Ok(started);
        }
        if let Some(pid) = super::scan_kodi()
            .unwrap_or_default()
            .into_iter()
            .find(|pid| descends_from(*pid, started))
        {
            return Ok(pid);
        }
        if ended(started) || Instant::now() >= deadline {
            return Err(failed());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn descends_from(pid: u32, ancestor: u32) -> bool {
    let mut at = pid;
    for _ in 0..16 {
        match parent_of(at) {
            Some(parent) if parent == ancestor => return true,
            Some(parent) if parent > 1 => at = parent,
            _ => return false,
        }
    }
    false
}

// ---- X11 windows -----------------------------------------------------------

struct X {
    conn: RustConnection,
    root: Window,
    pid_atom: u32,
    workarea: u32,
    active: u32,
    user_time: u32,
}

impl X {
    fn connect() -> XResult<Self> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn
            .setup()
            .roots
            .get(screen)
            .ok_or("no X screen")?
            .root;
        let atom = |name: &[u8]| -> XResult<u32> { Ok(conn.intern_atom(false, name)?.reply()?.atom) };
        let pid_atom = atom(b"_NET_WM_PID")?;
        let workarea = atom(b"_NET_WORKAREA")?;
        let active = atom(b"_NET_ACTIVE_WINDOW")?;
        let user_time = atom(b"_NET_WM_USER_TIME")?;
        Ok(Self {
            conn,
            root,
            pid_atom,
            workarea,
            active,
            user_time,
        })
    }

    /// Kodi's top-level windows: those whose `_NET_WM_PID` is `pid`, or that
    /// name no process and have Kodi's window class. Looks through window
    /// manager frames, three levels down from the root.
    fn kodi_windows(&self, pid: u32) -> XResult<Vec<Window>> {
        let mut level = vec![self.root];
        let mut found = Vec::new();
        for _ in 0..3 {
            let trees = level
                .iter()
                .map(|window| self.conn.query_tree(*window))
                .collect::<Result<Vec<_>, _>>()?;
            // A window can go between the calls; its reply is an error.
            let children: Vec<Window> = trees
                .into_iter()
                .filter_map(|cookie| cookie.reply().ok())
                .flat_map(|tree| tree.children)
                .collect();
            let pids = children
                .iter()
                .map(|window| {
                    self.conn
                        .get_property(false, *window, self.pid_atom, AtomEnum::CARDINAL, 0, 1)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let classes = children
                .iter()
                .map(|window| {
                    self.conn
                        .get_property(false, *window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 64)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let mut next = Vec::new();
            for ((window, owner), class) in children.into_iter().zip(pids).zip(classes) {
                let owner = owner
                    .reply()
                    .ok()
                    .and_then(|prop| prop.value32().and_then(|mut values| values.next()));
                let class = class.reply().ok().map(|prop| prop.value).unwrap_or_default();
                if owner == Some(pid) || (owner.is_none() && class_is_kodi(&class)) {
                    found.push(window);
                } else {
                    next.push(window);
                }
            }
            level = next;
        }
        Ok(found)
    }

    fn viewable(&self, window: Window) -> XResult<bool> {
        Ok(self.conn.get_window_attributes(window)?.reply()?.map_state == MapState::VIEWABLE)
    }

    /// Hidden as ICCCM withdraws a window: unmapped, and the window manager
    /// told, so it drops the frame too.
    fn withdraw(&self, window: Window) -> XResult<()> {
        self.conn.unmap_window(window)?;
        let event = UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: self.root,
            window,
            from_configure: false,
        };
        self.conn.send_event(
            false,
            self.root,
            EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )?;
        Ok(())
    }

    fn show(&self, window: Window, activate: bool) -> XResult<()> {
        if !activate {
            // A user time of 0 asks an EWMH window manager not to focus it.
            self.conn
                .change_property32(PropMode::REPLACE, window, self.user_time, AtomEnum::CARDINAL, &[0])?;
        }
        self.conn.map_window(window)?;
        if activate {
            let message = ClientMessageEvent::new(32, window, self.active, [2u32, 0, 0, 0, 0]);
            self.conn.send_event(
                false,
                self.root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                message,
            )?;
        }
        Ok(())
    }

    fn place(&self, window: Window, x: i32, y: i32, width: i32, height: i32) -> XResult<()> {
        let aux = ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width.max(1) as u32)
            .height(height.max(1) as u32);
        self.conn.configure_window(window, &aux)?;
        Ok(())
    }

    fn rect(&self, window: Window) -> XResult<Rect> {
        let geometry = self.conn.get_geometry(window)?.reply()?;
        let at = self.conn.translate_coordinates(window, self.root, 0, 0)?.reply()?;
        let (left, top) = (i32::from(at.dst_x), i32::from(at.dst_y));
        Ok(Rect {
            left,
            top,
            right: left + i32::from(geometry.width),
            bottom: top + i32::from(geometry.height),
        })
    }

    /// Every monitor, or the whole screen when RandR names none.
    fn monitors(&self) -> Vec<Rect> {
        let listed = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.monitors)
            .unwrap_or_default();
        let monitors: Vec<Rect> = listed
            .iter()
            .map(|monitor| Rect::sized(monitor.x.into(), monitor.y.into(), monitor.width.into(), monitor.height.into()))
            .collect();
        if monitors.is_empty() {
            vec![self.screen()]
        } else {
            monitors
        }
    }

    fn screen(&self) -> Rect {
        self.conn
            .setup()
            .roots
            .iter()
            .find(|screen| screen.root == self.root)
            .map(|screen| Rect::sized(0, 0, screen.width_in_pixels.into(), screen.height_in_pixels.into()))
            .unwrap_or(Rect::sized(0, 0, 1920, 1080))
    }

    /// The desktop's work area, past panels and docks, when the window
    /// manager publishes one.
    fn workarea(&self) -> Option<Rect> {
        let prop = self
            .conn
            .get_property(false, self.root, self.workarea, AtomEnum::CARDINAL, 0, 4)
            .ok()?
            .reply()
            .ok()?;
        let values: Vec<u32> = prop.value32()?.collect();
        let [x, y, width, height] = values.as_slice() else {
            return None;
        };
        Some(Rect::sized(*x as i32, *y as i32, *width as i32, *height as i32))
    }
}

fn class_is_kodi(class: &[u8]) -> bool {
    class
        .split(|byte| *byte == 0)
        .any(|part| part.eq_ignore_ascii_case(b"kodi"))
}

/// One connection to the display, shared, opened again after an error.
static DISPLAY: Mutex<Option<X>> = Mutex::new(None);

fn with_x<R>(run: impl FnOnce(&X) -> XResult<R>) -> Option<R> {
    let mut slot = DISPLAY.lock().unwrap_or_else(|err| err.into_inner());
    if slot.is_none() {
        *slot = X::connect().ok();
    }
    let x = slot.as_ref()?;
    let out = run(x).and_then(|value| {
        x.conn.flush()?;
        Ok(value)
    });
    match out {
        Ok(value) => Some(value),
        Err(_) => {
            *slot = None;
            None
        }
    }
}

fn window_id(hwnd: isize) -> Option<Window> {
    Window::try_from(hwnd).ok()
}

pub(super) fn windows_of(pid: u32) -> Vec<isize> {
    with_x(|x| x.kodi_windows(pid))
        .unwrap_or_default()
        .into_iter()
        .map(|window| window as isize)
        .collect()
}

pub(super) fn window_visible(pid: u32) -> bool {
    with_x(|x| {
        for window in x.kodi_windows(pid)? {
            if x.viewable(window).unwrap_or(false) {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .unwrap_or(false)
}

pub(super) fn show_hwnd(hwnd: isize, visible: bool) {
    let Some(window) = window_id(hwnd) else {
        return;
    };
    with_x(|x| if visible { x.show(window, true) } else { x.withdraw(window) });
}

pub(super) fn show_inactive(hwnd: isize) {
    if let Some(window) = window_id(hwnd) {
        with_x(|x| x.show(window, false));
    }
}

pub(super) fn window_rect(hwnd: isize) -> Option<(i32, i32, i32, i32)> {
    let window = window_id(hwnd)?;
    with_x(|x| x.rect(window)).map(|rect| (rect.left, rect.top, rect.right, rect.bottom))
}

pub(super) fn place_window_with(hwnd: isize, x: i32, y: i32, width: i32, height: i32, inactive: bool) {
    let Some(window) = window_id(hwnd) else {
        return;
    };
    with_x(|conn| {
        conn.place(window, x, y, width, height)?;
        conn.show(window, !inactive)
    });
}

pub(super) fn place_hidden(hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    if let Some(window) = window_id(hwnd) {
        with_x(|conn| conn.place(window, x, y, width, height));
    }
}

pub(super) fn offscreen_spot(window: &PlacedWindow, width: i32, height: i32) -> Option<(i32, i32)> {
    let monitors = with_x(|x| Ok(x.monitors()))?;
    offscreen_among(&monitors, window, width, height)
}

pub(super) fn beyond_desktop() -> (i32, i32) {
    let screen = with_x(|x| Ok(x.screen())).unwrap_or(Rect::sized(0, 0, 1920, 1080));
    (screen.right.saturating_add(64), screen.top)
}

pub(super) fn onscreen_place(window: &PlacedWindow) -> (i32, i32, i32, i32) {
    let Some((monitors, workarea)) = with_x(|x| Ok((x.monitors(), x.workarea()))) else {
        return (window.left, window.top, window.width.max(1), window.height.max(1));
    };
    onscreen_among(&monitors, workarea, window)
}

/// Keeps a Kodi that Portal started hidden while it starts. The root
/// window reports each new or shown window, and Kodi's are withdrawn at once;
/// a look every few milliseconds backs that up. A reveal for a dialog, or the
/// viewer showing Kodi, is left alone.
pub(super) fn guard_start(pid: u32) {
    START_GUARD.store(pid, Ordering::SeqCst);
    std::thread::spawn(move || {
        // A connection of its own, so its events reach only this thread.
        let Ok(x) = X::connect() else {
            let _ = START_GUARD.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
            return;
        };
        let watch = ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY);
        let _ = x.conn.change_window_attributes(x.root, &watch);
        let _ = x.conn.flush();
        let deadline = Instant::now() + START_GUARD_FOR;
        let mut known: Vec<Window> = Vec::new();
        let mut last_scan: Option<Instant> = None;
        while Instant::now() < deadline && START_GUARD.load(Ordering::SeqCst) == pid {
            let mut changed = false;
            while let Ok(Some(_)) = x.conn.poll_for_event() {
                changed = true;
            }
            if changed || last_scan.is_none_or(|at| at.elapsed() > Duration::from_millis(250)) {
                known = x.kodi_windows(pid).unwrap_or_default();
                last_scan = Some(Instant::now());
            }
            if REVEAL_DEPTH.load(Ordering::SeqCst) == 0 {
                for window in &known {
                    if x.viewable(*window).unwrap_or(false) {
                        let _ = x.withdraw(*window);
                    }
                }
                let _ = x.conn.flush();
            }
            std::thread::sleep(START_GUARD_EVERY);
        }
        let _ = START_GUARD.compare_exchange(pid, 0, Ordering::SeqCst, Ordering::SeqCst);
    });
}

// ---- monitor arithmetic, as Windows' MonitorFromRect does it --------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Rect {
    fn sized(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            left: x,
            top: y,
            right: x.saturating_add(width),
            bottom: y.saturating_add(height),
        }
    }

    fn overlap(&self, other: &Rect) -> i64 {
        let width = i64::from(self.right.min(other.right)) - i64::from(self.left.max(other.left));
        let height = i64::from(self.bottom.min(other.bottom)) - i64::from(self.top.max(other.top));
        if width > 0 && height > 0 {
            width * height
        } else {
            0
        }
    }

    fn gap(&self, other: &Rect) -> i64 {
        let dx = (i64::from(other.left) - i64::from(self.right))
            .max(i64::from(self.left) - i64::from(other.right))
            .max(0);
        let dy = (i64::from(other.top) - i64::from(self.bottom))
            .max(i64::from(self.top) - i64::from(other.bottom))
            .max(0);
        dx * dx + dy * dy
    }
}

/// The monitor a rectangle is most on, else the nearest one.
fn nearest(monitors: &[Rect], rect: &Rect) -> Option<usize> {
    let most = (0..monitors.len()).max_by_key(|at| monitors[*at].overlap(rect))?;
    if monitors[most].overlap(rect) > 0 {
        return Some(most);
    }
    (0..monitors.len()).min_by_key(|at| monitors[*at].gap(rect))
}

fn offscreen_among(monitors: &[Rect], window: &PlacedWindow, width: i32, height: i32) -> Option<(i32, i32)> {
    const GAP: i32 = 64;
    let was = Rect::sized(window.left, window.top, window.width, window.height);
    let home = nearest(monitors, &was)?;
    let screen = monitors[home];
    [
        (screen.right + GAP, screen.top),
        (screen.left - width - GAP, screen.top),
        (screen.left, screen.bottom + GAP),
        (screen.left, screen.top - height - GAP),
    ]
    .into_iter()
    .find(|&(x, y)| {
        let spot = Rect::sized(x, y, width, height);
        monitors.iter().all(|monitor| monitor.overlap(&spot) == 0) && nearest(monitors, &spot) == Some(home)
    })
}

fn onscreen_among(monitors: &[Rect], workarea: Option<Rect>, window: &PlacedWindow) -> (i32, i32, i32, i32) {
    let saved = Rect::sized(window.left, window.top, window.width, window.height);
    let Some(home) = nearest(monitors, &saved) else {
        return (window.left, window.top, window.width.max(1), window.height.max(1));
    };
    let screen = monitors[home];
    let work = workarea
        .map(|area| Rect {
            left: area.left.max(screen.left),
            top: area.top.max(screen.top),
            right: area.right.min(screen.right),
            bottom: area.bottom.min(screen.bottom),
        })
        .filter(|area| area.right > area.left && area.bottom > area.top)
        .unwrap_or(screen);
    let on_work = window.width > 80
        && window.height > 80
        && window.left < work.right
        && saved.right > work.left
        && window.top < work.bottom
        && saved.bottom > work.top;
    if on_work {
        return (window.left, window.top, window.width, window.height);
    }
    let span_x = (work.right - work.left).max(1);
    let span_y = (work.bottom - work.top).max(1);
    let width = window.width.max(960).min(span_x);
    let height = window.height.max(540).min(span_y);
    let x = work.left + ((span_x - width) / 2).max(0);
    let y = work.top + ((span_y - height) / 2).max(0);
    (x, y, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed(left: i32, top: i32, width: i32, height: i32) -> PlacedWindow {
        PlacedWindow {
            hwnd: 1,
            left,
            top,
            width,
            height,
        }
    }

    #[test]
    fn parks_beside_its_own_monitor_and_comes_back_on_its_work_area() {
        let left = Rect::sized(0, 0, 1920, 1080);
        let right = Rect::sized(1920, 0, 1920, 1080);
        let both = [left, right];
        // On the left monitor, with another to its right: parked to its left.
        let window = placed(100, 100, 1280, 720);
        assert_eq!(offscreen_among(&both, &window, 1280, 720), Some((-1344, 0)));
        assert_eq!(offscreen_among(&[left], &window, 1280, 720), Some((1984, 0)));
        // Already on the work area: kept where it is.
        assert_eq!(onscreen_among(&both, None, &window), (100, 100, 1280, 720));
        // Parked off every screen: centred on the right monitor's work area,
        // below a 40 px panel.
        let parked = placed(3900, 0, 1280, 720);
        let work = Some(Rect::sized(0, 40, 3840, 1040));
        assert_eq!(onscreen_among(&both, work, &parked), (2240, 200, 1280, 720));
        assert!(class_is_kodi(b"Kodi\0Kodi\0"));
        assert!(!class_is_kodi(b"firefox\0Firefox\0"));
    }

    #[test]
    fn reads_this_process_from_proc() {
        let me = std::process::id();
        assert!(process_started(me).is_some());
        assert!(!ended(me));
        assert_eq!(uid_of(me), Some(own_uid()));
        assert!(!is_kodi(me));
    }
}
