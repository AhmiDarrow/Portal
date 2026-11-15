use crate::catalog::plain_block;
use crate::dialog_catalog::{
    busy_overlay_file, overlay_keys, pick_message, pick_percent, pick_title, DialogCatalog,
    SkinDialog, SkinList,
};
use crate::error::EngineError;
use crate::rpc::{KodiClient, Note};
use portal_api::{Prompt, PromptAction, PromptAnswer, PromptKind};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

const LIMIT: Duration = Duration::from_secs(5);
const LABEL_BATCH: usize = 40;
const SELECT_CAP: usize = 300;
const MOVE_CAP: usize = 300;
/// Kodi queues a key press from JSON-RPC and acts on it with a later frame,
/// so the focused row is read again until it moves.
const STEP_POLLS: u32 = 20;
const STEP_POLL: Duration = Duration::from_millis(25);
/// Wait before the last read of the row Select will act on.
const SETTLE: Duration = Duration::from_millis(80);
const SKIN_PREFIX: &str = "skin:";

/// Held while Portal closes a dialog on its own. Several callers read
/// prompts at once, and a second Back would cancel whatever opened next.
pub(crate) static DIALOG_ACTIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Counts prompts the user cancelled. A resolve that then ends without a
/// stream was cancelled, not refused.
pub(crate) static CANCELS: AtomicU64 = AtomicU64::new(0);

/// When an add-on window last read as a list to choose from, in ms since the
/// Unix epoch. Add-on windows also show progress overlays; only a list is
/// someone reading, and only that should hold a resolve past its usual limit.
static LIST_SHOWN_AT: AtomicU64 = AtomicU64::new(0);
const LIST_FRESH_MS: u64 = 3_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|at| at.as_millis() as u64)
        .unwrap_or(0)
}

/// True while an add-on's own list has been read as a prompt in the last
/// few seconds, as it is while the page polls the open question.
pub(crate) fn addon_list_showing() -> bool {
    let at = LIST_SHOWN_AT.load(Ordering::SeqCst);
    at != 0 && now_ms().saturating_sub(at) <= LIST_FRESH_MS
}

#[derive(Clone)]
struct RequestedInput {
    title: String,
    value: String,
}

struct SkinContent {
    title: String,
    message: String,
    progress: Option<f64>,
}

/// Item text last read from a skin list. Pages poll the prompt often, and a
/// long source list costs many label reads.
struct ListRead {
    file: String,
    ends: (String, String),
    options: Vec<String>,
}

/// The showing of a dialog the last read returned. Kodi reuses a window id
/// for every select or confirm, so the prompt id carries a number that
/// changes when the dialog reopens, its words change, or it was answered.
#[derive(Default)]
struct Showing {
    base: String,
    signature: String,
    kind: Option<PromptKind>,
    number: u64,
    present: bool,
    answered: bool,
}

pub struct Prompts {
    client: KodiClient,
    input: Arc<Mutex<Option<RequestedInput>>>,
    catalog: Arc<Mutex<DialogCatalog>>,
    list_read: Mutex<Option<ListRead>>,
    showing: Mutex<Showing>,
    /// Profile `userdata/guisettings.xml` when a profile root was given.
    /// Tests omit it, so a focus builtin is not sent to a running Kodi.
    guisettings: Option<PathBuf>,
}

impl Prompts {
    pub fn new(client: KodiClient) -> Self {
        Self::with_addon_roots(client, Vec::new())
    }

    /// `roots` are Kodi profile and/or program trees that contain an `addons` folder.
    pub fn with_addon_roots(client: KodiClient, roots: Vec<PathBuf>) -> Self {
        let input = Arc::new(Mutex::new(None));
        let mut notes = client.subscribe();
        let slot = Arc::clone(&input);
        tokio::spawn(async move {
            loop {
                match notes.recv().await {
                    Ok(Note::Notification { method, params })
                        if method == "Input.OnInputRequested" =>
                    {
                        let next = RequestedInput {
                            title: data_str(&params, "title"),
                            value: data_str(&params, "value"),
                        };
                        *slot.lock().unwrap_or_else(|err| err.into_inner()) = Some(next);
                    }
                    Ok(Note::Notification { method, .. }) if method == "Input.OnInputFinished" => {
                        *slot.lock().unwrap_or_else(|err| err.into_inner()) = None;
                    }
                    Ok(Note::Disconnected) => {
                        *slot.lock().unwrap_or_else(|err| err.into_inner()) = None;
                    }
                    Ok(Note::Notification { .. }) => {}
                    Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
        let guisettings = roots
            .first()
            .map(|root| root.join("userdata").join("guisettings.xml"));
        Self {
            client,
            input,
            catalog: Arc::new(Mutex::new(DialogCatalog::new(roots))),
            list_read: Mutex::new(None),
            showing: Mutex::new(Showing::default()),
            guisettings,
        }
    }

    /// None when Kodi is showing a normal window or a window Portal does not mirror.
    pub async fn read(&self) -> Result<Option<Prompt>, EngineError> {
        if !self.client.connected().await {
            self.forget_list_unless(None);
            self.stamp(None);
            return Ok(None);
        }
        let mut result = match self.read_connected().await {
            Err(EngineError::Rpc(crate::rpc::RpcFail::NotConnected)) => Ok(None),
            other => other,
        };
        if let Ok(prompt) = &mut result {
            self.forget_list_unless(prompt.as_ref());
            self.stamp(prompt.as_mut());
        }
        result
    }

    /// Gives a read prompt the id of this showing, `w12000-7` or
    /// `skin:file.xml-7`. A progress window keeps its number while its words
    /// change, so a Cancel in flight still matches it.
    fn stamp(&self, prompt: Option<&mut Prompt>) {
        let mut showing = self.showing.lock().unwrap_or_else(|err| err.into_inner());
        let Some(prompt) = prompt else {
            showing.present = false;
            return;
        };
        let signature = prompt_signature(prompt);
        if !showing.present
            || showing.answered
            || showing.base != prompt.id
            || showing.signature != signature
        {
            showing.number += 1;
            showing.base = prompt.id.clone();
            showing.signature = signature;
            showing.present = true;
            showing.answered = false;
        }
        showing.kind = Some(prompt.kind);
        prompt.id = format!("{}-{}", prompt.id, showing.number);
    }

    /// False when `number` names an earlier showing of the window `base`.
    fn showing_current(&self, base: &str, number: Option<u64>) -> bool {
        let Some(number) = number else {
            return true;
        };
        let showing = self.showing.lock().unwrap_or_else(|err| err.into_inner());
        showing.base != base || showing.number == number
    }

    /// The next read of a dialog that looks the same is a new showing, as
    /// when an add-on asks the same question again. Progress is left alone.
    fn note_answered(&self, base: &str) {
        let mut showing = self.showing.lock().unwrap_or_else(|err| err.into_inner());
        if showing.base == base && showing.kind != Some(PromptKind::Progress) {
            showing.answered = true;
        }
    }

    /// Drops the kept list rows unless `shown` is still that list. The same
    /// add-on window opens again for the next item, and rows kept from the
    /// last one would put a choice on the wrong source.
    fn forget_list_unless(&self, shown: Option<&Prompt>) {
        let mut kept = self.list_read.lock().unwrap_or_else(|err| err.into_inner());
        let same = match (kept.as_ref(), shown) {
            (Some(kept), Some(prompt)) => {
                prompt.kind == PromptKind::Select
                    && prompt.id.strip_prefix(SKIN_PREFIX) == Some(kept.file.as_str())
            }
            _ => false,
        };
        if !same {
            *kept = None;
        }
    }

    pub async fn answer(&self, answer: &PromptAnswer) -> Result<(), EngineError> {
        self.answer_with(answer, &|| {}).await
    }

    /// `prepare` runs immediately before each progress Back. The hidden Kodi
    /// window does not take that action until it is on screen.
    pub async fn answer_with(
        &self,
        answer: &PromptAnswer,
        prepare: &(dyn Fn() + Sync),
    ) -> Result<(), EngineError> {
        if !self.client.connected().await {
            return Err(EngineError::Unavailable("Kodi is not running."));
        }
        let (base, number) = split_showing(&answer.id);
        if !self.showing_current(base, number) {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        let result = self.answer_window(base, answer, prepare).await;
        if result.is_ok() {
            self.note_answered(base);
        }
        result
    }

    /// `base` is the prompt id without its showing number.
    async fn answer_window(
        &self,
        base: &str,
        answer: &PromptAnswer,
        prepare: &(dyn Fn() + Sync),
    ) -> Result<(), EngineError> {
        if let Some(file) = base.strip_prefix(SKIN_PREFIX) {
            let result = self.answer_skin(file, answer, prepare).await;
            // A Cancel that did not close the window (a resolve skin would
            // have taken the key) must not make the play give up.
            if result.is_ok() && answer.action == PromptAction::Cancel {
                CANCELS.fetch_add(1, Ordering::SeqCst);
            }
            return result;
        }
        let Some(id) = self.window_id().await? else {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        };
        if format!("w{id}") != base {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        // Only a question that was still there, and that this answer closed,
        // counts. A late Cancel, or one that could not be sent, would make a
        // running resolve give up early.
        let result = match (id, answer.action) {
            (10103 | 10109 | 10126, PromptAction::Submit) => {
                let text = answer.text.as_deref().unwrap_or("");
                self.call("Input.SendText", json!({"text": text, "done": true}))
                    .await?;
                Ok(())
            }
            (10126, PromptAction::Cancel) => self.back_out_of(id).await,
            (10103 | 10109, PromptAction::Cancel) => self.cancel_keyboard(id).await,
            (10101 | 10138, PromptAction::Cancel) => {
                self.cancel_progress_visible(id, prepare).await
            }
            (10100, PromptAction::Yes) => self.answer_yes().await,
            (10100, PromptAction::No) => self.answer_no().await,
            (12002 | 10147, PromptAction::Ok) => self.answer_ok(id).await,
            (12000, PromptAction::Submit) => self.answer_select(answer).await,
            (12000 | 12002 | 10147, PromptAction::Cancel) => self.back_out_of(id).await,
            (10100, PromptAction::Cancel) => self.answer_no().await,
            _ => Err(EngineError::BadRequest(
                "That answer does not fit this prompt.".into(),
            )),
        };
        if result.is_ok() && answer.action == PromptAction::Cancel {
            CANCELS.fetch_add(1, Ordering::SeqCst);
        }
        result
    }

    async fn read_connected(&self) -> Result<Option<Prompt>, EngineError> {
        if let Some(id) = self.window_id().await? {
            // Add-on settings opened after authorize stays in hidden Kodi; close it
            // so Portal remains the settings UI.
            if id == 10140 {
                let _gate = DIALOG_ACTIONS.lock().await;
                if self.window_id().await? == Some(10140) {
                    let _ = self.call("Input.Back", json!({})).await;
                }
                return Ok(None);
            }
            // A busy window with nothing to read often sits on top of the
            // question (a file prompt, a choice, a keyboard). The question is
            // what the viewer can answer, whichever add-on opened it.
            if matches!(id, 10101 | 10138 | 10160) {
                if let Some(under) = self.question_under_busy().await? {
                    return Ok(Some(under));
                }
            }
            if classify_window(id).is_some() {
                // Answered already and only stuck closing: an add-on window
                // under it, such as a source list, is what waits now.
                if self.closing(id).await? {
                    return self.read_skin_dialog().await;
                }
                return self.read_builtin(id).await;
            }
        }
        self.read_skin_dialog().await
    }

    /// Close a blank busy dialog when a real question is already open under it,
    /// then read that question. A busy dialog alone is left for progress.
    async fn question_under_busy(&self) -> Result<Option<Prompt>, EngineError> {
        const UNDER: [&str; 4] = [
            "Window.IsVisible(selectdialog)",
            "Window.IsVisible(virtualkeyboard)",
            "Window.IsVisible(yesnodialog)",
            "Window.IsVisible(filebrowser)",
        ];
        let flags = self
            .call("XBMC.GetInfoBooleans", json!({ "booleans": UNDER }))
            .await?;
        if !UNDER.iter().any(|key| flag(&flags, key)) {
            return Ok(None);
        }
        let _gate = DIALOG_ACTIONS.lock().await;
        self.call("Input.ExecuteAction", json!({ "action": "close" }))
            .await?;
        tokio::time::sleep(Duration::from_millis(150)).await;
        let Some(id) = self.window_id().await? else {
            return Ok(None);
        };
        if matches!(id, 10100 | 10103 | 10109 | 12000 | 10126) {
            return self.read_window(id).await.map(Some);
        }
        Ok(None)
    }

    /// Built-in Yes/No and Ok with no copy must not reach the page. Empty ones
    /// also block resolve while Kodi stays hidden, so dismiss them the same way
    /// empty skin Ok dialogs are cleared.
    async fn read_builtin(&self, id: i64) -> Result<Option<Prompt>, EngineError> {
        let prompt = self.read_window(id).await?;
        if !prompt_copy_is_blank(&prompt) {
            return Ok(Some(prompt));
        }
        self.dismiss_blank_builtin(id).await
    }

    async fn dismiss_blank_builtin(&self, id: i64) -> Result<Option<Prompt>, EngineError> {
        let _gate = DIALOG_ACTIONS.lock().await;
        let shown = crate::process::reveal_player_window();
        // Resume questions often paint labels a beat after the window opens.
        tokio::time::sleep(Duration::from_millis(600)).await;
        if self.window_id().await? != Some(id) {
            drop(shown);
            return Ok(None);
        }
        let again = self.read_window(id).await?;
        if !prompt_copy_is_blank(&again) {
            drop(shown);
            return Ok(Some(again));
        }
        if matches!(again.kind, PromptKind::Ok) {
            let _ = self.call("Input.Select", json!({})).await;
        } else {
            let _ = self.call("Input.Back", json!({})).await;
        }
        if !self.wait_window_left(id, 12).await? {
            let _ = self
                .call("Input.ExecuteAction", json!({ "action": "previousmenu" }))
                .await;
            let _ = self.wait_window_left(id, 12).await?;
        }
        drop(shown);
        Ok(None)
    }

    async fn read_skin_dialog(&self) -> Result<Option<Prompt>, EngineError> {
        let dialogs = {
            let mut catalog = self.catalog.lock().unwrap_or_else(|err| err.into_inner());
            catalog.dialogs().to_vec()
        };
        if dialogs.is_empty() {
            return Ok(None);
        }
        let keys: Vec<String> = dialogs
            .iter()
            .map(|dialog| format!("Window.IsActive({})", dialog.file))
            .collect();
        // An add-on's busy window can still count as active once its source
        // list is up (a hidden Kodi never finishes the closing animation). The
        // question is what waits on the viewer, so it wins over progress.
        let mut working: Option<Prompt> = None;
        for chunk in keys.chunks(LABEL_BATCH) {
            let flags = self
                .call("XBMC.GetInfoBooleans", json!({ "booleans": chunk }))
                .await?;
            for key in chunk {
                if !flag(&flags, key) {
                    continue;
                }
                let file = key
                    .strip_prefix("Window.IsActive(")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .unwrap_or("");
                if let Some(dialog) = dialogs.iter().find(|item| item.file == file) {
                    match self.read_skin(dialog).await? {
                        Some(prompt) if prompt.kind == PromptKind::Progress => {
                            working.get_or_insert(prompt);
                        }
                        Some(prompt) => return Ok(Some(prompt)),
                        None => continue,
                    }
                }
            }
        }
        Ok(working)
    }

    async fn read_skin(&self, dialog: &SkinDialog) -> Result<Option<Prompt>, EngineError> {
        if let (PromptKind::Select, Some(list)) = (dialog.kind, &dialog.list) {
            let mut options = self.read_skin_list(&dialog.file, list).await?;
            // An empty list may still have buttons, which are the choices.
            // A filled source list stays a list, so its rows keep their indexes.
            if options.is_empty() {
                options = self
                    .visible_buttons(dialog)
                    .await?
                    .into_iter()
                    .map(|(_, label)| label)
                    .collect();
            }
            if !options.is_empty() {
                LIST_SHOWN_AT.store(now_ms(), Ordering::SeqCst);
                let content = self.read_skin_content(dialog).await?;
                let (title, message) = content
                    .map(|content| (show(&content.title), show(&content.message)))
                    .unwrap_or_default();
                return Ok(Some(prompt(
                    format!("{SKIN_PREFIX}{}", dialog.file),
                    PromptKind::Select,
                    if title.is_empty() {
                        "Choose one".to_string()
                    } else {
                        title
                    },
                    message,
                    options,
                    true,
                    None,
                )));
            }
        }
        let Some(content) = self.read_skin_content(dialog).await? else {
            // Progress / resolve overlays often open before their labels land.
            // Closing them aborts the add-on's work. Leave them alone.
            if matches!(dialog.kind, PromptKind::Progress) {
                return Ok(Some(prompt(
                    format!("{SKIN_PREFIX}{}", dialog.file),
                    PromptKind::Progress,
                    "Working",
                    "",
                    Vec::new(),
                    true,
                    None,
                )));
            }
            // Never invent a title from the XML filename ("ok"). That produced a
            // blank Ok/Cancel wall that blocked play with nothing to answer.
            self.dismiss_empty_skin(dialog).await?;
            return Ok(None);
        };
        if content.title.trim().is_empty()
            && content.message.trim().is_empty()
            && matches!(dialog.kind, PromptKind::Progress)
        {
            return Ok(Some(prompt(
                format!("{SKIN_PREFIX}{}", dialog.file),
                PromptKind::Progress,
                "Working",
                "",
                Vec::new(),
                true,
                content.progress,
            )));
        }
        let title = show(&content.title);
        let message = show(&content.message);
        // An add-on's own window always closes with Back, so every one can be
        // cancelled, even one shaped like an Ok dialog.
        Ok(Some(prompt(
            format!("{SKIN_PREFIX}{}", dialog.file),
            dialog.kind,
            title,
            message,
            Vec::new(),
            true,
            content.progress,
        )))
    }

    /// Item text for each row of a skin list, up to `SELECT_CAP` rows. Empty
    /// while the add-on has not filled the list yet.
    async fn read_skin_list(
        &self,
        file: &str,
        list: &SkinList,
    ) -> Result<Vec<String>, EngineError> {
        let count_key = format!("Container({}).NumItems", list.id);
        let count =
            count_of(&field(&self.labels([count_key.clone()]).await?, &count_key)).min(SELECT_CAP);
        if count == 0 {
            return Ok(Vec::new());
        }
        let ends = (
            self.read_list_rows(list, 0, 1).await?.remove(0),
            self.read_list_rows(list, count - 1, count).await?.remove(0),
        );
        {
            let kept = self.list_read.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(kept) = kept.as_ref() {
                if kept.file == file && kept.options.len() == count && kept.ends == ends {
                    return Ok(kept.options.clone());
                }
            }
        }
        let options = self.read_list_rows(list, 0, count).await?;
        *self.list_read.lock().unwrap_or_else(|err| err.into_inner()) = Some(ListRead {
            file: file.to_string(),
            ends,
            options: options.clone(),
        });
        Ok(options)
    }

    async fn read_list_rows(
        &self,
        list: &SkinList,
        start: usize,
        end: usize,
    ) -> Result<Vec<String>, EngineError> {
        let per_batch = (LABEL_BATCH / list.fields.len().max(1)).max(1);
        let name = |item: usize, info: &str| {
            format!("Container({}).ListItemAbsolute({item}).{info}", list.id)
        };
        let mut rows = Vec::with_capacity(end.saturating_sub(start));
        let mut index = start;
        while index < end {
            let stop = (index + per_batch).min(end);
            let names: Vec<String> = (index..stop)
                .flat_map(|item| list.fields.iter().map(move |info| name(item, info)))
                .collect();
            let labels = self.labels(names).await?;
            for item in index..stop {
                let values: Vec<String> = list
                    .fields
                    .iter()
                    .map(|info| field(&labels, &name(item, info)))
                    .collect();
                rows.push(row_text(&values, item));
            }
            index = stop;
        }
        Ok(rows)
    }

    /// Buttons that currently have a caption, in skin order.
    async fn visible_buttons(
        &self,
        dialog: &SkinDialog,
    ) -> Result<Vec<(i64, String)>, EngineError> {
        if dialog.buttons.is_empty() {
            return Ok(Vec::new());
        }
        let names: Vec<String> = dialog
            .buttons
            .iter()
            .map(|id| format!("Control.GetLabel({id})"))
            .collect();
        let labels = self.labels(names.clone()).await?;
        let mut out = Vec::new();
        for (id, name) in dialog.buttons.iter().zip(names.iter()) {
            let label = show(&field(&labels, name));
            if !label.is_empty() {
                out.push((*id, label));
            }
        }
        Ok(out)
    }

    async fn read_skin_content(
        &self,
        dialog: &SkinDialog,
    ) -> Result<Option<SkinContent>, EngineError> {
        let mut names = Vec::new();
        for property in &dialog.properties {
            names.push(format!("Window.Property({property})"));
        }
        // Text controls carry the dialog body. Progress and image ids are
        // numbered on the skin too, and their descriptions are chrome.
        let mut label_ids = Vec::new();
        for id in &dialog.control_ids {
            if dialog.text_ids.contains(id) {
                label_ids.push(*id);
            }
        }
        for id in &dialog.text_ids {
            if !label_ids.contains(id) {
                label_ids.push(*id);
            }
        }
        for id in label_ids.into_iter().take(12) {
            names.push(format!("Control.GetLabel({id})"));
        }
        if names.is_empty() {
            names.push("Window.Property(heading)".into());
            names.push("Window.Property(text)".into());
            names.push("Window.Property(message)".into());
            names.push("Window.Property(percent)".into());
        }
        let mut values = Vec::new();
        for chunk in names.chunks(LABEL_BATCH) {
            let labels = self.labels(chunk.iter().cloned()).await?;
            for name in chunk {
                let value = field(&labels, name);
                if value.trim().is_empty() {
                    continue;
                }
                if let Some(property) = name
                    .strip_prefix("Window.Property(")
                    .and_then(|rest| rest.strip_suffix(')'))
                {
                    values.push((property.to_string(), value));
                } else {
                    values.push((name.clone(), value));
                }
            }
        }
        let title = pick_title(&values);
        let message = pick_message(&values, &title);
        let progress = pick_percent(&values);
        if title.trim().is_empty() && message.trim().is_empty() {
            if matches!(dialog.kind, PromptKind::Progress) && progress.is_some() {
                return Ok(Some(SkinContent {
                    title: String::new(),
                    message: String::new(),
                    progress,
                }));
            }
            return Ok(None);
        }
        Ok(Some(SkinContent {
            title,
            message,
            progress,
        }))
    }

    async fn dismiss_empty_skin(&self, dialog: &SkinDialog) -> Result<(), EngineError> {
        let _gate = DIALOG_ACTIONS.lock().await;
        let key = format!("Window.IsActive({})", dialog.file);
        if !self.focused(&key).await? {
            return Ok(());
        }
        // Select/Back are ignored while Kodi is hidden, and the close
        // animation only runs while the window is on screen.
        let shown = crate::process::reveal_player_window();
        tokio::time::sleep(Duration::from_millis(400)).await;
        if !self.focused(&key).await? {
            drop(shown);
            return Ok(());
        }
        // Labels for a real message can land after the window opens.
        if let Ok(Some(_)) = self.read_skin_content(dialog).await {
            drop(shown);
            return Ok(());
        }
        // So can the rows of a list.
        if let Some(list) = &dialog.list {
            if matches!(self.read_skin_list(&dialog.file, list).await, Ok(rows) if !rows.is_empty())
            {
                drop(shown);
                return Ok(());
            }
        }
        // An empty list can still have buttons to press.
        if matches!(self.visible_buttons(dialog).await, Ok(buttons) if !buttons.is_empty()) {
            drop(shown);
            return Ok(());
        }
        // A resolve window treats Select and Back as cancel. Pressing either
        // to clear a leftover source list stops the add-on after one source.
        let resolving = self
            .call("XBMC.GetInfoBooleans", resolve_keys())
            .await?;
        if resolve_skin_open(&resolving) {
            drop(shown);
            return Ok(());
        }
        if matches!(dialog.kind, PromptKind::Ok) && !dialog.custom {
            let _ = self.call("Input.Select", json!({})).await;
        } else {
            let _ = self.call("Input.Back", json!({})).await;
        }
        if !self.wait_skin_left(&key, 12).await? {
            let _ = self.call("Input.Back", json!({})).await;
            let _ = self.wait_skin_left(&key, 12).await?;
        }
        drop(shown);
        Ok(())
    }

    /// True when a key sent to `file` would land on an open resolve window.
    async fn resolve_would_take_the_key(&self, file: &str) -> Result<bool, EngineError> {
        let resolving = self
            .call("XBMC.GetInfoBooleans", resolve_keys())
            .await?;
        Ok(answer_would_hit_resolve(file, &resolving))
    }

    async fn answer_skin(
        &self,
        file: &str,
        answer: &PromptAnswer,
        prepare: &(dyn Fn() + Sync),
    ) -> Result<(), EngineError> {
        let key = format!("Window.IsActive({file})");
        let active = self.focused(&key).await?;
        if !active {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        let dialog = {
            let mut catalog = self.catalog.lock().unwrap_or_else(|err| err.into_inner());
            catalog
                .dialogs()
                .iter()
                .find(|dialog| dialog.file == file)
                .cloned()
        };
        let custom = dialog.as_ref().is_some_and(|dialog| dialog.custom);
        let list = dialog.as_ref().and_then(|dialog| dialog.list.clone());
        let skin = dialog;
        // Submit on a skin list is decided by whether that list has focus.
        // A resolve skin still marked active under a focused list is the
        // close animation a hidden Kodi never finishes. Any other key, and
        // a list that is not focused, still lands on the resolve window,
        // and resolve windows treat Select or Back as cancel.
        let picking = list.is_some() && answer.action == PromptAction::Submit;
        if !picking && self.resolve_would_take_the_key(file).await? {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        // A failed resolve closes an add-on window it leaves open. Its Back
        // and this answer's keys must not interleave.
        let _gate = DIALOG_ACTIONS.lock().await;
        let shown = crate::process::reveal_player_window();
        tokio::time::sleep(Duration::from_millis(300)).await;
        // The resolve window can open during the reveal. The check above is
        // then already stale, and the key would still cancel the play.
        if !self.focused(&key).await? || (!picking && self.resolve_would_take_the_key(file).await?)
        {
            drop(shown);
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        // Whatever the answer, the rows read so far belong to this window.
        self.forget_list_unless(None);
        // Ok on the add-on's own window only closes it. Its focused control
        // could be anything, and a window left open holds the add-on until
        // resolve gives up.
        let closes = match answer.action {
            PromptAction::Cancel | PromptAction::No => true,
            PromptAction::Ok => custom,
            PromptAction::Yes | PromptAction::Submit => false,
        };
        prepare();
        let result = match (&list, answer.action) {
            (Some(list), PromptAction::Submit) => {
                let count_key = format!("Container({}).NumItems", list.id);
                let count = count_of(&field(&self.labels([count_key.clone()]).await?, &count_key));
                if count == 0 {
                    let visible = match skin.as_ref() {
                        Some(dialog) => self.visible_buttons(dialog).await?,
                        None => Vec::new(),
                    };
                    let picked = answer.index.and_then(|index| {
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| visible.get(index).map(|(id, _)| *id))
                    });
                    if let Some(id) = picked {
                        self.press_control(id, &key).await
                    } else {
                        Err(EngineError::BadRequest(
                            "That answer does not fit this prompt.".into(),
                        ))
                    }
                } else {
                    self.choose_skin_row(list, &key, answer).await
                }
            }
            _ if closes => self.close_skin(&key).await,
            _ => {
                self.call("Input.Select", json!({})).await?;
                if !self.wait_skin_left(&key, 12).await? {
                    self.call("Input.Back", json!({})).await?;
                    let _ = self.wait_skin_left(&key, 12).await?;
                }
                Ok(())
            }
        };
        drop(shown);
        result
    }

    /// Move focus onto `id` and press Select. Kodi's JSON-RPC cannot focus a
    /// control by id, so this steps the way a remote would, trying each
    /// direction in turn.
    async fn press_control(&self, id: i64, active: &str) -> Result<(), EngineError> {
        let focus = format!("Control.HasFocus({id})");
        let moves = ["up", "down", "right", "left"];
        for step in 0..8 {
            if !self.focused(active).await? {
                return Err(EngineError::BadRequest("That prompt is gone.".into()));
            }
            if self.focused(&focus).await? {
                self.call("Input.Select", json!({})).await?;
                return Ok(());
            }
            self.call(
                "Input.ExecuteAction",
                json!({ "action": moves[step % moves.len()] }),
            )
            .await?;
            tokio::time::sleep(Duration::from_millis(90)).await;
        }
        if self.focused(&focus).await? {
            self.call("Input.Select", json!({})).await?;
            return Ok(());
        }
        Err(EngineError::BadRequest(
            "That answer does not fit this prompt.".into(),
        ))
    }

    /// True when Select would hit a resolve window instead of this list.
    async fn select_would_cancel(
        &self,
        list_focused: bool,
        file: &str,
    ) -> Result<bool, EngineError> {
        let flags = if list_focused {
            serde_json::Value::Null
        } else {
            self.call("XBMC.GetInfoBooleans", resolve_keys())
                .await?
        };
        Ok(select_would_cancel_resolve(list_focused, file, &flags))
    }

    /// Back, then the previous-menu and close actions, until the window is
    /// gone. Some add-on windows ignore one of them.
    async fn close_skin(&self, key: &str) -> Result<(), EngineError> {
        if self.resolve_would_take_the_key(skin_file(key)).await? {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        self.call("Input.Back", json!({})).await?;
        if self.wait_skin_left(key, 12).await? {
            return Ok(());
        }
        for action in ["previousmenu", "close"] {
            self.call("Input.ExecuteAction", json!({ "action": action }))
                .await?;
            if self.wait_skin_left(key, 12).await? {
                break;
            }
        }
        Ok(())
    }

    /// Focus row `index` of the skin list and press Select on it, as a remote
    /// would. Kodi's JSON-RPC cannot focus a row directly, so this steps
    /// through the list and checks the focused row after each step.
    async fn choose_skin_row(
        &self,
        list: &SkinList,
        key: &str,
        answer: &PromptAnswer,
    ) -> Result<(), EngineError> {
        let wrong = || EngineError::BadRequest("That answer does not fit this prompt.".into());
        let index = answer.index.ok_or_else(wrong)?;
        let count_key = format!("Container({}).NumItems", list.id);
        let count = count_of(&field(&self.labels([count_key.clone()]).await?, &count_key));
        if index < 0 || index as usize >= count.min(SELECT_CAP) {
            return Err(wrong());
        }
        let focus = format!("Control.HasFocus({})", list.id);
        let focused = self.focused(&focus).await?;
        // A focused list takes Select. A resolve skin still marked active is
        // the close animation under that list. An unfocused list is a leftover,
        // and Select would cancel the play.
        if self.select_would_cancel(focused, skin_file(key)).await? {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        if !focused {
            return Err(EngineError::BadRequest(
                "Kodi's list is not ready yet. Try again, or cancel.".into(),
            ));
        }
        let (forward, back) = if list.horizontal {
            ("Input.Right", "Input.Left")
        } else {
            ("Input.Down", "Input.Up")
        };
        if !self
            .move_to_row(list.id, i64::from(index), forward, back)
            .await?
        {
            return Err(EngineError::Unavailable("Kodi did not reach that row."));
        }
        // Stepping to the row takes long enough for a real resolve window to
        // open on top and take the list's focus. Select then belongs to it.
        if !self.focused(&focus).await? {
            return Err(EngineError::BadRequest("That prompt is gone.".into()));
        }
        self.call("Input.Select", json!({})).await?;
        let _ = self.wait_skin_left(key, 12).await?;
        Ok(())
    }

    async fn wait_skin_left(&self, key: &str, ticks: u32) -> Result<bool, EngineError> {
        for _ in 0..ticks {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !self.focused(key).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn read_window(&self, id: i64) -> Result<Prompt, EngineError> {
        match id {
            10100 => self.read_yesno().await,
            10101 => self.read_progress().await,
            10103 | 10109 => self.read_text(id).await,
            10126 => self.read_file_browser().await,
            10138 => Ok(busy("w10138", true)),
            10160 => Ok(busy("w10160", false)),
            10147 => self.read_ok("w10147", "Control.GetLabel(5)").await,
            12002 => self.read_ok("w12002", "Control.GetLabel(9)").await,
            12000 => self.read_select().await,
            _ => Err(EngineError::BadRequest("That prompt is gone.".into())),
        }
    }

    async fn read_yesno(&self) -> Result<Prompt, EngineError> {
        let labels = self
            .labels([
                "Control.GetLabel(1)",
                "Control.GetLabel(9)",
                "Control.GetLabel(12)",
            ])
            .await?;
        let title = show(&field(&labels, "Control.GetLabel(1)"));
        let message = yesno_message(
            &field(&labels, "Control.GetLabel(9)"),
            &field(&labels, "Control.GetLabel(12)"),
        );
        Ok(prompt(
            "w10100",
            PromptKind::Yesno,
            title,
            message,
            Vec::new(),
            true,
            None,
        ))
    }

    async fn read_ok(&self, id: &str, message_label: &str) -> Result<Prompt, EngineError> {
        let labels = self.labels(["Control.GetLabel(1)", message_label]).await?;
        Ok(prompt(
            id,
            PromptKind::Ok,
            show(&field(&labels, "Control.GetLabel(1)")),
            show(&field(&labels, message_label)),
            Vec::new(),
            false,
            None,
        ))
    }

    async fn read_progress(&self) -> Result<Prompt, EngineError> {
        let labels = self
            .labels([
                "Control.GetLabel(1)",
                "Control.GetLabel(9)",
                "System.Progressbar",
            ])
            .await?;
        Ok(prompt(
            "w10101",
            PromptKind::Progress,
            show(&field(&labels, "Control.GetLabel(1)")),
            show(&field(&labels, "Control.GetLabel(9)")),
            Vec::new(),
            self.progress_cancellable().await,
            progress_fraction(&field(&labels, "System.Progressbar")),
        ))
    }

    async fn progress_cancellable(&self) -> bool {
        let keys = [
            "Control.IsVisible(10)",
            "Control.IsVisible(11)",
            "Control.IsVisible(12)",
        ];
        let flags = match self
            .call("XBMC.GetInfoBooleans", json!({ "booleans": keys }))
            .await
        {
            Ok(flags) => flags,
            Err(_) => return true,
        };
        let choices = flag(&flags, keys[1]) || flag(&flags, keys[2]);
        if !choices {
            true
        } else {
            flag(&flags, keys[0])
        }
    }

    async fn read_text(&self, id: i64) -> Result<Prompt, EngineError> {
        let pending = self.requested();
        let title = match pending
            .as_ref()
            .map(|item| item.title.trim().to_string())
            .filter(|title| !title.is_empty())
        {
            Some(title) => title,
            None => {
                let labels = self.labels(["Control.GetLabel(311)"]).await?;
                let label = show(&field(&labels, "Control.GetLabel(311)"));
                if label.is_empty() {
                    "Input".to_string()
                } else {
                    label
                }
            }
        };
        let message = if secret_title(&title) {
            String::new()
        } else {
            pending.map(|item| item.value).unwrap_or_default()
        };
        Ok(prompt(
            format!("w{id}"),
            PromptKind::Text,
            title,
            message,
            Vec::new(),
            true,
            None,
        ))
    }

    /// Kodi's file window. The viewer supplies a link or a path; SendText
    /// answers it the same way a keyboard is answered.
    async fn read_file_browser(&self) -> Result<Prompt, EngineError> {
        let labels = self.labels(["Control.GetLabel(411)"]).await?;
        let title = show(&field(&labels, "Control.GetLabel(411)"));
        Ok(prompt(
            "w10126",
            PromptKind::Text,
            if title.is_empty() {
                "Choose a file".to_string()
            } else {
                title
            },
            "Paste a link, or choose a file on this PC.",
            Vec::new(),
            true,
            None,
        ))
    }

    async fn read_select(&self) -> Result<Prompt, EngineError> {
        let (list, count) = self.select_list().await?;
        let count = count.min(SELECT_CAP);
        let mut title = String::new();
        let mut options = Vec::with_capacity(count);
        let mut index = 0usize;
        let mut need_title = true;
        while need_title || index < count {
            let mut names = Vec::new();
            if need_title {
                names.push("Control.GetLabel(1)".to_string());
                need_title = false;
            }
            let batch_start = index;
            while index < count && names.len() + 2 <= LABEL_BATCH {
                names.push(format!("Container({list}).ListItemAbsolute({index}).Label"));
                names.push(format!(
                    "Container({list}).ListItemAbsolute({index}).Label2"
                ));
                index += 1;
            }
            let labels = self.labels(names).await?;
            if batch_start == 0 {
                title = show(&field(&labels, "Control.GetLabel(1)"));
            }
            for item in batch_start..index {
                let label = field(
                    &labels,
                    &format!("Container({list}).ListItemAbsolute({item}).Label"),
                );
                let label2 = field(
                    &labels,
                    &format!("Container({list}).ListItemAbsolute({item}).Label2"),
                );
                options.push(option_text(&label, &label2));
            }
        }
        Ok(prompt(
            "w12000",
            PromptKind::Select,
            title,
            String::new(),
            options,
            true,
            None,
        ))
    }

    async fn select_list(&self) -> Result<(i64, usize), EngineError> {
        let labels = self
            .labels(["Container(3).NumItems", "Container(6).NumItems"])
            .await?;
        let n3 = count_of(&field(&labels, "Container(3).NumItems"));
        let n6 = count_of(&field(&labels, "Container(6).NumItems"));
        if n6 > n3 {
            Ok((6, n6))
        } else {
            Ok((3, n3))
        }
    }

    /// Ok on a confirm. Select is ignored while Kodi is hidden, and the
    /// close animation does not finish until the window is on screen.
    /// Success is the window leaving. A key that the dialog ignores is not an answer.
    async fn answer_ok(&self, id: i64) -> Result<(), EngineError> {
        let shown = crate::process::reveal_player_window();
        tokio::time::sleep(Duration::from_millis(300)).await;
        if self.window_id().await? == Some(id) {
            self.call("Input.Select", json!({})).await?;
            if !self.wait_window_left(id, 12).await? {
                self.call("Input.Back", json!({})).await?;
                if !self.wait_window_left(id, 12).await? {
                    drop(shown);
                    return Err(answer_missed());
                }
            }
        }
        drop(shown);
        Ok(())
    }

    async fn answer_select(&self, answer: &PromptAnswer) -> Result<(), EngineError> {
        let shown = crate::process::reveal_player_window();
        tokio::time::sleep(Duration::from_millis(300)).await;
        if self.window_id().await? == Some(12000) {
            self.submit_select(answer).await?;
            if !self.wait_window_left(12000, 12).await? {
                drop(shown);
                return Err(answer_missed());
            }
        }
        drop(shown);
        Ok(())
    }

    async fn submit_select(&self, answer: &PromptAnswer) -> Result<(), EngineError> {
        let Some(index) = answer.index else {
            return Err(EngineError::BadRequest(
                "That answer does not fit this prompt.".into(),
            ));
        };
        if index < 0 {
            return Err(EngineError::BadRequest(
                "That answer does not fit this prompt.".into(),
            ));
        }
        let (list, count) = self.select_list().await?;
        if index as usize >= count.min(SELECT_CAP) {
            return Err(EngineError::BadRequest(
                "That answer does not fit this prompt.".into(),
            ));
        }
        // Select on a row short of the one asked for would pick another item.
        if !self
            .move_to_row(list, i64::from(index), "Input.Down", "Input.Up")
            .await?
        {
            return Err(EngineError::Unavailable("Kodi did not reach that row."));
        }
        self.call("Input.Select", json!({})).await?;
        Ok(())
    }

    /// Steps the focused row of `list` toward `target`. True once it is there
    /// and stays there. Each press waits for the row to move before the next,
    /// since Kodi acts on a press a frame later; reading at once would see the
    /// old row, press again and land past the target. False when the row
    /// cannot be read or stops moving, as when the list lost focus.
    async fn move_to_row(
        &self,
        list: i64,
        target: i64,
        forward: &str,
        back: &str,
    ) -> Result<bool, EngineError> {
        let Some(mut current) = self.focused_row(list).await? else {
            return Ok(false);
        };
        for _ in 0..MOVE_CAP {
            if current == target {
                tokio::time::sleep(SETTLE).await;
                match self.focused_row(list).await? {
                    Some(row) if row == target => return Ok(true),
                    Some(row) => {
                        current = row;
                        continue;
                    }
                    None => return Ok(false),
                }
            }
            let method = if current < target { forward } else { back };
            self.call(method, json!({})).await?;
            let Some(row) = self.row_after_press(list, current).await? else {
                return Ok(false);
            };
            current = row;
        }
        Ok(false)
    }

    /// The focused row once it differs from `from`. None when it cannot be
    /// read or has not moved after `STEP_POLLS` reads.
    async fn row_after_press(&self, list: i64, from: i64) -> Result<Option<i64>, EngineError> {
        for _ in 0..STEP_POLLS {
            match self.focused_row(list).await? {
                Some(row) if row != from => return Ok(Some(row)),
                Some(_) => tokio::time::sleep(STEP_POLL).await,
                None => return Ok(None),
            }
        }
        Ok(None)
    }

    /// The focused row from 0 over the whole list. `Container.Position` is
    /// not used: it is the row on screen, so a scrolled list reads wrong.
    async fn focused_row(&self, list: i64) -> Result<Option<i64>, EngineError> {
        let key = format!("Container({list}).CurrentItem");
        let labels = self.labels([key.clone()]).await?;
        Ok(parse_i64(&field(&labels, &key))
            .filter(|item| *item >= 1)
            .map(|item| item - 1))
    }

    async fn cancel_progress_visible(
        &self,
        id: i64,
        prepare: &(dyn Fn() + Sync),
    ) -> Result<(), EngineError> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            prepare();
            tokio::time::sleep(Duration::from_millis(120)).await;
            prepare();
            self.cancel_progress(id).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
            if self.window_id().await? != Some(id) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    async fn cancel_progress(&self, id: i64) -> Result<(), EngineError> {
        self.call("Input.Back", json!({})).await?;
        if self.window_id().await? == Some(id) {
            self.call("Input.ExecuteAction", json!({"action": "previousmenu"}))
                .await?;
        }
        Ok(())
    }

    async fn cancel_keyboard(&self, id: i64) -> Result<(), EngineError> {
        self.call("Input.ExecuteAction", json!({"action": "previousmenu"}))
            .await?;
        if self.window_id().await? == Some(id) {
            self.call("Input.ExecuteAction", json!({"action": "previousmenu"}))
                .await?;
        }
        Ok(())
    }

    /// Yes on a confirm. The No button is focused when the dialog opens, and
    /// Left is ignored until Kodi is the active window. Select counts only
    /// after Yes is focused. A click on the revealed dialog can close it
    /// before that Select; the question is still answered. The window staying
    /// up is not an answer.
    async fn answer_yes(&self) -> Result<(), EngineError> {
        let _gate = DIALOG_ACTIONS.lock().await;
        let shown = crate::process::reveal_player_for_answer();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let closed = self.confirm_left(11).await?;
        if closed {
            self.finish_close_visible().await;
        }
        let follow = closed && self.modal_active().await;
        // A Kodi that was on screen before (the viewer's own, or shown on
        // purpose) stays on screen.
        let was_hidden = shown.as_ref().is_some_and(|window| window.was_hidden());
        drop(shown);
        if closed && !follow && was_hidden {
            crate::process::hide_player();
        }
        if closed {
            Ok(())
        } else {
            Err(answer_missed())
        }
    }

    /// No on a confirm. Hidden Kodi freezes the dialog close animation, so
    /// the window stays up until the dialog is gone. A dialog that is still
    /// there was not answered.
    async fn answer_no(&self) -> Result<(), EngineError> {
        let _gate = DIALOG_ACTIONS.lock().await;
        let shown = crate::process::reveal_player_for_answer();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut closed = self.yesno_left().await?;
        if !closed {
            closed = self.press_yesno(10).await?;
        }
        if !closed {
            if self.window_id().await? == Some(10100) {
                self.call("Input.Back", json!({})).await?;
                closed = self.wait_yesno_left(16).await?;
            } else {
                closed = true;
            }
        }
        if closed {
            self.finish_close_visible().await;
        }
        drop(shown);
        if closed {
            Ok(())
        } else {
            Err(answer_missed())
        }
    }

    /// True once window 10100 has left. Select on `control` is one way. The
    /// viewer closing the revealed dialog first is another.
    async fn confirm_left(&self, control: i64) -> Result<bool, EngineError> {
        if self.yesno_left().await? {
            return Ok(true);
        }
        match self.press_yes_control(control).await {
            Ok(()) => self.wait_yesno_left(16).await,
            Err(err) => {
                if self.window_id().await? != Some(10100) {
                    Ok(true)
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Stay on screen while the dialog close animation ends. Hiding during
    /// that animation leaves the add-on blocked inside the question.
    async fn finish_close_visible(&self) {
        for _ in 0..8 {
            if !self.modal_close_stuck().await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }

    /// Press `control` on the yes/no dialog and wait until that dialog leaves.
    /// Skins put Yes on either side of No, so focus is walked both ways.
    /// Select is sent only when that control is focused.
    async fn press_yesno(&self, control: i64) -> Result<bool, EngineError> {
        if self.window_id().await? != Some(10100) {
            return Ok(true);
        }
        match self.press_yes_control(control).await {
            Ok(()) => self.wait_yesno_left(16).await,
            Err(_) => {
                // Focus never held, or the viewer already closed the dialog.
                // A Select that was sent does not get a second press. Back is
                // still available to the caller when this returns false.
                if self.window_id().await? != Some(10100) {
                    return Ok(true);
                }
                if self.press_yes_control(control).await.is_ok() {
                    return self.wait_yesno_left(16).await;
                }
                Ok(self.window_id().await? != Some(10100))
            }
        }
    }

    async fn press_yes_control(&self, control: i64) -> Result<(), EngineError> {
        let key = format!("Control.HasFocus({control})");
        // Arrow keys are ignored while Kodi is not ready, and a second arrow
        // wraps off Yes onto No. Focus the button directly, then Select only
        // after it has stayed focused on the open dialog.
        let toward = if control == 11 {
            "Input.Left"
        } else {
            "Input.Right"
        };
        let mut held = 0;
        let mut stepped = false;
        for _ in 0..40 {
            if self.window_id().await? != Some(10100) {
                return Err(answer_missed());
            }
            if self.focused(&key).await? && self.modal_active().await {
                held += 1;
                if held >= 2 {
                    self.call("Input.Select", json!({})).await?;
                    return Ok(());
                }
            } else {
                held = 0;
                let mut sent = false;
                if let Some(path) = &self.guisettings {
                    sent = crate::event::send_dialog_focus(path, control).await.is_ok();
                }
                if !sent && !stepped {
                    self.call(toward, json!({})).await?;
                    stepped = true;
                }
            }
            tokio::time::sleep(Duration::from_millis(80)).await;
        }
        Err(answer_missed())
    }

    async fn modal_active(&self) -> bool {
        const ACTIVE: &str = "System.HasActiveModalDialog";
        match self
            .call("XBMC.GetInfoBooleans", json!({ "booleans": [ACTIVE] }))
            .await
        {
            Ok(value) => flag(&value, ACTIVE),
            Err(_) => false,
        }
    }

    /// Cancel on a list or notice: Back, with the window on screen, since
    /// hidden Kodi ignores it. A list left behind by an add-on that already
    /// gave up closes the same way.
    async fn back_out_of(&self, id: i64) -> Result<(), EngineError> {
        let _gate = DIALOG_ACTIONS.lock().await;
        let shown = crate::process::reveal_player_window();
        tokio::time::sleep(Duration::from_millis(300)).await;
        if self.window_id().await? == Some(id) {
            self.call("Input.Back", json!({})).await?;
            if !self.wait_window_left(id, 12).await? {
                self.call("Input.ExecuteAction", json!({"action": "previousmenu"}))
                    .await?;
                if !self.wait_window_left(id, 12).await? {
                    drop(shown);
                    return Err(answer_missed());
                }
            }
        }
        drop(shown);
        Ok(())
    }

    /// Like `wait_window_left` for the yes/no dialog, where a dialog stuck in
    /// its close animation has been answered too.
    async fn wait_yesno_left(&self, ticks: u32) -> Result<bool, EngineError> {
        for _ in 0..ticks {
            if self.yesno_left().await? {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(false)
    }

    async fn wait_window_left(&self, id: i64, ticks: u32) -> Result<bool, EngineError> {
        for _ in 0..ticks {
            if self.window_id().await? != Some(id) {
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(false)
    }

    async fn focused(&self, key: &str) -> Result<bool, EngineError> {
        let flags = self
            .call("XBMC.GetInfoBooleans", json!({ "booleans": [key] }))
            .await?;
        Ok(flag(&flags, key))
    }

    async fn window_id(&self) -> Result<Option<i64>, EngineError> {
        let props = self
            .call(
                "GUI.GetProperties",
                json!({"properties": ["currentwindow"]}),
            )
            .await?;
        Ok(props
            .get("currentwindow")
            .and_then(|window| window.get("id"))
            .and_then(Value::as_i64))
    }

    /// The yes/no dialog is gone, or still on top only while it closes: the
    /// question was answered either way.
    async fn yesno_left(&self) -> Result<bool, EngineError> {
        if self.window_id().await? != Some(10100) {
            return Ok(true);
        }
        self.closing(10100).await
    }

    /// Window `id` is on top but no longer active: Kodi is closing it, and
    /// a hidden Kodi never finishes that close.
    async fn closing(&self, id: i64) -> Result<bool, EngineError> {
        Ok(!self.focused(&format!("Window.IsActive({id})")).await?)
    }

    /// A modal dialog is closing. Kodi ignores every action until that close
    /// animation ends, Back and a controller alike, and a hidden Kodi never
    /// advances it. Usually no other modal is open then; but Kodi can still
    /// count one as active while the dialog on top is the one closing.
    pub async fn modal_close_stuck(&self) -> bool {
        const VISIBLE: &str = "System.HasVisibleModalDialog";
        const ACTIVE: &str = "System.HasActiveModalDialog";
        let Ok(value) = self
            .call(
                "XBMC.GetInfoBooleans",
                json!({ "booleans": [VISIBLE, ACTIVE] }),
            )
            .await
        else {
            return false;
        };
        if !flag(&value, VISIBLE) {
            return false;
        }
        if !flag(&value, ACTIVE) {
            return true;
        }
        match self.window_id().await {
            Ok(Some(id)) if is_dialog_window(id) => self.closing(id).await.unwrap_or(false),
            _ => false,
        }
    }

    /// Shows Kodi without activating it until a stuck dialog close ends,
    /// then hides it again. True when the dialog finished closing.
    pub async fn finish_modal_close(&self) -> bool {
        let _gate = DIALOG_ACTIONS.lock().await;
        if !self.modal_close_stuck().await {
            return true;
        }
        let Some(shown) = crate::process::reveal_player_window() else {
            return false;
        };
        let mut closed = false;
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if !self.modal_close_stuck().await {
                closed = true;
                break;
            }
        }
        drop(shown);
        closed
    }

    async fn labels(
        &self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Value, EngineError> {
        let names: Vec<String> = names.into_iter().map(Into::into).collect();
        self.call("XBMC.GetInfoLabels", json!({ "labels": names }))
            .await
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, EngineError> {
        self.client
            .call(method, params, LIMIT)
            .await
            .map_err(Into::into)
    }

    fn requested(&self) -> Option<RequestedInput> {
        self.input
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }
}

/// Kodi's own dialogs: the 10100 range, and the ones Portal reads as prompts.
fn is_dialog_window(id: i64) -> bool {
    (10100..=10199).contains(&id) || classify_window(id).is_some()
}

pub fn classify_window(id: i64) -> Option<PromptKind> {
    match id {
        10100 => Some(PromptKind::Yesno),
        10101 | 10138 | 10160 => Some(PromptKind::Progress),
        10103 | 10109 | 10126 => Some(PromptKind::Text),
        10147 | 12002 => Some(PromptKind::Ok),
        12000 => Some(PromptKind::Select),
        _ => None,
    }
}

pub fn progress_fraction(label: &str) -> Option<f64> {
    let text = label.trim();
    if text.is_empty() {
        return None;
    }
    text.parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|value| value / 100.0)
}

/// Empty Yes/No and Ok dialogs have nothing to show. Progress/Text/Select keep
/// working even when the heading is still blank.
fn prompt_copy_is_blank(prompt: &Prompt) -> bool {
    match prompt.kind {
        PromptKind::Yesno | PromptKind::Ok => {
            prompt.title.trim().is_empty()
                && prompt.message.trim().is_empty()
                && prompt.options.iter().all(|option| option.trim().is_empty())
        }
        PromptKind::Progress | PromptKind::Text | PromptKind::Select => false,
    }
}

/// What makes two reads the same showing. Progress words and fraction move
/// all the time, so only the kind counts there.
fn prompt_signature(prompt: &Prompt) -> String {
    if prompt.kind == PromptKind::Progress {
        return "progress".to_string();
    }
    serde_json::to_string(&(
        prompt.kind,
        &prompt.title,
        &prompt.message,
        &prompt.options,
        prompt.cancellable,
    ))
    .unwrap_or_default()
}

/// `w12000-7` into `w12000` and 7. An id without a number, as older pages
/// send, is taken whole. A skin file name ends in `.xml`, so a dash inside
/// it is never read as one.
fn split_showing(id: &str) -> (&str, Option<u64>) {
    match id.rsplit_once('-') {
        Some((base, number))
            if !base.is_empty()
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            match number.parse() {
                Ok(number) => (base, Some(number)),
                Err(_) => (id, None),
            }
        }
        _ => (id, None),
    }
}

fn busy(id: &str, cancellable: bool) -> Prompt {
    prompt(
        id,
        PromptKind::Progress,
        "Working",
        "",
        Vec::new(),
        cancellable,
        None,
    )
}

fn prompt(
    id: impl Into<String>,
    kind: PromptKind,
    title: impl Into<String>,
    message: impl Into<String>,
    options: Vec<String>,
    cancellable: bool,
    progress: Option<f64>,
) -> Prompt {
    Prompt {
        id: id.into(),
        kind,
        title: title.into(),
        message: message.into(),
        options,
        cancellable,
        progress,
    }
}

fn yesno_message(body: &str, custom: &str) -> String {
    let body = body.trim();
    let custom = custom.trim();
    if custom.is_empty() {
        body.to_string()
    } else if body.is_empty() {
        custom.to_string()
    } else {
        format!("{body}\n{custom}")
    }
}

fn option_text(label: &str, label2: &str) -> String {
    let label = plain_block(label);
    let label2 = plain_block(label2);
    if label2.is_empty() {
        label.to_string()
    } else {
        format!("{label} \u{2014} {label2}")
    }
}

/// One skin list row: the first value names it, the rest follow as details.
/// A row with no text at all is numbered so it can still be chosen.
fn row_text(values: &[String], index: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    for value in values {
        let text = plain_block(value).trim().to_string();
        let placeholder = matches!(
            text.to_ascii_lowercase().as_str(),
            "" | "none" | "null" | "n/a" | "na"
        );
        if !placeholder && !parts.contains(&text) {
            parts.push(text);
        }
    }
    if parts.is_empty() {
        return format!("Item {}", index + 1);
    }
    let name = parts.remove(0);
    option_text(&name, &parts.join(" | "))
}

fn secret_title(title: &str) -> bool {
    let title = title.to_ascii_lowercase();
    // "PIN" only as a word, so a title such as "Opinion" still shows its value.
    let pin = title
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|word| word == "pin");
    title.contains("password") || pin || title.contains("api key") || title.contains("token")
}

fn show(text: &str) -> String {
    plain_block(text)
}

fn field(map: &Value, key: &str) -> String {
    match map.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

/// Busy windows, found in the installed add-ons by their
/// shape. A key sent while one of these is up is delivered to it, and a
/// resolve window closes on Select or Back.
fn resolve_keys() -> Value {
    json!({ "booleans": overlay_keys() })
}

/// True when any overlay window asked about is open.
fn resolve_skin_open(flags: &Value) -> bool {
    flags
        .as_object()
        .is_some_and(|map| map.keys().any(|key| flag(flags, key)))
}

fn skin_file(key: &str) -> &str {
    key.strip_prefix("Window.IsActive(")
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or(key)
}

fn is_resolve_skin(file: &str) -> bool {
    busy_overlay_file(file)
        || overlay_keys()
            .iter()
            .any(|key| skin_file(key).eq_ignore_ascii_case(file))
}

/// True when answering `file` would deliver the key to an open resolve window.
fn answer_would_hit_resolve(file: &str, flags: &Value) -> bool {
    !is_resolve_skin(file) && resolve_skin_open(flags)
}

/// Select on a focused list is the pick. Select on an unfocused list while a
/// resolve window is open lands on that window and cancels the play.
fn select_would_cancel_resolve(list_focused: bool, file: &str, flags: &Value) -> bool {
    !list_focused && answer_would_hit_resolve(file, flags)
}

fn answer_missed() -> EngineError {
    EngineError::BadRequest("That answer did not go through.".into())
}

fn flag(map: &Value, key: &str) -> bool {
    match map.get(key) {
        Some(Value::Bool(value)) => *value,
        Some(Value::String(text)) => {
            let text = text.trim();
            text.eq_ignore_ascii_case("true") || text == "1"
        }
        Some(Value::Number(number)) => number.as_i64() == Some(1),
        _ => false,
    }
}

fn count_of(text: &str) -> usize {
    parse_i64(text)
        .map(|value| value.max(0) as usize)
        .unwrap_or(0)
}

fn parse_i64(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = text.parse::<i64>() {
        return Some(value);
    }
    let value = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
    Some(value as i64)
}

fn data_str(params: &Value, key: &str) -> String {
    params
        .get("data")
        .and_then(|data| data.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/prompts.rs"]
mod tests;
