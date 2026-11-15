//! The tray's state and menu, kept free of Win32 so it can be tested.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::service::{EncoderChoice, Health, Probe, Update};

/// A service that dies on its own is restarted at most this often.
const RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(10 * 60);
/// While an update installs, the service going away is expected.
const UPDATE_GRACE: Duration = Duration::from_secs(5 * 60);
/// An install passes through "idle" only for a moment (after its checks,
/// which take well under this). Idle for longer means nothing is installing.
const UPDATE_SETTLE: Duration = Duration::from_secs(2 * 60);

/// Menu commands. The Win32 menu item ids are these values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    Open = 1,
    InstallUpdate,
    ToggleLan,
    RestartEngine,
    StartServer,
    RestartServer,
    CheckUpdates,
    OpenServerPage,
    OpenLogs,
    ToggleAutostart,
    Quit,
    ToggleEncoder,
}

impl Cmd {
    const ALL: [Cmd; 12] = [
        Cmd::Open,
        Cmd::InstallUpdate,
        Cmd::ToggleLan,
        Cmd::RestartEngine,
        Cmd::StartServer,
        Cmd::RestartServer,
        Cmd::CheckUpdates,
        Cmd::OpenServerPage,
        Cmd::OpenLogs,
        Cmd::ToggleAutostart,
        Cmd::Quit,
        Cmd::ToggleEncoder,
    ];

    pub fn id(self) -> usize {
        self as usize
    }

    pub fn from_id(id: usize) -> Option<Cmd> {
        Cmd::ALL.into_iter().find(|cmd| cmd.id() == id)
    }
}

/// One line of the tray menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    Separator,
    Submenu(String, Vec<Entry>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// `None` for a status line, which is always disabled.
    pub cmd: Option<Cmd>,
    pub label: String,
    pub enabled: bool,
    pub checked: bool,
    /// Shown in bold; what a left click does.
    pub default: bool,
}

impl Item {
    fn new(cmd: Cmd, label: impl Into<String>, enabled: bool) -> Self {
        Self {
            cmd: Some(cmd),
            label: label.into(),
            enabled,
            checked: false,
            default: false,
        }
    }

    fn status(label: impl Into<String>) -> Self {
        Self {
            cmd: None,
            label: label.into(),
            enabled: false,
            checked: false,
            default: false,
        }
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// What the tray knows about the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Unknown,
    Running,
    Stopped,
    PortTaken,
}

/// A long operation on the service. Only one runs at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Job {
    Starting,
    Restarting,
    Quitting,
}

/// Allows `max` uses in any `window` of time.
#[derive(Debug)]
pub struct RestartBudget {
    max: usize,
    window: Duration,
    used: VecDeque<Instant>,
}

impl RestartBudget {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            used: VecDeque::new(),
        }
    }

    /// Spends one restart if the budget allows it.
    pub fn take(&mut self, now: Instant) -> bool {
        while self
            .used
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= self.window)
        {
            self.used.pop_front();
        }
        if self.used.len() >= self.max {
            return false;
        }
        self.used.push_back(now);
        true
    }

    pub fn reset(&mut self) {
        self.used.clear();
    }
}

/// Something the tray should do after a health report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reaction {
    /// The service went away on its own: start it again.
    AutoRestart,
    /// It keeps going away: tell the user and stop trying.
    GaveUp,
    UpdateAvailable(String),
    UpdateFailed(String),
}

#[derive(Debug)]
pub struct Model {
    pub port: u16,
    pub service: Service,
    pub engine: Option<String>,
    pub lan: Option<bool>,
    pub encoder: Option<EncoderChoice>,
    pub update: Option<Update>,
    pub job: Option<Job>,
    pub autostart: bool,
    /// Set when the user asked to install an update.
    pub updating_since: Option<Instant>,
    /// Set when Quit arrives while a start or restart is still running.
    quit_pending: bool,
    gave_up: bool,
    budget: RestartBudget,
    announced_version: Option<String>,
    announced_error: Option<String>,
}

impl Model {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            service: Service::Unknown,
            engine: None,
            lan: None,
            encoder: None,
            update: None,
            job: None,
            autostart: false,
            updating_since: None,
            quit_pending: false,
            gave_up: false,
            budget: RestartBudget::new(RESTARTS, RESTART_WINDOW),
            announced_version: None,
            announced_error: None,
        }
    }

    fn idle(&self) -> bool {
        self.job.is_none() && self.updating_since.is_none()
    }

    fn running(&self) -> bool {
        self.service == Service::Running
    }

    /// The version to offer, when an update can be installed now.
    pub fn offered_update(&self) -> Option<&str> {
        let update = self.update.as_ref()?;
        if !update.available || matches!(update.state.as_str(), "downloading" | "installing") {
            return None;
        }
        update.latest.as_deref()
    }

    /// Begins quitting. True when the shutdown can start now; a quit that
    /// arrives during a start or restart waits for it (see `started`).
    pub fn quit(&mut self) -> bool {
        match self.job {
            Some(Job::Quitting) => false,
            Some(_) => {
                self.quit_pending = true;
                false
            }
            None => {
                self.job = Some(Job::Quitting);
                true
            }
        }
    }

    /// Whether Portal is quitting or will once a start finishes.
    pub fn quitting(&self) -> bool {
        self.quit_pending || self.job == Some(Job::Quitting)
    }

    /// Records a finished start or restart. True when a quit was waiting for
    /// it; the job is then `Quitting` and the shutdown should start.
    pub fn started(&mut self, ok: bool, by_user: bool) -> bool {
        self.job = None;
        if ok {
            self.service = Service::Running;
            self.gave_up = false;
            if by_user {
                self.budget.reset();
            }
        } else if self.service == Service::Unknown || self.service == Service::Running {
            self.service = Service::Stopped;
        }
        if self.quit_pending {
            self.quit_pending = false;
            self.job = Some(Job::Quitting);
            return true;
        }
        false
    }

    /// Marks an update the user already heard about, so the next health
    /// report does not announce it again.
    pub fn announced(&mut self, version: &str) {
        self.announced_version = Some(version.to_string());
    }

    pub fn on_health(&mut self, health: Health, now: Instant) -> Vec<Reaction> {
        let mut reactions = Vec::new();
        if self.job.is_some() {
            // A start, restart or quit is deciding the state right now.
            return reactions;
        }
        match health.probe {
            Probe::Portal => {
                self.service = Service::Running;
                self.engine = health.engine;
                self.lan = health.lan;
                self.encoder = health.encoder;
                self.update = health.update;
                self.gave_up = false;
            }
            Probe::Other => {
                self.service = Service::PortTaken;
                self.engine = None;
            }
            // Connected but slow: keep what we knew and do not restart it.
            Probe::Silent => return reactions,
            Probe::Nothing => {
                self.service = Service::Stopped;
                self.engine = None;
            }
        }
        if let Some(update) = self.update.clone().filter(|_| self.running()) {
            if let Some(version) = update.latest.filter(|_| update.available) {
                if self.announced_version.as_deref() != Some(version.as_str()) {
                    self.announced_version = Some(version.clone());
                    reactions.push(Reaction::UpdateAvailable(version));
                }
            }
            if update.state == "failed" {
                let error = update
                    .error
                    .unwrap_or_else(|| "The update did not finish.".to_string());
                // Failed background checks (offline, say) stay quiet; a failed
                // install the user asked for does not.
                if self.updating_since.take().is_some()
                    && self.announced_error.as_deref() != Some(error.as_str())
                {
                    self.announced_error = Some(error.clone());
                    reactions.push(Reaction::UpdateFailed(error));
                }
            } else {
                self.announced_error = None;
            }
            // The installer finished without replacing this Portal, or the
            // service restarted mid-download: without this the tray would say
            // "updating…" and keep the server commands greyed out for good.
            if update.state == "idle"
                && self
                    .updating_since
                    .is_some_and(|since| now.saturating_duration_since(since) >= UPDATE_SETTLE)
            {
                self.updating_since = None;
            }
        }
        if self.service == Service::Stopped {
            let updating = self
                .updating_since
                .is_some_and(|since| now.saturating_duration_since(since) < UPDATE_GRACE);
            if !updating && !self.gave_up {
                if self.budget.take(now) {
                    reactions.push(Reaction::AutoRestart);
                } else {
                    self.gave_up = true;
                    reactions.push(Reaction::GaveUp);
                }
            }
        }
        reactions
    }

    pub fn tooltip(&self) -> String {
        let state = if self.updating_since.is_some() {
            "updating…".to_string()
        } else {
            match (self.job, self.service) {
                (Some(Job::Starting), _) | (None, Service::Unknown) => "starting…".to_string(),
                (Some(Job::Restarting), _) => "restarting…".to_string(),
                (Some(Job::Quitting), _) => "closing…".to_string(),
                (None, Service::Running) if self.offered_update().is_some() => {
                    "running (update available)".to_string()
                }
                (None, Service::Running) => "running".to_string(),
                (None, Service::Stopped) => "stopped".to_string(),
                (None, Service::PortTaken) => format!("port {} is in use", self.port),
            }
        };
        format!("Portal — {state}")
    }

    pub fn menu(&self) -> Vec<Entry> {
        let mut open = Item::new(Cmd::Open, "Open Portal", self.service != Service::PortTaken);
        open.default = true;
        let mut menu = vec![Entry::Item(open)];
        if let Some(update) = self
            .update
            .as_ref()
            .filter(|_| self.updating_since.is_some())
        {
            let label = match update.progress.filter(|_| update.state == "downloading") {
                Some(done) => format!("Updating Portal… {:.0}%", done.clamp(0.0, 1.0) * 100.0),
                None => "Updating Portal…".to_string(),
            };
            menu.push(Entry::Item(Item::status(label)));
        } else if let Some(version) = self.offered_update() {
            menu.push(Entry::Item(Item::new(
                Cmd::InstallUpdate,
                format!("Install update {version}…"),
                self.running() && self.idle(),
            )));
        }
        menu.push(Entry::Submenu("Server".to_string(), self.server_menu()));
        menu.push(Entry::Item(
            Item::new(Cmd::ToggleAutostart, "Start with Windows", true).checked(self.autostart),
        ));
        menu.push(Entry::Separator);
        menu.push(Entry::Item(Item::new(
            Cmd::Quit,
            "Quit Portal",
            self.job != Some(Job::Quitting),
        )));
        menu
    }

    fn server_menu(&self) -> Vec<Entry> {
        let ready = self.running() && self.idle();
        let status = match (self.job, self.service) {
            (Some(Job::Starting), _) | (None, Service::Unknown) => "Server: starting…".to_string(),
            (Some(Job::Restarting), _) => "Server: restarting…".to_string(),
            (Some(Job::Quitting), _) => "Server: stopping…".to_string(),
            (None, Service::Running) => format!("Server: running on port {}", self.port),
            (None, Service::Stopped) => "Server: stopped".to_string(),
            (None, Service::PortTaken) => format!("Server: port {} is in use", self.port),
        };
        let mut items = vec![Entry::Item(Item::status(status))];
        if let Some(engine) = self.engine.as_deref().filter(|_| self.running()) {
            items.push(Entry::Item(Item::status(format!("Kodi engine: {engine}"))));
        }
        if let Some(update) = self.update.as_ref().filter(|_| self.running()) {
            items.push(Entry::Item(Item::status(format!(
                "Version {}",
                update.current
            ))));
        }
        if let Some(encoder) = self.encoder.as_ref().filter(|_| self.running()) {
            items.push(Entry::Item(Item::status(encoder_line(encoder))));
        }
        items.push(Entry::Separator);
        items.push(Entry::Item(
            Item::new(
                Cmd::ToggleLan,
                "Allow TVs on this network",
                ready && self.lan.is_some(),
            )
            .checked(self.lan == Some(true)),
        ));
        let card = self.encoder.as_ref().and_then(|e| card_name(&e.card));
        items.push(Entry::Item(
            Item::new(
                Cmd::ToggleEncoder,
                "Encode video on the graphics card",
                ready && card.is_some(),
            )
            .checked(card.is_some() && self.encoder.as_ref().is_some_and(|e| e.hardware)),
        ));
        items.push(Entry::Item(Item::new(
            Cmd::RestartEngine,
            "Restart Kodi engine",
            ready,
        )));
        if self.service == Service::Stopped && self.job.is_none() {
            items.push(Entry::Item(Item::new(
                Cmd::StartServer,
                "Start Portal server",
                true,
            )));
        } else {
            items.push(Entry::Item(Item::new(
                Cmd::RestartServer,
                "Restart Portal server",
                ready,
            )));
        }
        items.push(Entry::Item(Item::new(
            Cmd::CheckUpdates,
            "Check for updates",
            ready && self.update.is_some(),
        )));
        items.push(Entry::Separator);
        items.push(Entry::Item(Item::new(
            Cmd::OpenServerPage,
            "Open server page",
            self.service != Service::PortTaken,
        )));
        items.push(Entry::Item(Item::new(
            Cmd::OpenLogs,
            "Open log folder",
            true,
        )));
        items
    }
}

/// The maker of a graphics card Portal can encode with, as people know it.
fn card_name(card: &str) -> Option<&'static str> {
    match card {
        "nvidia" => Some("NVIDIA"),
        "intel" => Some("Intel"),
        "amd" => Some("AMD"),
        _ => None,
    }
}

/// What encodes the video now, in words, as the dashboard's menu says it.
fn encoder_line(encoder: &EncoderChoice) -> String {
    match card_name(&encoder.card) {
        None => "Video encoding: processor (no graphics card found for it)".to_string(),
        Some(card) if encoder.hardware => format!("Video encoding: {card} graphics card"),
        Some(_) => "Video encoding: processor".to_string(),
    }
}

/// Copies `text` into a fixed, NUL-terminated UTF-16 buffer, cutting it
/// short when it does not fit.
pub fn fill_wide(buffer: &mut [u16], text: &str) {
    let Some(room) = buffer.len().checked_sub(1) else {
        return;
    };
    let mut used = 0;
    for unit in text.encode_utf16().take(room) {
        buffer[used] = unit;
        used += 1;
    }
    // Do not leave half of a surrogate pair at the end.
    if used > 0 && (0xD800..0xDC00).contains(&buffer[used - 1]) {
        used -= 1;
    }
    buffer[used..].fill(0);
}

#[cfg(test)]
#[path = "../../../tests-private/launcher/src/model.rs"]
mod tests;
