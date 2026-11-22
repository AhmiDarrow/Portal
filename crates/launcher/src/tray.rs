//! The tray icon. `portal-launcher` without `--stop` becomes this long-running process: the
//! engine's tray icon shows the menu `Model` builds, a timer checks the service every few
//! seconds, and anything that waits on the service runs as background work whose result
//! comes back to the UI thread. Questions and problems are shown in Portal's own theme.
//!
//! A second launch hands its arguments here (`AppConfig::single_instance_reply`) and exits
//! with the code this answers: `--stop` quits Portal and answers once the service stopped.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use remedy_ui::host::{
    Notification, TrayEvent, TrayIcon, TrayImage, TrayItem, Urgency, WindowIcon,
};
use remedy_ui::reactive::Signal;
use remedy_ui::{App, ConfirmSpec, InstanceReply, ReplyLater};

use crate::autostart;
use crate::model::{Cmd, Entry, Job, Model, Reaction, Service};
use crate::service::{self, EncoderChoice, Health, LanReply, Update};
use crate::ui;

/// How often the service is checked.
pub const HEALTH_EVERY: Duration = Duration::from_secs(5);
/// A double click arrives as two activations; open the dashboard once.
const OPEN_DEBOUNCE: Duration = Duration::from_millis(800);
/// Present once the first-run notification was shown.
const INTRO_MARKER: &str = "tray-intro-shown";

// The dashboard's Portal menu asks the same questions (web/src/appmenu.ts).
pub const QUIT_TEXT: &str = "Quit Portal?\n\n\
    This stops the Portal server. Anything playing or casting will stop, and Kodi will close \
    if Portal started it.\n\n\
    Open Portal again from the Start menu or desktop.";
pub const RESTART_TEXT: &str = "Restart the Portal server? Anything playing will stop.";
pub const LAN_TEXT: &str =
    "Changing who can reach Portal can restart it, which stops anything playing. Continue?";
const UPDATE_HINT: &str = "Right-click the Portal icon and choose Install update.";

/// Results of background work.
enum Event {
    Health(Health),
    Started {
        result: Result<(), String>,
        by_user: bool,
        first: bool,
    },
    Lan(Result<LanReply, String>),
    Encoder(Result<EncoderChoice, String>),
    Engine(Result<(), String>),
    Checked(Result<Update, String>),
    Install(Result<(), String>),
    Stopped(Result<bool, String>),
}

struct State {
    app: App,
    model: RefCell<Model>,
    /// Bumped whenever the menu or the tooltip may have changed: the tray follows it.
    shown_rev: Signal<u64>,
    /// The icon stays up until Portal quits (or its first start fails).
    tray_up: Cell<bool>,
    icon: (TrayImage, Option<WindowIcon>),
    health_pending: Cell<bool>,
    /// A page to open once the service answers.
    pending_open: RefCell<Option<String>>,
    last_open: Cell<Option<Instant>>,
    /// Dialogs open now. Tray clicks wait until they close, as they did for message boxes.
    dialogs: Cell<u32>,
    /// Later launches (`--stop`) waiting for the service to stop, and whether each is quiet.
    stop_replies: RefCell<Vec<(ReplyLater, bool)>>,
    /// The process's exit code once the app quits.
    exit_code: Rc<Cell<i32>>,
    next_notice: Cell<u64>,
}

/// The running tray. Cheap to clone; every clone is the same tray.
#[derive(Clone)]
pub struct Tray(Rc<State>);

impl Tray {
    /// Puts the icon up and starts the service on `port` (opening the dashboard once it
    /// answers when `open_browser`), on `app`. The process should exit with `exit_code`
    /// once the app quits.
    pub fn start(app: &App, port: u16, open_browser: bool, exit_code: Rc<Cell<i32>>) -> Tray {
        app.keep_running(true);
        let mut model = Model::new(port);
        model.autostart = autostart::enabled();
        let tray = Tray(Rc::new(State {
            app: app.clone(),
            model: RefCell::new(model),
            shown_rev: Signal::new(0),
            tray_up: Cell::new(true),
            icon: ui::icon(),
            health_pending: Cell::new(false),
            pending_open: RefCell::new(open_browser.then(|| service::dashboard_url(port))),
            last_open: Cell::new(None),
            dialogs: Cell::new(0),
            stop_replies: RefCell::new(Vec::new()),
            exit_code,
            next_notice: Cell::new(1),
        }));
        let t = tray.clone();
        app.tray_dyn(move || t.icon_now());
        let t = tray.clone();
        app.on_tray(move |event| t.on_tray(event));
        let t = tray.clone();
        app.on_second_instance(move |args| t.on_launch(&args));
        tray.begin(Job::Starting, false, true);
        tray.schedule_health();
        tray
    }

    /// The model, for tests and the brand menu.
    pub fn model(&self) -> std::cell::Ref<'_, Model> {
        self.0.model.borrow()
    }

    fn changed(&self) {
        self.0.shown_rev.update(|r| *r += 1);
    }

    /// What the tray shows now: the model's tooltip and menu, until Portal quits.
    fn icon_now(&self) -> Option<TrayIcon> {
        self.0.shown_rev.get();
        if !self.0.tray_up.get() {
            return None;
        }
        let model = self.0.model.borrow();
        let (image, artwork) = &self.0.icon;
        let tray = TrayIcon::new(model.tooltip(), image.clone(), tray_items(&model.menu()));
        Some(match artwork {
            Some(a) => tray.with_artwork(a.clone()),
            None => tray,
        })
    }

    fn hide_icon(&self) {
        self.0.tray_up.set(false);
        self.changed();
    }

    fn schedule_health(&self) {
        let t = self.clone();
        self.0.app.after(HEALTH_EVERY, move || {
            if t.0.tray_up.get() || t.0.model.borrow().quitting() {
                t.check_health();
                t.schedule_health();
            }
        });
    }

    fn on_tray(&self, event: TrayEvent) {
        if self.0.dialogs.get() > 0 {
            return;
        }
        match event {
            TrayEvent::Activate => {
                let now = Instant::now();
                let fresh = self
                    .0
                    .last_open
                    .get()
                    .is_none_or(|at| now.duration_since(at) >= OPEN_DEBOUNCE);
                self.0.last_open.set(Some(now));
                if fresh {
                    self.command(Cmd::Open);
                }
            }
            TrayEvent::Menu(id) => {
                if let Some(cmd) = Cmd::from_id(id) {
                    self.command(cmd);
                }
            }
            _ => {}
        }
    }

    /// A later launch's arguments: `--stop` quits Portal and answers 0 once the service
    /// stopped (1 when it would not); `--restart-server` restarts it; anything else opens
    /// the dashboard unless `--no-browser`.
    fn on_launch(&self, args: &[String]) -> InstanceReply {
        let flag = |name: &str| args.iter().any(|arg| arg == name);
        if flag("--stop") {
            let (reply, later) = InstanceReply::later();
            self.0
                .stop_replies
                .borrow_mut()
                .push((later, flag("--quiet")));
            self.quit();
            return reply;
        }
        if flag("--restart-server") {
            // The dashboard asks first, so the tray does not ask again.
            self.begin(Job::Restarting, true, false);
        } else if !flag("--no-browser") {
            self.command(Cmd::Open);
        }
        InstanceReply::code(0)
    }

    /// Asks `spec`; `yes` runs only when the answer is yes.
    fn ask(&self, spec: ConfirmSpec, yes: impl FnOnce(&Tray) + 'static) {
        let t = self.clone();
        self.0.dialogs.set(self.0.dialogs.get() + 1);
        self.0.app.confirm(spec, move |answer| {
            t.0.dialogs.set(t.0.dialogs.get().saturating_sub(1));
            if answer {
                yes(&t);
            }
        });
    }

    fn alert(&self, message: &str) {
        self.ask(ui::notice(message), |_| {});
    }

    /// Runs a menu command. Questions are asked first, as on the dashboard: nothing changes
    /// when the answer is no.
    pub fn command(&self, cmd: Cmd) {
        match cmd {
            Cmd::Open => self.open_page(service::dashboard_url),
            Cmd::OpenServerPage => self.open_page(service::server_page_url),
            Cmd::OpenLogs => {
                let dir = service::log_dir();
                let _ = std::fs::create_dir_all(&dir);
                let log = dir.join("portal.log");
                // The folder opens with the log selected (the folder itself when there is none).
                let target = if log.is_file() { log } else { dir };
                if let Err(err) = remedy_ui::desktop::reveal(&target) {
                    self.alert(&format!("Portal could not open its log folder ({err})."));
                }
            }
            Cmd::ToggleAutostart => {
                let next = !autostart::enabled();
                match autostart::set(next) {
                    Ok(()) => {
                        self.0.model.borrow_mut().autostart = next;
                        self.changed();
                    }
                    Err(message) => self.alert(&message),
                }
            }
            Cmd::ToggleLan => self.ask(ui::question(LAN_TEXT), Tray::toggle_lan),
            Cmd::ToggleEncoder => self.toggle_encoder(),
            Cmd::RestartEngine => self.restart_engine(),
            Cmd::StartServer => self.begin(Job::Starting, true, false),
            Cmd::RestartServer => self.ask(ui::question(RESTART_TEXT), |t| {
                t.begin(Job::Restarting, true, false);
            }),
            Cmd::CheckUpdates => self.check_updates(),
            Cmd::InstallUpdate => {
                let Some(version) = self.model().offered_update().map(str::to_string) else {
                    return;
                };
                let question = format!(
                    "Install Portal {version}?\n\n\
                     Portal will close, update, and reopen. Anything playing will stop."
                );
                self.ask(ui::question(&question), Tray::install_update);
            }
            Cmd::Quit => self.ask(ui::question(QUIT_TEXT), Tray::quit),
        }
    }

    /// Opens a dashboard page, starting the service first when it is down.
    fn open_page(&self, url: fn(u16) -> String) {
        let (target, quitting, job, state) = {
            let m = self.model();
            (url(m.port), m.quitting(), m.job, m.service)
        };
        if quitting {
            return;
        }
        match (job, state) {
            (Some(_), _) => *self.0.pending_open.borrow_mut() = Some(target),
            (None, Service::Running) => self.open(&target),
            (None, Service::PortTaken) => {
                let port = self.model().port;
                self.alert(&service::port_taken_message(port));
            }
            (None, Service::Stopped | Service::Unknown) => {
                *self.0.pending_open.borrow_mut() = Some(target);
                self.begin(Job::Starting, true, false);
            }
        }
    }

    /// Opens `url` in the default browser.
    fn open(&self, url: &str) {
        self.0.app.open_url(url, |_| {});
    }

    /// Runs `work` in the background and handles what it returns.
    fn spawn(&self, work: impl FnOnce() -> Event + Send + 'static) {
        let t = self.clone();
        self.0.app.background(work, move |event| t.handle(event));
    }

    /// Starts (or restarts) the service in the background.
    fn begin(&self, job: Job, by_user: bool, first: bool) {
        {
            let mut m = self.0.model.borrow_mut();
            if m.job.is_some() {
                return;
            }
            m.job = Some(job);
        }
        let port = self.model().port;
        self.spawn(move || Event::Started {
            result: match job {
                Job::Restarting => service::restart(port),
                _ => service::ensure_running(port),
            },
            by_user,
            first,
        });
        self.changed();
    }

    fn check_health(&self) {
        if self.0.health_pending.get() || self.model().job.is_some() {
            return;
        }
        self.0.health_pending.set(true);
        let port = self.model().port;
        self.spawn(move || Event::Health(service::health(port)));
    }

    fn toggle_lan(&self) {
        let (Some(lan), port) = (self.model().lan, self.model().port) else {
            return;
        };
        self.spawn(move || Event::Lan(service::set_lan(port, !lan)));
    }

    /// Takes effect with the next video started, like the dashboard's menu.
    fn toggle_encoder(&self) {
        let hardware = self.model().encoder.as_ref().map(|e| e.hardware);
        let (Some(hardware), port) = (hardware, self.model().port) else {
            return;
        };
        self.spawn(move || Event::Encoder(service::set_encoder(port, !hardware)));
    }

    fn restart_engine(&self) {
        let port = self.model().port;
        self.spawn(move || Event::Engine(service::restart_engine(port)));
    }

    fn check_updates(&self) {
        let port = self.model().port;
        self.spawn(move || Event::Checked(service::check_update(port)));
    }

    fn install_update(&self) {
        self.0.model.borrow_mut().updating_since = Some(Instant::now());
        self.changed();
        let port = self.model().port;
        self.spawn(move || Event::Install(service::install_update(port)));
    }

    /// Stops the service, then ends the tray. A start in flight finishes first, so the
    /// service it may bring up is stopped too.
    pub fn quit(&self) {
        let now = self.0.model.borrow_mut().quit();
        self.hide_icon();
        if now {
            self.stop();
        }
    }

    fn stop(&self) {
        let port = self.model().port;
        self.spawn(move || Event::Stopped(service::shutdown(port)));
    }

    /// Ends the app with `code`.
    fn exit(&self, code: i32) {
        self.0.exit_code.set(code);
        self.hide_icon();
        self.0.app.quit();
    }

    fn handle(&self, event: Event) {
        match event {
            Event::Health(health) => {
                self.0.health_pending.set(false);
                let reactions = self.0.model.borrow_mut().on_health(health, Instant::now());
                self.changed();
                for reaction in reactions {
                    self.react(reaction);
                }
            }
            Event::Started {
                result,
                by_user,
                first,
            } => {
                let quit = self.0.model.borrow_mut().started(result.is_ok(), by_user);
                if quit {
                    // Quit was chosen while starting: stop what just started.
                    *self.0.pending_open.borrow_mut() = None;
                    self.stop();
                    return;
                }
                self.changed();
                let open = self.0.pending_open.borrow_mut().take();
                if result.is_ok() {
                    if first {
                        self.introduce();
                    }
                    self.check_health();
                    if let Some(target) = open {
                        self.open(&target);
                    }
                }
                if let Err(message) = result {
                    if first {
                        // Gone at once; the process ends with an error once it is read.
                        self.hide_icon();
                        let t = self.clone();
                        self.0.app.confirm(ui::notice(&message), move |_| t.exit(1));
                    } else if by_user {
                        self.alert(&message);
                    }
                }
            }
            Event::Lan(Ok(reply)) => {
                self.0.model.borrow_mut().lan = Some(reply.lan);
                self.changed();
                if reply.restart_needed {
                    self.begin(Job::Restarting, true, false);
                }
            }
            Event::Encoder(Ok(choice)) => {
                self.0.model.borrow_mut().encoder = Some(choice);
                self.changed();
            }
            Event::Lan(Err(message))
            | Event::Encoder(Err(message))
            | Event::Engine(Err(message)) => {
                self.alert(&message);
            }
            Event::Engine(Ok(())) => self.check_health(),
            Event::Checked(result) => self.checked(result),
            // The installer takes over from here and closes this tray.
            Event::Install(Ok(())) => {}
            Event::Install(Err(message)) => {
                self.0.model.borrow_mut().updating_since = None;
                self.changed();
                self.notify("Portal could not update", &message, true);
            }
            Event::Stopped(result) => {
                let code = i32::from(result.is_err());
                let replies = std::mem::take(&mut *self.0.stop_replies.borrow_mut());
                let quiet = !replies.is_empty() && replies.iter().all(|(_, quiet)| *quiet);
                for (reply, _) in replies {
                    reply.send(code);
                }
                match result {
                    Err(message) if !quiet => {
                        let t = self.clone();
                        self.0
                            .app
                            .confirm(ui::notice(&message), move |_| t.exit(code));
                    }
                    _ => self.exit(code),
                }
            }
        }
    }

    fn react(&self, reaction: Reaction) {
        match reaction {
            Reaction::AutoRestart => self.begin(Job::Starting, false, false),
            Reaction::RestartHung {
                pid,
                misses,
                silent,
            } => {
                if self.model().job.is_some() {
                    return;
                }
                let process = pid.map_or_else(|| "pid unknown".to_string(), |pid| pid.to_string());
                service::note(&format!(
                    "Portal (process {process}) has not answered {misses} checks in a row over \
                     {}s; ending it and starting it again.",
                    silent.as_secs()
                ));
                self.0.model.borrow_mut().job = Some(Job::Restarting);
                let port = self.model().port;
                self.spawn(move || {
                    let result = service::restart_hung(port, pid);
                    if let Err(message) = &result {
                        service::note(&format!("restarting the hung Portal failed: {message}"));
                    }
                    Event::Started {
                        result,
                        by_user: false,
                        first: false,
                    }
                });
                self.changed();
            }
            Reaction::GaveUp => self.notify(
                "Portal's server keeps stopping",
                &format!(
                    "Portal stopped restarting it. Right-click the Portal icon to start it again. \
                     The log is in {}.",
                    log_place()
                ),
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

    fn checked(&self, result: Result<Update, String>) {
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
                self.0.model.borrow_mut().update = Some(update);
                match (offered, note) {
                    (Some(version), _) => {
                        self.0.model.borrow_mut().announced(&version);
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
                self.changed();
            }
            Err(message) => self.notify("Portal could not check for updates", &message, true),
        }
    }

    /// The first time ever, says where Portal went.
    fn introduce(&self) {
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

    /// A system notification from Portal; warnings are marked as such.
    fn notify(&self, title: &str, text: &str, warning: bool) {
        let id = self.0.next_notice.get();
        self.0.next_notice.set(id + 1);
        let urgency = if warning {
            Urgency::Warning
        } else {
            Urgency::Info
        };
        self.0
            .app
            .notify(Notification::new(id, title, text).urgency(urgency), |_| {});
    }
}

/// Where the log is, as the notification names it.
fn log_place() -> String {
    if cfg!(windows) {
        r"%LOCALAPPDATA%\Portal\logs".to_string()
    } else {
        service::log_dir().display().to_string()
    }
}

/// A tray label as written: `&` is not an access key here.
fn label(text: &str) -> String {
    text.replace('&', "&&")
}

/// The model's menu as the engine's tray items, word for word.
pub fn tray_items(entries: &[Entry]) -> Vec<TrayItem> {
    entries
        .iter()
        .map(|entry| match entry {
            Entry::Separator => TrayItem::Separator,
            Entry::Submenu(text, inner) => TrayItem::Submenu {
                label: label(text),
                items: tray_items(inner),
            },
            Entry::Item(item) if item.checkable => TrayItem::Check {
                id: item.cmd.map_or(0, Cmd::id),
                label: label(&item.label),
                enabled: item.enabled,
                checked: item.checked,
            },
            Entry::Item(item) => TrayItem::Item {
                id: item.cmd.map_or(0, Cmd::id),
                label: label(&item.label),
                enabled: item.enabled,
                default: item.default,
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "../../../tests-private/launcher/src/tray.rs"]
mod tests;
