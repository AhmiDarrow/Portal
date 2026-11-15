//! The tray icon. `portal-launcher.exe` without `--stop` becomes this
//! long-running process: a hidden window owns the notification-area icon, a
//! timer checks the service every few seconds, and anything that waits on the
//! service runs on a worker thread that posts its result back. The message
//! loop never blocks on HTTP.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, HANDLE, HWND, LPARAM,
    LRESULT, WAIT_OBJECT_0, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
    NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NINF_KEY, NIN_SELECT,
    NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, FindWindowW, GetMessageW, GetSystemMetrics,
    GetWindowThreadProcessId, KillTimer, LoadIconW, LoadImageW, PostMessageW, PostQuitMessage,
    RegisterClassExW, RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, SetTimer,
    TrackPopupMenu, TranslateMessage, HICON, HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR,
    MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, MSGFLT_ALLOW, SM_CXSMICON,
    TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON, WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_ENDSESSION, WM_NULL, WM_TIMER, WNDCLASSEXW, WS_OVERLAPPED,
};

use crate::autostart;
use crate::model::{fill_wide, Cmd, Entry, Job, Model, Reaction, Service};
use crate::service::{self, Health, LanReply, Update};
use crate::ui::{self, wide};

/// Window class of the hidden window. `--stop` finds a running tray by it.
const CLASS: &str = "PortalTray";
/// Held by the running tray so a second launch knows it is not the first.
const MUTEX: &str = r"Local\PortalTray";
/// Asks a running tray to exit without touching the service.
const EXIT_MESSAGE: &str = "PortalTrayExit";
/// Asks a running tray to restart the service, as its menu does. The
/// dashboard asks first, so the tray does not ask again.
const RESTART_MESSAGE: &str = "PortalTrayRestart";
const WM_TRAY: u32 = WM_APP + 1;
const WM_WORKER: u32 = WM_APP + 2;
const ICON_ID: u32 = 1;
const HEALTH_TIMER: usize = 1;
const HEALTH_EVERY_MS: u32 = 5_000;
const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
/// A double click arrives as two selects; open the dashboard once.
const OPEN_DEBOUNCE: Duration = Duration::from_millis(800);
/// Present once the first-run notification was shown.
const INTRO_MARKER: &str = "tray-intro-shown";

const QUIT_TEXT: &str = "Quit Portal?\n\n\
    This stops the Portal server. Anything playing or casting will stop, and Kodi will close \
    if Portal started it.\n\n\
    Open Portal again from the Start menu or desktop.";
const RESTART_TEXT: &str = "Restart the Portal server? Anything playing will stop.";
const LAN_RESTART_TEXT: &str =
    "Portal needs to restart to apply this. Anything playing will stop. Restart now?";
const UPDATE_HINT: &str = "Right-click the Portal icon and choose Install update.";

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static EXIT_REQUEST: AtomicU32 = AtomicU32::new(0);
static RESTART_REQUEST: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// Message boxes open right now. Tray clicks wait until they close.
    static DIALOGS: Cell<u32> = const { Cell::new(0) };
}

/// Results from worker threads.
enum Event {
    Health(Health),
    Started {
        result: Result<(), String>,
        by_user: bool,
        first: bool,
    },
    Lan(Result<LanReply, String>),
    Engine(Result<(), String>),
    Checked(Result<Update, String>),
    Install(Result<(), String>),
    Stopped(Result<bool, String>),
}

/// Hands worker results to the UI thread and wakes it.
#[derive(Clone)]
struct Poster {
    events: Sender<Event>,
    /// The hidden window, as an integer so the poster can cross threads.
    hwnd: usize,
}

impl Poster {
    fn spawn(&self, work: impl FnOnce() -> Event + Send + 'static) {
        let poster = self.clone();
        std::thread::spawn(move || poster.send(work()));
    }

    fn send(&self, event: Event) {
        if self.events.send(event).is_ok() {
            // SAFETY: posting to a window that is already gone just fails.
            unsafe { PostMessageW(self.hwnd as HWND, WM_WORKER, 0, 0) };
        }
    }
}

struct App {
    hwnd: HWND,
    icon: HICON,
    model: Model,
    events: Receiver<Event>,
    poster: Poster,
    icon_shown: bool,
    health_pending: bool,
    /// A page to open once the service answers.
    pending_open: Option<String>,
    last_open: Option<Instant>,
    /// Why the first start failed; `run` returns it.
    failure: Option<String>,
}

/// Held for the life of the tray process.
pub struct Instance(HANDLE);

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateMutexW and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// Claims the single-instance mutex. `None` when a tray already runs.
pub fn claim_instance() -> Option<Instance> {
    let name = wide(MUTEX);
    // SAFETY: the name is NUL-terminated; no security attributes.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    // SAFETY: reads this thread's last error right after the call.
    let error = unsafe { GetLastError() };
    if error == ERROR_ALREADY_EXISTS {
        // SAFETY: a handle to the existing mutex; ours to close.
        unsafe { CloseHandle(handle) };
        return None;
    }
    // A tray running as administrator holds a mutex this one may not open.
    // Without this check a second tray would start next to it.
    if handle.is_null() && error == ERROR_ACCESS_DENIED {
        return None;
    }
    Some(Instance(handle))
}

/// Asks a running tray from this install folder to restart the service.
/// False when there is none to ask.
pub fn request_restart() -> bool {
    let class = wide(CLASS);
    // SAFETY: the class name is NUL-terminated; no window title filter.
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return false;
    }
    let mut pid = 0u32;
    // SAFETY: hwnd came from FindWindowW; pid is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if !crate::service::in_our_folder(pid) {
        return false;
    }
    // SAFETY: posting to another process's window with no pointers attached.
    unsafe { PostMessageW(hwnd, register_message(RESTART_MESSAGE), 0, 0) != 0 }
}

/// Asks a running tray to exit without stopping the service, and waits for
/// its process to end. `Ok(false)` when no tray is running.
pub fn close_running(wait: Duration) -> Result<bool, String> {
    let class = wide(CLASS);
    let find = || {
        // SAFETY: the class name is NUL-terminated; no window title filter.
        unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) }
    };
    let hwnd = find();
    if hwnd.is_null() {
        return Ok(false);
    }
    let mut pid = 0u32;
    // SAFETY: hwnd came from FindWindowW; pid is a valid out pointer.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    // A tray from another install folder is not this launcher's to close.
    if !crate::service::in_our_folder(pid) {
        return Ok(false);
    }
    // SAFETY: plain call; a null handle is handled below.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    // SAFETY: posting to another process's window with no pointers attached.
    unsafe { PostMessageW(hwnd, register_message(EXIT_MESSAGE), 0, 0) };
    let closed = if process.is_null() {
        let deadline = Instant::now() + wait;
        loop {
            if find().is_null() {
                break true;
            }
            if Instant::now() >= deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    } else {
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: process is a valid handle opened with SYNCHRONIZE, closed once.
        unsafe {
            let done = WaitForSingleObject(process, millis) == WAIT_OBJECT_0;
            CloseHandle(process);
            done
        }
    };
    if closed {
        Ok(true)
    } else {
        Err("Portal's tray icon did not close. End portal-launcher.exe in Task Manager.".into())
    }
}

/// Runs the tray until the user quits Portal or `--stop` asks it to exit.
pub fn run(open_browser: bool) -> Result<(), String> {
    let port = service::port();
    TASKBAR_CREATED.store(register_message("TaskbarCreated"), Ordering::Relaxed);
    EXIT_REQUEST.store(register_message(EXIT_MESSAGE), Ordering::Relaxed);
    RESTART_REQUEST.store(register_message(RESTART_MESSAGE), Ordering::Relaxed);
    let hwnd = create_window()?;
    let (events, receiver) = channel();
    let mut model = Model::new(port);
    model.autostart = autostart::enabled();
    let app = App {
        hwnd,
        icon: load_icon(),
        model,
        events: receiver,
        poster: Poster {
            events,
            hwnd: hwnd as usize,
        },
        icon_shown: false,
        health_pending: false,
        pending_open: open_browser.then(|| service::dashboard_url(port)),
        last_open: None,
        failure: None,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    with_app(|app| {
        app.show_icon();
        app.start(Job::Starting, false, true);
    });
    // SAFETY: hwnd is our window; no timer callback, so WM_TIMER is posted.
    unsafe { SetTimer(hwnd, HEALTH_TIMER, HEALTH_EVERY_MS, None) };

    // SAFETY: MSG is plain data and the loop only passes it back to Windows.
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let app = APP.with(|cell| cell.borrow_mut().take());
    match app.and_then(|app| app.failure) {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

fn register_message(name: &str) -> u32 {
    let name = wide(name);
    // SAFETY: the name is NUL-terminated and outlives the call.
    unsafe { RegisterWindowMessageW(name.as_ptr()) }
}

fn create_window() -> Result<HWND, String> {
    let class = wide(CLASS);
    let title = wide("Portal");
    // SAFETY: a null module name returns this executable's handle. The class
    // and title strings outlive the calls, which copy them. The window is
    // never shown; it only receives messages.
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let mut class_info: WNDCLASSEXW = std::mem::zeroed();
        class_info.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
        class_info.lpfnWndProc = Some(window_proc);
        class_info.hInstance = instance;
        class_info.lpszClassName = class.as_ptr();
        if RegisterClassExW(&class_info) == 0 {
            return Err("Portal could not create its tray icon.".into());
        }
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            return Err("Portal could not create its tray icon.".into());
        }
        // An elevated tray would otherwise miss these from Explorer and from
        // a non-elevated `--stop`.
        for message in [
            TASKBAR_CREATED.load(Ordering::Relaxed),
            EXIT_REQUEST.load(Ordering::Relaxed),
            RESTART_REQUEST.load(Ordering::Relaxed),
        ] {
            ChangeWindowMessageFilterEx(hwnd, message, MSGFLT_ALLOW, std::ptr::null_mut());
        }
        Ok(hwnd)
    }
}

/// The exe's own icon (resource 1, embedded by assets/winres.rs).
fn load_icon() -> HICON {
    // SAFETY: resource 1 is passed as MAKEINTRESOURCE; a failed load returns
    // null and falls back to the stock application icon.
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        let icon = LoadImageW(
            GetModuleHandleW(std::ptr::null()),
            std::ptr::without_provenance::<u16>(1),
            IMAGE_ICON,
            size,
            size,
            LR_DEFAULTCOLOR,
        );
        if icon.is_null() {
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION)
        } else {
            icon
        }
    }
}

fn with_app<R>(work: impl FnOnce(&mut App) -> R) -> Option<R> {
    // A nested call (a message arriving while the state is borrowed) is
    // skipped; worker events stay queued for the next wake-up.
    APP.with(|cell| cell.try_borrow_mut().ok()?.as_mut().map(work))
}

/// Shows a message box. Never call this while the state is borrowed.
fn dialog<R>(show: impl FnOnce() -> R) -> R {
    DIALOGS.with(|open| open.set(open.get() + 1));
    let result = show();
    DIALOGS.with(|open| open.set(open.get().saturating_sub(1)));
    result
}

fn in_dialog() -> bool {
    DIALOGS.with(|open| open.get() > 0)
}

fn registered(slot: &AtomicU32, msg: u32) -> bool {
    let value = slot.load(Ordering::Relaxed);
    value != 0 && value == msg
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAY => on_tray((lparam as u32) & 0xFFFF, wparam),
        WM_WORKER => drain_events(),
        WM_TIMER if wparam == HEALTH_TIMER => {
            drain_events();
            with_app(App::check_health);
        }
        // WM_CLOSE comes from Restart Manager; the exit request from `--stop`.
        WM_CLOSE => exit_quietly(),
        _ if registered(&EXIT_REQUEST, msg) => exit_quietly(),
        _ if registered(&RESTART_REQUEST, msg) => {
            with_app(|app| app.start(Job::Restarting, true, false));
        }
        _ if registered(&TASKBAR_CREATED, msg) => {
            with_app(|app| {
                app.icon_shown = false;
                app.show_icon();
            });
        }
        WM_ENDSESSION if wparam != 0 => {
            with_app(App::hide_icon);
        }
        // SAFETY: plain call from the window's own thread.
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        // SAFETY: forwarding the message as received.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}

/// Notification-area callbacks (NOTIFYICON_VERSION_4).
fn on_tray(event: u32, wparam: WPARAM) {
    if in_dialog() {
        return;
    }
    match event {
        NIN_SELECT | NIN_KEYSELECT => {
            let now = Instant::now();
            let fresh = with_app(|app| {
                let fresh = app
                    .last_open
                    .is_none_or(|at| now.duration_since(at) >= OPEN_DEBOUNCE);
                app.last_open = Some(now);
                fresh
            });
            if fresh == Some(true) {
                command(Cmd::Open);
            }
        }
        WM_CONTEXTMENU => {
            // The anchor point, in screen coordinates.
            let x = (wparam & 0xFFFF) as u16 as i16 as i32;
            let y = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
            show_menu(x, y);
        }
        _ => {}
    }
}

fn show_menu(x: i32, y: i32) {
    let Some((hwnd, entries)) = with_app(|app| {
        app.model.autostart = autostart::enabled();
        (app.hwnd, app.model.menu())
    }) else {
        return;
    };
    // SAFETY: the menu is built, shown and destroyed here. SetForegroundWindow
    // and the WM_NULL post are the documented way to make a tray menu close
    // when the user clicks elsewhere.
    let chosen = unsafe {
        let menu = build_menu(&entries);
        SetForegroundWindow(hwnd);
        let chosen = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            0,
            hwnd,
            std::ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        chosen
    };
    if let Some(cmd) = usize::try_from(chosen).ok().and_then(Cmd::from_id) {
        command(cmd);
    }
}

/// Builds a popup menu from the model's entries.
///
/// # Safety
/// The caller destroys the returned menu; DestroyMenu also frees submenus.
unsafe fn build_menu(entries: &[Entry]) -> HMENU {
    let menu = CreatePopupMenu();
    for entry in entries {
        match entry {
            Entry::Separator => {
                AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
            }
            Entry::Submenu(label, inner) => {
                let submenu = build_menu(inner);
                let label = wide(label);
                AppendMenuW(menu, MF_POPUP, submenu as usize, label.as_ptr());
            }
            Entry::Item(item) => {
                let mut flags = MF_STRING;
                if item.checked {
                    flags |= MF_CHECKED;
                }
                if !item.enabled {
                    flags |= MF_GRAYED;
                }
                let id = item.cmd.map_or(0, Cmd::id);
                let label = wide(&item.label);
                AppendMenuW(menu, flags, id, label.as_ptr());
                if item.default {
                    SetMenuDefaultItem(menu, id as u32, 0);
                }
            }
        }
    }
    menu
}

/// Runs a menu command. Confirmations happen here, outside the state borrow.
fn command(cmd: Cmd) {
    match cmd {
        Cmd::Open => open_page(service::dashboard_url),
        Cmd::OpenServerPage => open_page(service::server_page_url),
        Cmd::OpenLogs => {
            let dir = service::log_dir();
            let _ = std::fs::create_dir_all(&dir);
            ui::open(&dir.display().to_string());
        }
        Cmd::ToggleAutostart => {
            let next = !autostart::enabled();
            match autostart::set(next) {
                Ok(()) => {
                    with_app(|app| app.model.autostart = next);
                }
                Err(message) => dialog(|| ui::alert(&message)),
            }
        }
        Cmd::ToggleLan => {
            with_app(App::toggle_lan);
        }
        Cmd::RestartEngine => {
            with_app(App::restart_engine);
        }
        Cmd::StartServer => {
            with_app(|app| app.start(Job::Starting, true, false));
        }
        Cmd::RestartServer => {
            if dialog(|| ui::confirm(RESTART_TEXT)) {
                with_app(|app| app.start(Job::Restarting, true, false));
            }
        }
        Cmd::CheckUpdates => {
            with_app(App::check_updates);
        }
        Cmd::InstallUpdate => {
            let Some(version) =
                with_app(|app| app.model.offered_update().map(str::to_string)).flatten()
            else {
                return;
            };
            let question = format!(
                "Install Portal {version}?\n\n\
                 Portal will close, update, and reopen. Anything playing will stop."
            );
            if dialog(|| ui::confirm(&question)) {
                with_app(App::install_update);
            }
        }
        Cmd::Quit => {
            if dialog(|| ui::confirm(QUIT_TEXT)) {
                with_app(App::quit);
            }
        }
    }
}

/// Opens a dashboard page, starting the service first when it is down.
fn open_page(url: fn(u16) -> String) {
    enum Next {
        Open(String),
        Alert(String),
        Wait,
    }
    let next = with_app(|app| {
        let target = url(app.model.port);
        if app.model.quitting() {
            return Next::Wait;
        }
        match (app.model.job, app.model.service) {
            (Some(_), _) => {
                app.pending_open = Some(target);
                Next::Wait
            }
            (None, Service::Running) => Next::Open(target),
            (None, Service::PortTaken) => Next::Alert(service::port_taken_message(app.model.port)),
            (None, Service::Stopped | Service::Unknown) => {
                app.pending_open = Some(target);
                app.start(Job::Starting, true, false);
                Next::Wait
            }
        }
    });
    match next {
        Some(Next::Open(target)) => ui::open(&target),
        Some(Next::Alert(message)) => dialog(|| ui::alert(&message)),
        Some(Next::Wait) | None => {}
    }
}

fn drain_events() {
    while let Some(event) = with_app(|app| app.events.try_recv().ok()).flatten() {
        handle(event);
    }
}

fn handle(event: Event) {
    match event {
        Event::Health(health) => {
            let reactions = with_app(|app| {
                app.health_pending = false;
                let reactions = app.model.on_health(health, Instant::now());
                app.refresh_tip();
                reactions
            })
            .unwrap_or_default();
            for reaction in reactions {
                with_app(|app| app.react(reaction));
            }
        }
        Event::Started {
            result,
            by_user,
            first,
        } => {
            let quit = with_app(|app| {
                let quit = app.model.started(result.is_ok(), by_user);
                if quit {
                    // Quit was chosen while starting: stop what just started.
                    app.pending_open = None;
                    app.stop();
                }
                quit
            });
            if quit.unwrap_or(false) {
                return;
            }
            let open = with_app(|app| {
                app.refresh_tip();
                let open = app.pending_open.take();
                if result.is_ok() {
                    if first {
                        app.introduce();
                    }
                    app.check_health();
                }
                open.filter(|_| result.is_ok())
            })
            .flatten();
            if let Some(target) = open {
                ui::open(&target);
            }
            if let Err(message) = result {
                if first {
                    with_app(|app| app.failure = Some(message));
                    // Gone at once, not when the pointer next crosses it.
                    with_app(App::hide_icon);
                    close_window();
                } else if by_user {
                    dialog(|| ui::alert(&message));
                }
            }
        }
        Event::Lan(Ok(reply)) => {
            with_app(|app| app.model.lan = Some(reply.lan));
            if reply.restart_needed && dialog(|| ui::confirm(LAN_RESTART_TEXT)) {
                with_app(|app| app.start(Job::Restarting, true, false));
            }
        }
        Event::Lan(Err(message)) | Event::Engine(Err(message)) => {
            dialog(|| ui::alert(&message));
        }
        Event::Engine(Ok(())) => {
            with_app(App::check_health);
        }
        Event::Checked(result) => {
            with_app(|app| app.checked(result));
        }
        // The installer takes over from here and closes this tray.
        Event::Install(Ok(())) => {}
        Event::Install(Err(message)) => {
            with_app(|app| {
                app.model.updating_since = None;
                app.refresh_tip();
                app.notify("Portal could not update", &message, true);
            });
        }
        Event::Stopped(result) => {
            if let Err(message) = result {
                dialog(|| ui::alert(&message));
            }
            close_window();
        }
    }
}

/// Removes the icon and ends the message loop; the service keeps running.
fn exit_quietly() {
    with_app(App::hide_icon);
    close_window();
}

fn close_window() {
    let Some(hwnd) = with_app(|app| app.hwnd) else {
        return;
    };
    // SAFETY: our own window, destroyed from its thread. WM_DESTROY (sent
    // synchronously) posts the quit message; the state is not borrowed here.
    unsafe {
        KillTimer(hwnd, HEALTH_TIMER);
        DestroyWindow(hwnd);
    }
}

impl App {
    /// Starts (or restarts) the service on a worker thread.
    fn start(&mut self, job: Job, by_user: bool, first: bool) {
        if self.model.job.is_some() {
            return;
        }
        self.model.job = Some(job);
        let port = self.model.port;
        self.poster.spawn(move || Event::Started {
            result: match job {
                Job::Restarting => service::restart(port),
                _ => service::ensure_running(port),
            },
            by_user,
            first,
        });
        self.refresh_tip();
    }

    fn check_health(&mut self) {
        if !self.icon_shown && !self.model.quitting() {
            // Explorer was not ready yet (early at sign-in).
            self.show_icon();
        }
        if self.health_pending || self.model.job.is_some() {
            return;
        }
        self.health_pending = true;
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Health(service::health(port)));
    }

    fn toggle_lan(&mut self) {
        let Some(lan) = self.model.lan else {
            return;
        };
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Lan(service::set_lan(port, !lan)));
    }

    fn restart_engine(&mut self) {
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Engine(service::restart_engine(port)));
    }

    fn check_updates(&mut self) {
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Checked(service::check_update(port)));
    }

    fn install_update(&mut self) {
        self.model.updating_since = Some(Instant::now());
        self.refresh_tip();
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Install(service::install_update(port)));
    }

    /// Stops the service, then ends the tray. A start in flight finishes
    /// first, so the service it may bring up is stopped too.
    fn quit(&mut self) {
        let now = self.model.quit();
        self.hide_icon();
        if now {
            self.stop();
        }
    }

    fn stop(&mut self) {
        let port = self.model.port;
        self.poster
            .spawn(move || Event::Stopped(service::shutdown(port)));
    }

    fn react(&mut self, reaction: Reaction) {
        match reaction {
            Reaction::AutoRestart => self.start(Job::Starting, false, false),
            Reaction::GaveUp => self.notify(
                "Portal's server keeps stopping",
                "Portal stopped restarting it. Right-click the Portal icon to start it again. \
                 The log is in %LOCALAPPDATA%\\Portal\\logs.",
                true,
            ),
            Reaction::UpdateAvailable(version) => self.notify(
                &format!("Portal {version} is available"),
                UPDATE_HINT,
                false,
            ),
            Reaction::UpdateFailed(error) => self.notify("Portal could not update", &error, true),
        }
    }

    fn checked(&mut self, result: Result<Update, String>) {
        match result {
            Ok(update) if update.state == "failed" => {
                let error = update
                    .error
                    .unwrap_or_else(|| "The check did not finish.".to_string());
                self.notify("Portal could not check for updates", &error, true);
            }
            Ok(update) => {
                let offered = update.latest.clone().filter(|_| update.available);
                let current = update.current.clone();
                // A newer release without a Windows installer comes with a note.
                let note = update.error.clone();
                self.model.update = Some(update);
                match (offered, note) {
                    (Some(version), _) => {
                        self.model.announced(&version);
                        self.notify(
                            &format!("Portal {version} is available"),
                            UPDATE_HINT,
                            false,
                        );
                    }
                    (None, Some(note)) => self.notify("No update to install", &note, false),
                    (None, None) => self.notify(
                        "Portal is up to date",
                        &format!("You have Portal {current}."),
                        false,
                    ),
                }
                self.refresh_tip();
            }
            Err(message) => self.notify("Portal could not check for updates", &message, true),
        }
    }

    /// The first time ever, says where Portal went.
    fn introduce(&mut self) {
        let marker = service::data_dir().join(INTRO_MARKER);
        if marker.exists() {
            return;
        }
        self.notify(
            "Portal",
            "Portal is running in the background. Use the tray icon to open or quit it.",
            false,
        );
        let _ = std::fs::create_dir_all(service::data_dir());
        let _ = std::fs::write(marker, b"");
    }

    fn icon_data(&self) -> NOTIFYICONDATAW {
        // SAFETY: NOTIFYICONDATAW is plain data; all-zero is a valid start.
        let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = ICON_ID;
        data
    }

    fn show_icon(&mut self) {
        let mut data = self.icon_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = self.icon;
        fill_wide(&mut data.szTip, &self.model.tooltip());
        // SAFETY: data is fully initialised and outlives the calls.
        unsafe {
            if Shell_NotifyIconW(NIM_ADD, &data) == 0 {
                // Explorer is not ready; the health timer tries again.
                return;
            }
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &data);
        }
        self.icon_shown = true;
    }

    fn hide_icon(&mut self) {
        if !self.icon_shown {
            return;
        }
        let data = self.icon_data();
        // SAFETY: data identifies our icon and outlives the call.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &data) };
        self.icon_shown = false;
    }

    fn refresh_tip(&mut self) {
        if !self.icon_shown {
            return;
        }
        let mut data = self.icon_data();
        data.uFlags = NIF_TIP | NIF_SHOWTIP;
        fill_wide(&mut data.szTip, &self.model.tooltip());
        // SAFETY: data identifies our icon and outlives the call.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    /// A Windows notification from the tray icon.
    fn notify(&mut self, title: &str, text: &str, warning: bool) {
        if !self.icon_shown {
            return;
        }
        let mut data = self.icon_data();
        data.uFlags = NIF_INFO;
        fill_wide(&mut data.szInfoTitle, title);
        fill_wide(&mut data.szInfo, text);
        data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        // SAFETY: data identifies our icon and outlives the call.
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }
}
