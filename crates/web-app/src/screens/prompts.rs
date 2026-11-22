//! Add-on prompts (player.ts:2438-2685): the dialog Kodi shows while an add-on works, as a
//! modal in Portal's theme. Text (a password field for a secret; a file from this PC for a
//! link or file, uploaded to `/api/incoming`), a list to pick from, Yes/No, Ok, and Cancel
//! when the add-on allows it. Tab stays inside, Enter in the field submits, Escape cancels,
//! and focus goes back where it was. Progress dialogs are not shown here: the player's stage
//! and the activity panel say them.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use portal_api::{Prompt, PromptAction, PromptAnswer, PromptKind};
use remedy_ui::a11y::{Action, Role, Semantics};
use remedy_ui::core::{Color, Insets};
use remedy_ui::host::FileDialog;
use remedy_ui::input::Key;
use remedy_ui::layout::{Align, Justify, Length, Position};
use remedy_ui::reactive::Cancel;
use remedy_ui::style::{LineHeight, States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use crate::theme::{mix, Look};
use crate::{api, Ctx};

/// What the page does around an answer: show an error, and act once it went through.
#[derive(Clone)]
pub struct Hooks {
    /// A sentence to show (the answer or an upload failed).
    pub on_error: Rc<dyn Fn(String)>,
    /// After the answer was sent (whether it went through or not), with what it was.
    pub after: Rc<dyn Fn(&Prompt, PromptAction)>,
}

struct Shown {
    id: String,
    layer: El,
    back: Option<El>,
}

thread_local! {
    static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    /// True while an answer is on its way: Kodi's dialog stays up until Portal has worked it
    /// through, so a poll meanwhile still reports it, and it must not take a second answer.
    static ANSWERING: Cell<bool> = const { Cell::new(false) };
}

/// A title that asks for a secret (player.ts `secretTitle`).
pub fn secret_title(title: &str) -> bool {
    let t = title.to_lowercase();
    ["password", "pin", "api key", "token"].iter().any(|w| t.contains(w))
}

/// A text question asking for a link or a file (player.ts `wantsLocalFile`).
pub fn wants_local_file(p: &Prompt) -> bool {
    if secret_title(&p.title) {
        return false;
    }
    let text = format!("{} {}", p.title, p.message).to_lowercase();
    let words: Vec<&str> = text.split(|c: char| !c.is_alphanumeric()).collect();
    ["link", "url", "zip", "path", "location", "package", "browse", "http"].iter().any(|k| text.contains(k))
        || words.contains(&"file")
}

/// Anything to read or pick (player.ts `promptHasWords`).
pub fn has_words(p: &Prompt) -> bool {
    !p.title.trim().is_empty() || !p.message.trim().is_empty() || p.options.iter().any(|o| !o.trim().is_empty())
}

/// What the stage says for a progress dialog (player.ts `progressNote`).
pub fn progress_note(p: &Prompt) -> String {
    let (t, m) = (p.title.trim(), p.message.trim());
    if !t.is_empty() && !m.is_empty() && t != m {
        return format!("{t}\n{m}");
    }
    [t, m, "Working…"].into_iter().find(|s| !s.is_empty()).unwrap_or("Working…").to_owned()
}

/// True while a question shows.
pub fn is_open() -> bool {
    SHOWN.with(|s| s.borrow().is_some())
}

/// Takes the question away; focus goes back where it was if it was inside.
pub fn hide() {
    let Some(s) = SHOWN.with(|s| s.borrow_mut().take()) else { return };
    let had_focus = s.layer.states().contains(States::FOCUS_WITHIN);
    s.layer.remove();
    if let (true, Some(back)) = (had_focus, s.back) {
        back.focus();
    }
}

/// Sends the answer (player.ts `answer`).
pub fn answer(ctx: &Ctx, p: &Prompt, action: PromptAction, text: Option<String>, index: Option<i32>, hooks: &Hooks) {
    if ANSWERING.with(Cell::get) {
        return;
    }
    hide();
    ANSWERING.with(|a| a.set(true));
    let (p2, h) = (p.clone(), hooks.clone());
    let body = PromptAnswer { id: p.id.clone(), action, text, index };
    ctx.api.run(api::answer_prompt(&body), &Cancel::new(), move |out| {
        ANSWERING.with(|a| a.set(false));
        if let Err(e) = out {
            (h.on_error)(e.detail);
        }
        (h.after)(&p2, action);
    });
}

/// Shows `p` (player.ts `renderPrompt`); a progress dialog or one with no words hides it.
/// The same question is not built again while it shows.
pub fn show(ui: &Ui, ctx: &Ctx, p: &Prompt, hooks: Hooks) {
    if p.kind == PromptKind::Progress || !has_words(p) {
        hide();
        return;
    }
    if ANSWERING.with(Cell::get) || SHOWN.with(|s| s.borrow().as_ref().is_some_and(|s| s.id == p.id)) {
        return;
    }
    // Under an open Info panel it could not be answered at all.
    if crate::screens::info::is_open() {
        crate::screens::info::close(false);
    }
    let back = SHOWN
        .with(|s| s.borrow_mut().take())
        .map(|s| {
            s.layer.remove();
            s.back
        })
        .unwrap_or_else(|| ui.tree().focused().map(|f| ui.element(f)));
    let heading = if p.title.trim().is_empty() { "The add-on is asking".to_owned() } else { p.title.trim().to_owned() };
    let mut card = ui
        .el("p_prompt_card")
        .column()
        .clip()
        .semantics(Semantics::new(Role::Dialog).label(heading.clone()))
        .child(ui.el("heading").variant("2").text(heading.clone()));
    if !p.message.is_empty() {
        card = card.child(ui.el("p_prompt_copy").text(p.message.clone()).layout(|l| l.margin.top = 5.6));
    }
    let mut actions = ui.el("p_prompt_actions").row().layout(|l| {
        l.margin.top = 12.8;
        l.wrap = true;
        l.gap = (8.0, 8.0);
        l.shrink = 0.0;
    });
    let mut first: Option<El> = None;
    let button = |label: &str, f: Rc<dyn Fn()>| crate::widgets::button(ui, label, move || f());
    match p.kind {
        PromptKind::Text => {
            let field = if secret_title(&p.title) { w::secret_input(ui, heading.clone()) } else { w::text_input(ui, heading.clone()) };
            let (c, pr, h, f2) = (ctx.clone(), p.clone(), hooks.clone(), field.clone());
            // A secret field keeps its text from `value()`; it is taken out once, here.
            let submit: Rc<dyn Fn()> = Rc::new(move || {
                let text = if f2.kind() == remedy_ui::view::FieldKind::Secret { f2.take_secret().expose().to_owned() } else { f2.value() };
                answer(&c, &pr, PromptAction::Submit, Some(text), None, &h);
            });
            let s2 = submit.clone();
            let input = field.on_submit(move || s2()).el().variant("portal_search").fill_width();
            first = Some(input.clone());
            card = card.child(input);
            actions = actions.child(button("Submit", submit));
            if wants_local_file(p) {
                card = card.child(file_button(ui, ctx, p, &hooks));
            }
        }
        PromptKind::Select => {
            let mut list = ui
                .el("p_prompt_list")
                .column()
                .gap(6.4)
                .scroll()
                .semantics(Semantics::new(Role::List).label(heading.clone()))
                .layout(|l| {
                    l.margin.top = 12.8;
                    l.grow = 1.0;
                    l.shrink = 1.0;
                    l.min_height = Length::Px(0.0);
                });
            for (i, label) in p.options.iter().enumerate() {
                let (c, pr, h) = (ctx.clone(), p.clone(), hooks.clone());
                let b = crate::widgets::button(ui, label.as_str(), move || answer(&c, &pr, PromptAction::Submit, None, Some(i as i32), &h))
                    .variant("option")
                    .fill_width()
                    .semantics(Semantics::new(Role::ListItem).label(label.clone()).action(Action::Click));
                first.get_or_insert(b.clone());
                list = list.child(b);
            }
            card = card.child(list);
        }
        PromptKind::Yesno => {
            for (label, action) in [("Yes", PromptAction::Yes), ("No", PromptAction::No)] {
                let (c, pr, h) = (ctx.clone(), p.clone(), hooks.clone());
                let b = button(label, Rc::new(move || answer(&c, &pr, action, None, None, &h)));
                first.get_or_insert(b.clone());
                actions = actions.child(b);
            }
        }
        PromptKind::Ok => {
            let (c, pr, h) = (ctx.clone(), p.clone(), hooks.clone());
            let b = button("Ok", Rc::new(move || answer(&c, &pr, PromptAction::Ok, None, None, &h)));
            first.get_or_insert(b.clone());
            actions = actions.child(b);
        }
        PromptKind::Progress => {}
    }
    if p.cancellable {
        let (c, pr, h) = (ctx.clone(), p.clone(), hooks.clone());
        let b = button("Cancel", Rc::new(move || answer(&c, &pr, PromptAction::Cancel, None, None, &h)));
        first.get_or_insert(b.clone());
        actions = actions.child(b);
    }
    card = card.child(actions);
    let (c, pr, h, cancellable) = (ctx.clone(), p.clone(), hooks.clone(), p.cancellable);
    let card = card.on_key(move |e| {
        // Escape cancels, like any dialog, when the add-on allows it.
        if !e.pressed || e.key != Key::Escape || !cancellable {
            return false;
        }
        answer(&c, &pr, PromptAction::Cancel, None, None, &h);
        true
    });
    let c2 = card.clone();
    let layer = ui
        .el("p_prompt")
        .column()
        .align(Align::Center)
        .justify(Justify::Center)
        // A modal question keeps Tab inside it, like the Info dialog.
        .focus_scope()
        .layout(|l| {
            l.position = Position::Absolute;
            l.inset = Insets::ZERO;
            l.width = Length::Percent(1.0);
            l.height = Length::Percent(1.0);
        })
        .padding(Insets::all(16.0))
        .child(card)
        .on_resize(move |size| {
            let _ = c2.clone().layout(move |l| {
                l.width = Length::Px(704.0_f32.min(size.width - 32.0).max(0.0));
                l.max_height = Length::Px((size.height - 32.0).max(0.0));
            });
        });
    let layer = ui.overlay(layer);
    if let Some(f) = first {
        f.focus();
    }
    SHOWN.with(|s| *s.borrow_mut() = Some(Shown { id: p.id.clone(), layer, back }));
}

/// "File on this PC": picks a file, uploads it, and answers with the path Kodi can read.
fn file_button(ui: &Ui, ctx: &Ctx, p: &Prompt, hooks: &Hooks) -> El {
    let busy = remedy_ui::reactive::Signal::new(false);
    let (c, pr, h) = (ctx.clone(), p.clone(), hooks.clone());
    let b = crate::widgets::button(ui, "File on this PC", move || {
        if busy.get_untracked().unwrap_or(true) {
            return;
        }
        let mut d = FileDialog::default();
        d.title = "File on this PC".into();
        d.filters = vec![("Add-on files".into(), vec!["zip".into(), "xml".into(), "json".into(), "txt".into()])];
        let (c2, pr2, h2) = (c.clone(), pr.clone(), h.clone());
        c.app.file_dialog(None, d, move |paths| {
            let Some(path) = paths.into_iter().next() else { return };
            let Some(bytes) = read_picked(&path) else {
                (h2.on_error)("That file could not be read.".into());
                return;
            };
            busy.set(true);
            let name = path.file_name().map_or_else(|| "file".to_owned(), |n| n.to_string_lossy().into_owned());
            let kind = content_type(&name);
            let (c3, pr3, h3) = (c2.clone(), pr2.clone(), h2.clone());
            c2.api.run(api::upload_incoming(&name, kind, bytes), &Cancel::new(), move |out| match out {
                Ok(saved) => answer(&c3, &pr3, PromptAction::Submit, Some(saved.path), None, &h3),
                Err(e) => {
                    let _ = busy.try_set(false);
                    (h3.on_error)(e.detail);
                }
            });
        });
    });
    b.disabled(move || busy.get())
}

fn read_picked(path: &PathBuf) -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = remedy_ui::web::files::read(path);
        remedy_ui::web::files::remove(path);
        bytes
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::fs::read(path).ok()
    }
}

fn content_type(name: &str) -> &'static str {
    match name.rsplit('.').next().map(str::to_ascii_lowercase).as_deref() {
        Some("zip") => "application/zip",
        Some("xml") => "text/xml",
        Some("json") => "application/json",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// How an add-on action's dialogs ended (player.ts `awaitAddonPrompts`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A dialog showed and closed.
    Prompted,
    /// No dialog came up.
    Idle,
    /// The add-on opened its settings: Portal's own form took over.
    Settings,
}

/// How long [`await_addon_prompts`] watches, in milliseconds: at most `limit` in all (a
/// device sign-in waits for its code), `quiet` after the last dialog, `idle` for a first
/// one, `settle` after one closed, polling every `poll`. [`set_watch`] changes them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Watch {
    pub limit: u64,
    pub quiet: u64,
    pub idle: u64,
    pub settle: u64,
    pub poll: u64,
}

impl Default for Watch {
    fn default() -> Self {
        Watch { limit: 15 * 60_000, quiet: 120_000, idle: 4_000, settle: 1_500, poll: 400 }
    }
}

thread_local! {
    static WATCH: Cell<Watch> = Cell::new(Watch::default());
}

pub fn set_watch(w: Watch) {
    WATCH.with(|c| c.set(w));
}

/// What an action's dialog says, for the page: the text, and the progress dialog when it
/// can be cancelled.
pub type OnNote = Rc<dyn Fn(String, Option<Prompt>)>;

/// Mirrors the dialogs an add-on action opens until they close (player.ts
/// `awaitAddonPrompts`): questions show as the modal, progress as `on_note`. The add-on
/// opening its settings in Kodi opens Portal's form for them instead.
pub fn await_addon_prompts(ui: &Ui, ctx: &Ctx, on_note: OnNote, done: impl FnOnce(Outcome) + 'static) {
    use remedy_ui::core::time::Instant;
    let w = WATCH.with(Cell::get);
    let started = Instant::now();
    let st = Rc::new(RefCell::new((w.quiet, false, None::<u64>, Some(Box::new(done) as Box<dyn FnOnce(Outcome)>))));
    fn tick(ui: Ui, ctx: Ctx, on_note: OnNote, started: remedy_ui::core::time::Instant, w: Watch, st: Rc<RefCell<(u64, bool, Option<u64>, Option<Box<dyn FnOnce(Outcome)>>)>>) {
        let ms = move || started.elapsed().as_millis() as u64;
        let finish = |st: &Rc<RefCell<(u64, bool, Option<u64>, Option<Box<dyn FnOnce(Outcome)>>)>>, o: Outcome| {
            if let Some(d) = st.borrow_mut().3.take() {
                d(o);
            }
        };
        if ms() >= st.borrow().0 {
            hide();
            let saw = st.borrow().1;
            finish(&st, if saw { Outcome::Prompted } else { Outcome::Idle });
            return;
        }
        let (u2, c2) = (ui.clone(), ctx.clone());
        ctx.api.run(api::prompt(), &Cancel::new(), move |out| {
            let again = |u: &Ui, c: &Ctx| {
                let (u3, c3, n3, s3) = (u.clone(), c.clone(), on_note.clone(), st.clone());
                u.after(std::time::Duration::from_millis(w.poll), move || tick(u3, c3, n3, started, w, s3));
            };
            let read = match out {
                Ok(r) => r,
                Err(_) => return again(&u2, &c2),
            };
            if let Some(addon) = read.settings_addon.filter(|a| !a.is_empty()) {
                hide();
                c2.go(&crate::pages::Page::AddonSettings(addon));
                finish(&st, Outcome::Settings);
                return;
            }
            let now = ms();
            match read.prompt {
                Some(p) => {
                    {
                        let mut s = st.borrow_mut();
                        s.1 = true;
                        s.2 = None;
                        s.0 = w.limit.min(s.0.max(now + w.quiet));
                    }
                    if p.kind == PromptKind::Progress {
                        hide();
                        on_note(progress_note(&p), Some(p));
                    } else if has_words(&p) {
                        let c3 = c2.clone();
                        let hooks = Hooks { on_error: Rc::new(move |e| c3.activity.notice(e)), after: Rc::new(|_, _| {}) };
                        show(&u2, &c2, &p, hooks);
                        let t = p.title.trim();
                        on_note(if t.is_empty() { "Waiting for your answer…".into() } else { t.to_owned() }, None);
                    } else {
                        hide();
                        on_note("Working…".into(), None);
                    }
                }
                None => {
                    hide();
                    let (saw, since) = (st.borrow().1, st.borrow().2);
                    if saw {
                        match since {
                            None => st.borrow_mut().2 = Some(now),
                            Some(t) if now - t > w.settle => return finish(&st, Outcome::Prompted),
                            Some(_) => {}
                        }
                    } else if now > w.idle {
                        return finish(&st, Outcome::Idle);
                    }
                }
            }
            again(&u2, &c2);
        });
    }
    tick(ui.clone(), ctx.clone(), on_note, started, w, st);
}

/// A Run row of a folder (main.ts `openRunItem` and `runAddonAction`): Kodi runs the
/// plugin link, then whatever dialog it opens shows here.
pub fn run_addon_action(ui: &Ui, ctx: &Ctx, label: &str, path: &str) {
    use crate::widgets::activity::{describe_action, Owner};
    if !path.to_ascii_lowercase().contains("plugin://") {
        ctx.activity.notice("That item runs an add-on action, not a video.");
        return;
    }
    if ctx.is_remote() {
        ctx.activity.notice("Add-on tools run only on the PC running Portal.");
        return;
    }
    let ticket = ctx.activity.begin(Owner::Page, describe_action(Some(label)), 0);
    let (u, c) = (ui.clone(), ctx.clone());
    ctx.api.run(api::run_plugin(path), &Cancel::new(), move |out| match out {
        Ok(r) if r.ran => {
            c.activity.notice("Opening…");
            let c2 = c.clone();
            let note: OnNote = Rc::new(move |text, _| c2.activity.notice(text));
            let c3 = c.clone();
            await_addon_prompts(&u, &c, note, move |o| {
                c3.activity.notice(if o == Outcome::Idle { "The add-on did not open a dialog." } else { "" });
                c3.activity.end(ticket);
            });
        }
        Ok(_) => {
            c.activity.notice("That item runs an add-on action, not a video.");
            c.activity.end(ticket);
        }
        Err(e) => {
            c.activity.notice(crate::api::shown_detail(&e));
            c.activity.end(ticket);
        }
    });
}

/// `.prompt` and `.prompt-card` (styles.css).
pub fn rules(s: &mut StyleSheet, k: &Look) {
    let clear = Color::TRANSPARENT;
    s.rule("p_prompt", None, States::NONE, StylePatch::default().background(mix(k.bg, 0.68, clear)).backdrop_blur(8.0));
    s.rule(
        "p_prompt_card",
        None,
        States::NONE,
        StylePatch::default()
            .background(k.raised)
            .border(k.line)
            .border_width(1.0)
            .radius(14.0)
            .padding(Insets { top: 18.4, bottom: 18.4, start: 19.2, end: 19.2 })
            .shadow(k.shadow_lift),
    );
    s.rule("p_prompt_copy", None, States::NONE, StylePatch::default().line_height(LineHeight::Relative(1.5)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prompt(kind: PromptKind, title: &str, message: &str) -> Prompt {
        Prompt { id: "1".into(), kind, title: title.into(), message: message.into(), options: vec![], cancellable: true, progress: None }
    }

    #[test]
    fn words_and_files_as_player_ts() {
        assert!(secret_title("Enter your API key"));
        assert!(wants_local_file(&prompt(PromptKind::Text, "Install from zip", "")));
        assert!(wants_local_file(&prompt(PromptKind::Text, "Pick a file", "")));
        assert!(!wants_local_file(&prompt(PromptKind::Text, "Profile name", "")));
        assert!(!wants_local_file(&prompt(PromptKind::Text, "Password for the zip", "")));
        assert!(!has_words(&prompt(PromptKind::Ok, " ", "")));
        assert_eq!(progress_note(&prompt(PromptKind::Progress, "Finding", "Link 2")), "Finding\nLink 2");
        assert_eq!(progress_note(&prompt(PromptKind::Progress, "", "")), "Working…");
    }
}
