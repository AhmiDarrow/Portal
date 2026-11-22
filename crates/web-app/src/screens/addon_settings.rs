//! One add-on's settings, saved per field (main.ts:2377-2661 `renderSettings`,
//! `paintSettings`, `fieldHtml`, `wireSettings`; plan P12). Categories as pill buttons, then
//! one card of rows: the name on the left, the control on the right (stacked under 720 px
//! unless compact), a status line under both. Actions run in Kodi and their dialogs show
//! here: questions as the prompt modal, progress in the row's status line.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use portal_api::{FieldKind, Prompt, PromptAction, PromptAnswer, PromptKind, SettingsDoc, SettingsField};
use remedy_ui::layout::{Align, Display, Justify, Length, Track};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{LineHeight, States, StylePatch, StyleSheet};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use super::prompts::{self, Outcome};
use super::source;
use crate::api::{self, shown_detail};
use crate::pages::Page;
use crate::theme::{mix, Look};
use crate::widgets::activity::{describe_action, describe_settings, Owner};
use crate::widgets::page::h1;
use crate::widgets::sections;
use crate::Ctx;

/// A label as Kodi would show it, without its markup (settings-link.ts `plainSettingLabel`):
/// `[B]`, `[COLOR x]` and the like go, `[CR]` and `[TABS]` become spaces, and every run of
/// spaces is one.
pub fn plain_setting_label(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(i) = rest.find('[') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        match tail[1..].find(']').map(|e| &tail[..e + 2]) {
            Some(tok) if tok.eq_ignore_ascii_case("[CR]") || tok.eq_ignore_ascii_case("[TABS]") => {
                out.push(' ');
                rest = &tail[tok.len()..];
            }
            Some(tok) if is_marker(tok) => rest = &tail[tok.len()..],
            _ => {
                out.push('[');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out.replace('\u{a4}', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `[/?Name]` or `[Name value]` / `[Name=value]`.
fn is_marker(tok: &str) -> bool {
    let inner = &tok[1..tok.len() - 1];
    let inner = inner.strip_prefix('/').unwrap_or(inner);
    let mut chars = inner.char_indices();
    if !chars.next().is_some_and(|(_, c)| c.is_ascii_alphabetic()) {
        return false;
    }
    match chars.find(|(_, c)| !c.is_ascii_alphanumeric()) {
        None => true,
        Some((_, c)) => c == ' ' || c == '=',
    }
}

/// main.ts `displayName`: the plain label, or `fallback` when nothing is left.
pub fn display_name(value: &str, fallback: &str) -> String {
    let p = plain_setting_label(value);
    if p.is_empty() { fallback.to_owned() } else { p }
}

/// A label as a person reads it (main.ts `tidyLabel`): no "-- Help --" decoration, a
/// capital first letter.
pub fn tidy_label(value: &str) -> String {
    let plain = plain_setting_label(value);
    let lead = |c: char| "-=*_~>#".contains(c);
    let trail = |c: char| "-=*_~<#".contains(c);
    let mut bare = plain.trim_start();
    let n = bare.chars().take_while(|c| lead(*c)).count();
    if n >= 2 {
        bare = bare[bare.char_indices().nth(n).map_or(bare.len(), |(i, _)| i)..].trim_start();
    }
    let mut b = bare.trim_end();
    let n = b.chars().rev().take_while(|c| trail(*c)).count();
    if n >= 2 {
        let cut = b.char_indices().rev().nth(n - 1).map_or(0, |(i, _)| i);
        b = b[..cut].trim_end();
    }
    let text = if b.trim().is_empty() { plain.as_str() } else { b.trim() };
    let mut cs = text.chars();
    cs.next().map(|f| f.to_uppercase().chain(cs).collect()).unwrap_or_default()
}

/// What Portal and the old page show on a phone, tablet or TV for Source and add-on
/// settings (main.ts:366).
pub fn remote_view(ui: &Ui, ctx: &Ctx) -> El {
    w::column(ui).fill_width().child(h1(ui, "Source")).child(
        w::row(ui)
            .layout(|l| l.wrap = true)
            .child(w::text(ui, "Add-ons and their settings change only on the PC running Portal. ").variant("muted"))
            .child(crate::widgets::link(ui, &ctx.router, &Page::Server.path(), "Settings", "", None))
            .child(w::text(ui, " for this screen are open here.").variant("muted")),
    )
}

thread_local! {
    /// The add-on and category last shown (main.ts `categoryAddon`, `categoryIndex`).
    static CATEGORY: RefCell<(String, usize)> = RefCell::new((String::new(), 0));
}

/// The page's state: the form as loaded, a value per setting (shown in every category and
/// kept across category switches), and the line under each row.
struct Form {
    ui: Ui,
    ctx: Ctx,
    doc: SettingsDoc,
    values: HashMap<String, Signal<String>>,
    route: remedy_ui::router::Route,
}

#[derive(Clone, PartialEq)]
enum Note {
    Plain(String),
    /// An add-on dialog: links become links, a cancellable progress gets Cancel.
    Dialog(String, Option<Prompt>),
}

pub fn view(ui: &Ui, ctx: &Ctx, addon: &str) -> El {
    if ctx.is_remote() {
        return remote_view(ui, ctx);
    }
    let root = w::column(ui).fill_width();
    if addon.is_empty() {
        return root.child(
            w::row(ui)
                .child(w::text(ui, "Choose a video add-on first. ").variant("error"))
                .child(crate::widgets::link(ui, &ctx.router, &Page::Source.path(), "Go to Source", "", None)),
        );
    }
    let open = Rc::new(Cell::new(true));
    let ticket = ctx.activity.begin(Owner::Page, describe_settings(false), 0);
    source::note_kodi(ctx, open.clone(), describe_settings(true));
    // Opened from a link rather than Source: the add-on's name is read too.
    if source::known_addons().is_empty() {
        let c = ctx.clone();
        ctx.api.run(api::addons(), &Cancel::new(), move |out| {
            if let Ok(l) = out {
                source::note_addons(l.items);
                c.activity.notice(String::new());
            }
        });
    }
    let (u, c, r) = (ui.clone(), ctx.clone(), root.clone());
    let route = ctx.router.route_untracked();
    ctx.api.run(api::settings(addon), &Cancel::new(), move |out| {
        open.set(false);
        if c.router.route_untracked() != route {
            return;
        }
        c.activity.end(ticket);
        c.activity.end_page();
        match out {
            Ok(doc) => {
                CATEGORY.with(|k| {
                    let mut k = k.borrow_mut();
                    if k.0 != doc.addon_id {
                        *k = (doc.addon_id.clone(), 0);
                    }
                    if k.1 >= doc.categories.len() {
                        k.1 = 0;
                    }
                });
                let values = doc
                    .categories
                    .iter()
                    .flat_map(|cat| cat.fields.iter())
                    .filter_map(|f| f.id.clone().map(|id| (id, f.value.clone())))
                    .fold(HashMap::new(), |mut m, (id, v)| {
                        m.entry(id).or_insert_with(|| Signal::new(v));
                        m
                    });
                let form = Rc::new(Form { ui: u.clone(), ctx: c.clone(), doc, values, route });
                paint(&form, &r);
                // Rows follow the 720 px layout.
                let (w2, r2) = (c.width, r.clone());
                let was = Cell::new(c.width.get_untracked().unwrap_or(1280.0) <= 720.0);
                let _ = r.clone().bind(move || {
                    let now = w2.get() <= 720.0;
                    if was.replace(now) != now {
                        untrack(|| paint(&form, &r2));
                    }
                });
            }
            Err(e) => {
                let _ = r.clone().child(w::text(&u, shown_detail(&e)).variant("error"));
            }
        }
    });
    root
}

/// The heading, categories and the current category's rows.
fn paint(form: &Rc<Form>, root: &El) {
    let (ui, ctx) = (&form.ui, &form.ctx);
    let index = CATEGORY.with(|k| k.borrow().1);
    let phone = ctx.width.get_untracked().unwrap_or(1280.0) <= 720.0;
    let mut cats = sections::cats(ui, "Categories", true, phone).layout(|l| l.margin.bottom = 16.0);
    let mut current = None;
    for (i, cat) in form.doc.categories.iter().enumerate() {
        let (f2, r2) = (form.clone(), root.clone());
        let label = display_name(&cat.label, "Settings");
        let b = sections::cat_button(ui, &label, i == index, move || {
            CATEGORY.with(|k| k.borrow_mut().1 = i);
            paint(&f2, &r2);
        });
        if i == index {
            current = Some(b.clone());
        }
        cats = cats.child(b);
    }
    let name = source::known_addons().into_iter().find(|a| a.id == form.doc.addon_id).map(|a| plain_setting_label(&a.name)).unwrap_or_default();
    let title = if name.is_empty() { form.doc.addon_id.clone() } else { name };
    let kicker = w::row(ui)
        .layout(|l| {
            l.wrap = true;
            l.margin.top = 6.4;
            l.margin.bottom = 16.0;
        })
        .child(w::text(ui, "Add-on settings · ").variant("muted"))
        .child(crate::widgets::link(ui, &ctx.router, &Page::Source.path(), "Back to Source", "", None));
    let list = sections::card(ui, "Settings").layout(|l| {
        l.padding.top = 3.2;
        l.padding.bottom = 3.2;
    });
    let list = match form.doc.categories.get(index) {
        None => list.child(w::text(ui, "No settings in this category.").variant("muted")),
        Some(cat) => {
            let mut list = list;
            let mut prev_row = false;
            for (n, f) in cat.fields.iter().enumerate() {
                if f.kind == FieldKind::Header {
                    list = list.child(header(ui, &tidy_label(&f.label), n == 0));
                    prev_row = false;
                    continue;
                }
                if prev_row {
                    list = list.child(ui.el("p_setting_rule").height(1.0).fill_width());
                }
                list = list.child(row(form, f));
                prev_row = true;
            }
            list
        }
    };
    let had_focus = current.is_some() && ui.tree().focused().is_some_and(|f| ui.tree().descendants(root.id()).contains(&f));
    untrack(|| {
        root.clear();
        let _ = root.clone().child(h1(ui, &title)).child(kicker).child(cats).child(list);
    });
    // The repaint replaced the category button that had focus.
    if let (true, Some(b)) = (had_focus, current) {
        b.focus();
    }
}

fn header(ui: &Ui, text: &str, first: bool) -> El {
    let mut col = w::column(ui).fill_width();
    if !first {
        col = col.child(ui.el("p_setting_head_rule").height(1.0).fill_width().layout(|l| l.margin.top = 16.0));
    }
    col.child(w::heading(ui, 3, text.to_owned()).variant("p_setting_head").layout(move |l| {
        l.margin.top = if first { 4.8 } else { 9.6 };
        l.margin.bottom = 1.6;
    }))
}

/// One field as a row (main.ts `fieldHtml`).
fn row(form: &Rc<Form>, f: &SettingsField) -> El {
    let (ui, ctx) = (&form.ui, &form.ctx);
    let label = tidy_label(&f.label);
    let id = f.id.clone().unwrap_or_default();
    let note = Signal::new(Note::Plain(String::new()));
    let value = form.values.get(&id).copied().unwrap_or_else(|| Signal::new(f.value.clone()));
    let name = || w::text(ui, label.clone()).variant(if f.enabled { "" } else { "muted" }).layout(|l| l.min_width = Length::Px(0.0));
    let mut compact = false;
    let (name_el, control) = match f.kind {
        FieldKind::Bool => {
            compact = true;
            let on = Signal::new(value.get_untracked().unwrap_or_default() == "true");
            let (fm, i2) = (form.clone(), id.clone());
            let t = sections::toggle(ui, &label, on, f.enabled, move |now| {
                on.set(now);
                save(&fm, &i2, if now { "true" } else { "false" }, note);
            });
            // A value read again (after an action, or a failed save) shows.
            let t = t.bind(move || {
                let v = value.get() == "true";
                if on.get_untracked().ok() != Some(v) {
                    on.set(v);
                }
            });
            (name(), t)
        }
        FieldKind::Select => {
            let opts = f.options.clone();
            let labels: Vec<String> = opts.iter().map(|o| plain_setting_label(&o.label)).collect();
            let at = |v: &str| opts.iter().position(|o| o.value == v).unwrap_or(0);
            let chosen = Signal::new(at(&value.get_untracked().unwrap_or_default()));
            let s = sections::select(ui, &label, labels, chosen).disabled(!f.enabled).fill_width();
            let (fm, i2, o2) = (form.clone(), id.clone(), opts.clone());
            let s = s.bind(move || {
                let v = value.get();
                let i = o2.iter().position(|o| o.value == v).unwrap_or(0);
                if chosen.get_untracked().ok() != Some(i) {
                    chosen.set(i);
                }
            });
            let s = s.bind(move || {
                let Some(o) = opts.get(chosen.get()) else { return };
                if value.get_untracked().ok().as_deref() != Some(o.value.as_str()) {
                    let v = o.value.clone();
                    untrack(|| save(&fm, &i2, &v, note));
                }
            });
            (name(), s)
        }
        FieldKind::Action => action_control(form, f, &label, &id, value, note, &mut compact),
        FieldKind::Secret | FieldKind::Number | FieldKind::Text => {
            let secret = f.kind == FieldKind::Secret;
            let field = if secret { w::secret_input(ui, label.clone()).placeholder("Leave blank to keep it") } else { w::text_input(ui, label.clone()) };
            let field = if f.kind == FieldKind::Number { field.numeric() } else { field };
            if !secret {
                field.set_value(&f.value);
            }
            let (fm, i2, f2) = (form.clone(), id.clone(), field.clone());
            // `change`: Enter, or leaving the field with a different value.
            let commit = Rc::new(move || {
                if secret {
                    if f2.is_empty() {
                        return;
                    }
                    let v = f2.take_secret().expose().to_owned();
                    save(&fm, &i2, &v, note);
                    return;
                }
                let v = f2.value();
                if value.get_untracked().ok().as_deref() != Some(v.as_str()) {
                    save(&fm, &i2, &v, note);
                }
            });
            let c2 = commit.clone();
            let el = field.clone().on_submit(move || c2()).el().variant("portal_search").fill_width().disabled(!f.enabled);
            let f3 = field.clone();
            let el = el.on_focus_change(move |on| {
                if !on {
                    commit();
                }
            });
            // A value read again shows, unless the field is being typed in.
            let (e2, u2) = (el.clone(), ui.clone());
            let el = if secret {
                el
            } else {
                el.bind(move || {
                    let v = value.get();
                    if u2.tree().focused() != Some(e2.id()) && f3.value() != v {
                        f3.set_value(&v);
                    }
                })
            };
            (name(), el)
        }
        FieldKind::Header => unreachable!("headers are drawn by paint"),
    };
    let phone = ctx.width.get_untracked().unwrap_or(1280.0) <= 720.0;
    let stacked = phone && !compact;
    let ctrl = ui
        .el("p_setting_control")
        .row()
        .align(Align::Center)
        .justify(if stacked { Justify::Start } else { Justify::End })
        .gap(9.6)
        .layout(|l| l.min_width = Length::Px(0.0))
        .child(control);
    let note_el = note_view(ui, ctx, note).layout(|l| l.grid_column = Some((1, 2)));
    let cols = if stacked {
        vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0)))]
    } else if compact {
        vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0))), Track::MaxContent]
    } else {
        vec![Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Fr(1.0))), Track::MinMax(Box::new(Track::Px(0.0)), Box::new(Track::Px(288.0)))]
    };
    let note_el = if stacked { note_el.layout(|l| l.grid_column = None) } else { note_el };
    ui.el("p_setting")
        .layout(move |l| {
            l.display = Display::Grid;
            l.columns = cols.clone();
            l.gap = (16.0, 4.8);
            l.align_items = Align::Center;
            l.padding.top = 9.6;
            l.padding.bottom = 9.6;
        })
        .fill_width()
        .child(name_el)
        .child(ctrl)
        .child(note_el)
}

/// The three kinds of action row: another add-on's settings, a plugin link to run, or the
/// add-on's own action (what it stored shows beside its button).
fn action_control(form: &Rc<Form>, f: &SettingsField, label: &str, id: &str, value: Signal<String>, note: Signal<Note>, compact: &mut bool) -> (El, El) {
    let (ui, ctx) = (&form.ui, &form.ctx);
    let name = w::text(ui, label.to_owned()).layout(|l| l.min_width = Length::Px(0.0));
    *compact = true;
    if let Some(other) = crate::widgets::shows::settings_addon(&f.action) {
        let link = crate::widgets::link(ui, &ctx.router, &Page::AddonSettings(other).path(), "Open settings", "action", None);
        return (name, link);
    }
    let busy = Signal::new(false);
    if id.is_empty() && !f.action.is_empty() {
        let (fm, path, l2) = (form.clone(), f.action.clone(), label.to_owned());
        let b = crate::widgets::button(ui, "Run", move || {
            if busy.get_untracked().unwrap_or(true) {
                return;
            }
            busy.set(true);
            note.set(Note::Plain(String::new()));
            let fm2 = fm.clone();
            run_plugin_row(&fm, &l2, &path, note, move || {
                let _ = busy.try_set(false);
                refresh(&fm2);
            });
        });
        let enabled = f.enabled;
        return (name, b.disabled(move || busy.get() || !enabled));
    }
    // The stored value needs room beside its button; a bare button does not.
    if !plain_setting_label(&f.value).is_empty() {
        *compact = false;
    }
    let shown = ui.el("p_setting_value").layout(|l| l.min_width = Length::Px(0.0));
    let s2 = shown.clone();
    let shown = shown.bind(move || {
        let v = plain_setting_label(&value.get());
        let _ = s2.clone().text(v.clone()).hidden(v.is_empty());
    });
    let (fm, i2, l2) = (form.clone(), id.to_owned(), label.to_owned());
    let b = crate::widgets::button(ui, move || if plain_setting_label(&value.get()).is_empty() { "Run".to_owned() } else { "Change".to_owned() }, move || {
        // An add-on action can take a while; a second press would run it twice.
        if busy.get_untracked().unwrap_or(true) {
            return;
        }
        busy.set(true);
        let fm2 = fm.clone();
        run_action(&fm, &i2, &l2, note, move || {
            let _ = busy.try_set(false);
            refresh(&fm2);
        });
    });
    let enabled = f.enabled;
    let b = b.disabled(move || busy.get() || !enabled).layout(|l| l.shrink = 0.0);
    (name, w::row(ui).align(Align::Center).gap(9.6).layout(|l| l.min_width = Length::Px(0.0)).child(shown).child(b))
}

/// The line under a row (`.note`): plain text, or an add-on dialog's words with its https
/// addresses as links and Cancel for a progress Kodi lets the user cancel.
fn note_view(ui: &Ui, ctx: &Ctx, note: Signal<Note>) -> El {
    let el = ui.el("p_setting_note").column().fill_width().semantics(remedy_ui::a11y::Semantics::new(remedy_ui::a11y::Role::Status));
    let (e2, u, c) = (el.clone(), ui.clone(), ctx.clone());
    el.bind(move || {
        let n = note.get();
        untrack(|| {
            e2.clear();
            let (text, prompt, links) = match n {
                Note::Plain(t) => (t, None, false),
                Note::Dialog(t, p) => (t, p, true),
            };
            let _ = e2.clone().hidden(text.is_empty() && prompt.is_none());
            for line in text.split('\n') {
                let mut row = w::row(&u).layout(|l| l.wrap = true);
                let parts = if links { split_links(line) } else { vec![(line.to_owned(), false)] };
                for (part, is_link) in parts {
                    row = if is_link {
                        let (a, href) = (c.app.clone(), part.trim_end_matches(['.', ',', ';', ':', '!', '?', ')']).to_owned());
                        row.child(w::link(&u, part, move || a.open_url(&href, |_| {})).variant("p_note_link"))
                    } else {
                        row.child(ui_text(&u, part))
                    };
                }
                let _ = e2.clone().child(row);
            }
            if let Some(p) = prompt.filter(|p| p.kind == PromptKind::Progress && p.cancellable) {
                let (c2, done) = (c.clone(), Signal::new(false));
                let b = crate::widgets::button(&u, "Cancel", move || {
                    done.set(true);
                    let body = PromptAnswer { id: p.id.clone(), action: PromptAction::Cancel, text: None, index: None };
                    c2.api.run(api::answer_prompt(&body), &Cancel::new(), move |out| {
                        if let Err(e) = out {
                            note.set(Note::Plain(shown_detail(&e)));
                        }
                    });
                })
                .disabled(move || done.get())
                .layout(|l| {
                    l.align_self = Some(Align::Start);
                    l.margin.top = 4.0;
                });
                let _ = e2.clone().child(b);
            }
        });
    })
}

fn ui_text(ui: &Ui, s: String) -> El {
    ui.el("p_setting_note_text").text(s)
}

/// Splits `text` around its https addresses (main.ts `paintActionNote`).
pub fn split_links(text: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("https://") {
        if i > 0 {
            out.push((rest[..i].to_owned(), false));
        }
        let tail = &rest[i..];
        let end = tail.find(|c: char| c.is_whitespace() || "<>\"'".contains(c)).unwrap_or(tail.len());
        out.push((tail[..end].to_owned(), true));
        rest = &tail[end..];
    }
    if !rest.is_empty() {
        out.push((rest.to_owned(), false));
    }
    out
}

/// Saves one value (main.ts `saveSetting`); the shown value changes at once and goes back
/// when the save fails, unless a later change already replaced it.
fn save(form: &Rc<Form>, id: &str, value: &str, note: Signal<Note>) {
    let sig = form.values.get(id).copied();
    let before = sig.and_then(|s| s.get_untracked().ok());
    let secret = form.doc.categories.iter().flat_map(|c| c.fields.iter()).any(|f| f.id.as_deref() == Some(id) && f.kind == FieldKind::Secret);
    if let (Some(s), false) = (sig, secret) {
        s.set(value.to_owned());
    }
    note.set(Note::Plain("Saving…".into()));
    let v = value.to_owned();
    form.ctx.api.run(api::write_setting(&form.doc.addon_id, id, value), &Cancel::new(), move |out| match out {
        Ok(_) => {
            let _ = note.try_set(Note::Plain("Saved".into()));
        }
        Err(e) => {
            let _ = note.try_set(Note::Plain(shown_detail(&e)));
            if let (Some(s), Some(b), false) = (sig, before, secret) {
                if s.get_untracked().ok().as_deref() == Some(v.as_str()) {
                    let _ = s.try_set(b);
                }
            }
        }
    });
}

/// An action is the add-on changing its own settings: the form is read again and every
/// field not being typed in shows what it stored (main.ts `refreshSettings`).
fn refresh(form: &Rc<Form>) {
    let f2 = form.clone();
    form.ctx.api.run(api::settings(&form.doc.addon_id), &Cancel::new(), move |out| {
        let Ok(fresh) = out else { return };
        for f in fresh.categories.iter().flat_map(|c| c.fields.iter()) {
            let Some(id) = &f.id else { continue };
            if matches!(f.kind, FieldKind::Secret | FieldKind::Header) {
                continue;
            }
            if let Some(s) = f2.values.get(id) {
                if s.get_untracked().ok().as_deref() != Some(f.value.as_str()) {
                    let _ = s.try_set(f.value.clone());
                }
            }
        }
    });
}

/// A plugin link the row runs (`data-run`): as a Run row of a folder, its dialogs shown in
/// the row's line.
fn run_plugin_row(form: &Rc<Form>, label: &str, path: &str, note: Signal<Note>, done: impl FnOnce() + 'static) {
    let (ui, ctx) = (form.ui.clone(), form.ctx.clone());
    let ticket = ctx.activity.begin(Owner::Page, describe_action(Some(label)), 0);
    let c = ctx.clone();
    ctx.api.run(api::run_plugin(path), &Cancel::new(), move |out| match out {
        Ok(r) if r.ran => {
            c.activity.notice("Opening…");
            let c2 = c.clone();
            let n: prompts::OnNote = Rc::new(move |text, _| c2.activity.notice(text));
            let c3 = c.clone();
            prompts::await_addon_prompts(&ui, &c, n, move |o| {
                c3.activity.notice(if o == Outcome::Idle { "The add-on did not open a dialog." } else { "" });
                c3.activity.end(ticket);
                done();
            });
        }
        Ok(_) => {
            c.activity.notice("That item runs an add-on action, not a video.");
            c.activity.end(ticket);
            done();
        }
        Err(e) => {
            c.activity.notice(shown_detail(&e));
            c.activity.end(ticket);
            let _ = note;
            done();
        }
    });
}

/// The add-on's own action (main.ts `runAction`).
fn run_action(form: &Rc<Form>, id: &str, label: &str, note: Signal<Note>, done: impl FnOnce() + 'static) {
    let (ui, ctx) = (form.ui.clone(), form.ctx.clone());
    let ticket = ctx.activity.begin(Owner::Page, describe_action(Some(label)), 0);
    let here = form.route.clone();
    let c = ctx.clone();
    ctx.api.run(api::action(&form.doc.addon_id, id), &Cancel::new(), move |out| {
        let r = match out {
            Ok(r) => r,
            Err(e) => {
                let _ = note.try_set(Note::Plain(shown_detail(&e)));
                c.activity.end(ticket);
                return done();
            }
        };
        if let Some(addon) = r.settings_addon.filter(|a| !a.is_empty()) {
            c.activity.notice("");
            c.go(&Page::AddonSettings(addon));
            return done();
        }
        if let Some(path) = r.browse_path.filter(|p| !p.is_empty()) {
            c.go(&Page::Browse(path));
            return done();
        }
        if !r.ran {
            let _ = note.try_set(Note::Plain("Done".into()));
            c.activity.end(ticket);
            return done();
        }
        let _ = note.try_set(Note::Plain("Opening…".into()));
        let n: prompts::OnNote = Rc::new(move |text, p| {
            let _ = note.try_set(Note::Dialog(text, p));
        });
        let c2 = c.clone();
        prompts::await_addon_prompts(&ui, &c, n, move |o| {
            // Settings for another add-on opened: that page takes over. The same add-on's
            // settings are this page, which stays.
            if o == Outcome::Settings && c2.router.route_untracked() != here {
                return done();
            }
            let _ = note.try_set(Note::Plain(if o == Outcome::Idle { "The add-on did not open a dialog.".into() } else { "Done".into() }));
            c2.activity.end(ticket);
            done();
        });
    });
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    s.rule("p_setting_rule", None, States::NONE, StylePatch::default().background(mix(k.line, 0.75, remedy_ui::core::Color::TRANSPARENT)));
    s.rule("p_setting_head_rule", None, States::NONE, StylePatch::default().background(k.line));
    s.rule(
        "heading",
        Some("p_setting_head"),
        States::NONE,
        StylePatch::default().heading(true).weight(400.0).text_size(16.0).foreground(k.gold).line_height(LineHeight::Relative(1.4)),
    );
    s.rule("p_setting_note", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.4));
    s.rule("p_setting_note_text", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.4));
    s.rule("p_setting_value", None, States::NONE, StylePatch::default().foreground(k.muted).text_size(14.4));
    // a.action-link: a button that is a link.
    let pad = remedy_ui::core::Insets { top: 6.72, bottom: 6.72, start: 12.0, end: 12.0 };
    s.rule(
        "p_link",
        Some("action"),
        States::NONE,
        StylePatch::default().foreground(k.text).background(k.bg).border(k.line).border_width(1.0).radius(8.0).padding(pad),
    );
    s.rule("p_link", Some("action"), States::HOVER, StylePatch::default().border(k.gold));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_as_settings_link_ts() {
        assert_eq!(plain_setting_label("[B]Bold[/B] [COLOR red]red[/COLOR]"), "Bold red");
        assert_eq!(plain_setting_label("a[CR]b[tabs]c"), "a b c");
        assert_eq!(plain_setting_label("[1] keep [x"), "[1] keep [x");
        assert_eq!(plain_setting_label("  \u{a4} spaced  out "), "spaced out");
        assert_eq!(tidy_label("-- help --"), "Help");
        assert_eq!(tidy_label("==Advanced=="), "Advanced");
        assert_eq!(tidy_label("-1 = unlimited"), "-1 = unlimited");
        assert_eq!(tidy_label("----"), "----");
        assert_eq!(display_name(" ", "Add-on"), "Add-on");
    }

    #[test]
    fn links_split_out() {
        assert_eq!(
            split_links("Go to https://x.tv/link. Code: 12"),
            vec![("Go to ".into(), false), ("https://x.tv/link.".into(), true), (" Code: 12".into(), false)]
        );
        assert_eq!(split_links("none"), vec![("none".into(), false)]);
    }
}
