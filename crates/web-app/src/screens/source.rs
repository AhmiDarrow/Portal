//! Source: the video add-ons and the one in use, add-on updates, repositories and their
//! add-ons, and installing a repository from a link (main.ts:1760-2364 `renderSource`,
//! `paintSource` and the functions under it; plan P12). On a phone, tablet or TV the page
//! says it changes only on the PC (main.ts:366).
//!
//! What the page knows lives across visits, as main.ts's module state does (the update
//! check is kept, an open repository opens again); a change that the server made (an
//! install, a removal, an update) reads and paints the whole page again (`render`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use portal_api::{AddonCard, AddonUpdate, InstallResult, RemoveResult, RepoAddon, RepoCard, Source};
use remedy_ui::image::{image_ext, Fit, ImageOptions, Source as ImgSource};
use remedy_ui::layout::{Align, Length};
use remedy_ui::reactive::{untrack, Cancel, Signal};
use remedy_ui::style::{States, StylePatch, StyleSheet};
use remedy_ui::view::{EnterKey, InputPurpose, TextField};
use remedy_ui::widgets as w;
use remedy_ui::{El, Ui};

use super::addon_settings::{display_name, remote_view};
use crate::api::{self, shown_detail};
use crate::pages::Page;
use crate::theme::{mix, Look};
use crate::widgets::activity::{
    describe_addon_install, describe_addon_update, describe_remove, describe_repository_install, describe_repository_open, describe_source, ActivityInput, Owner,
    Removed,
};
use crate::widgets::art::{art_url, letter_tile};
use crate::widgets::page::h1;
use crate::widgets::sections::{self, keep_focus};
use crate::Ctx;

#[derive(Default)]
struct St {
    source: Option<Source>,
    addons: Vec<AddonCard>,
    repos: Vec<RepoCard>,
    /// `None` until the first check (main.ts `addonUpdates === null`).
    updates: Option<Vec<AddonUpdate>>,
    updates_error: String,
    updates_checking: bool,
    update_flash: String,
    open_repo: String,
    open_repo_name: String,
    open_repo_items: Vec<RepoAddon>,
    /// The open repository's list: loading, or the error it gave.
    open_repo_state: RepoState,
    open_repo_modules: bool,
    /// A sentence for the repository note after the next paint (`sourceFlash`).
    flash: String,
    busy: bool,
}

#[derive(Clone, Default, PartialEq)]
enum RepoState {
    #[default]
    Loading,
    Ready,
    Error(String),
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
    static RENDER: Cell<u64> = const { Cell::new(0) };
    /// The open repository's Show/Hide modules button, as last painted.
    static MODS_BUTTON: RefCell<Option<El>> = const { RefCell::new(None) };
    /// The page shown, while it shows.
    static VIEW: RefCell<Option<Rc<View>>> = const { RefCell::new(None) };
}

fn st<R>(f: impl FnOnce(&mut St) -> R) -> R {
    ST.with(|s| f(&mut s.borrow_mut()))
}

/// The video add-ons as last read (main.ts `loadedAddons`), for the settings page's name.
pub fn known_addons() -> Vec<AddonCard> {
    st(|s| s.addons.clone())
}

pub fn note_addons(items: Vec<AddonCard>) {
    st(|s| {
        if s.addons.is_empty() {
            s.addons = items;
        }
    });
}

/// main.ts `noteKodi`: while `open`, says Kodi is being started when it is not running.
pub fn note_kodi(ctx: &Ctx, open: Rc<Cell<bool>>, down: ActivityInput) {
    let c = ctx.clone();
    ctx.api.run(api::status(), &Cancel::new(), move |out| {
        if let Ok(s) = out {
            if open.get() && s.state != portal_api::EngineState::Running {
                c.activity.begin(Owner::Page, down, 0);
            }
        }
    });
}

/// main.ts `addonIdOk`.
pub fn addon_id_ok(id: &str) -> bool {
    (1..=80).contains(&id.len()) && !id.contains("..") && id.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// The painted page: its parts repaint on their own signal.
struct View {
    ui: Ui,
    ctx: Ctx,
    loaded: Signal<u64>,
    updates: Signal<u64>,
    repos: Signal<u64>,
    videos: Signal<u64>,
    note: Signal<String>,
    installing: Signal<bool>,
    source_filter: TextField,
    repo_filter: TextField,
    repo_videos: El,
    root: El,
}

fn bump(s: Signal<u64>) {
    let _ = s.try_update(|v| *v += 1);
}

/// Runs `f` with the page when Source still shows (main.ts `route().page === "source"`).
fn with_view(f: impl FnOnce(&Rc<View>)) {
    if let Some(v) = VIEW.with(|v| v.borrow().clone()) {
        if Page::from_route(&v.ctx.router.route_untracked()) == Page::Source {
            f(&v);
        }
    }
}

pub fn view(ui: &Ui, ctx: &Ctx) -> El {
    if ctx.is_remote() {
        return remote_view(ui, ctx);
    }
    let root = w::column(ui).fill_width();
    VIEW.with(|v| *v.borrow_mut() = None);
    render(ui, ctx, &root);
    root
}

/// main.ts `renderSource`: reads the source, the add-ons and the repositories, then paints.
fn render(ui: &Ui, ctx: &Ctx, root: &El) {
    let token = RENDER.with(|r| {
        r.set(r.get() + 1);
        r.get()
    });
    let open = Rc::new(Cell::new(true));
    let ticket = ctx.activity.begin(Owner::Page, describe_source(false), 0);
    note_kodi(ctx, open.clone(), describe_source(true));
    type Parts = (Option<Option<Source>>, Option<Result<Vec<AddonCard>, String>>, Option<Result<Vec<RepoCard>, String>>);
    let parts: Rc<RefCell<Parts>> = Rc::default();
    let (u, c, r) = (ui.clone(), ctx.clone(), root.clone());
    let route = ctx.router.route_untracked();
    let finish = Rc::new(move |parts: &Rc<RefCell<Parts>>| {
        let (Some(source), Some(addons), Some(repos)) = ({ parts.borrow().0.clone() }, { parts.borrow().1.clone() }, { parts.borrow().2.clone() }) else { return };
        open.set(false);
        c.activity.end(ticket);
        if RENDER.with(Cell::get) != token || c.router.route_untracked() != route {
            return;
        }
        c.activity.end_page();
        let (addon_error, repo_error) = st(|s| {
            s.source = source;
            let ae = match addons {
                Ok(a) => {
                    s.addons = a;
                    String::new()
                }
                Err(e) => {
                    s.addons.clear();
                    e
                }
            };
            let re = match repos {
                Ok(a) => {
                    s.repos = a;
                    String::new()
                }
                Err(e) => {
                    s.repos.clear();
                    e
                }
            };
            if !s.open_repo.is_empty() && !s.repos.iter().any(|x| x.id == s.open_repo) {
                s.open_repo.clear();
                s.open_repo_name.clear();
                s.open_repo_items.clear();
            }
            (ae, re)
        });
        paint(&u, &c, &r, &addon_error, &repo_error);
        let (repo, busy) = st(|s| (s.open_repo.clone(), s.busy));
        if !repo.is_empty() && !busy {
            open_repo(&repo);
        }
    });
    let (p1, f1) = (parts.clone(), finish.clone());
    ctx.api.run(api::source(), &Cancel::new(), move |out| {
        p1.borrow_mut().0 = Some(out.ok());
        f1(&p1);
    });
    let (p2, f2) = (parts.clone(), finish.clone());
    ctx.api.run(api::addons(), &Cancel::new(), move |out| {
        p2.borrow_mut().1 = Some(out.map(|l| l.items).map_err(|e| shown_detail(&e)));
        f2(&p2);
    });
    let (p3, f3) = (parts, finish);
    ctx.api.run(api::repos(), &Cancel::new(), move |out| {
        p3.borrow_mut().2 = Some(out.map(|l| l.items).map_err(|e| shown_detail(&e)));
        f3(&p3);
    });
}

/// main.ts `render()` after a change: the page is read and painted again.
fn rerender() {
    with_view(|v| render(&v.ui, &v.ctx, &v.root));
}

/// A search-style field in a card (`.search input`).
fn filter_field(ui: &Ui, label: &str) -> TextField {
    w::text_input(ui, label).placeholder(label).purpose(InputPurpose::Search)
}

fn search_row(ui: &Ui, field: El) -> El {
    ui.el("p_src_search")
        .row()
        .align(Align::Center)
        .gap(8.0)
        .fill_width()
        .layout(|l| {
            l.wrap = true;
            l.margin.top = 6.4;
            l.margin.bottom = 9.6;
        })
        .child(field.variant("portal_search").layout(|l| {
            l.grow = 1.0;
            l.shrink = 1.0;
            l.basis = Length::Px(224.0);
            l.min_width = Length::Px(0.0);
            l.max_width = Length::Px(448.0);
        }))
}

/// A column that repaints when `tick` changes.
fn live(ui: &Ui, tick: Signal<u64>, keep: bool, build: impl Fn() -> Vec<El> + 'static) -> El {
    let c = ui.el("p_src_rows").column().fill_width();
    let (c2, u) = (c.clone(), ui.clone());
    c.bind(move || {
        tick.get();
        let paint = || {
            c2.clear();
            for e in build() {
                let _ = c2.clone().child(e);
            }
        };
        if keep {
            keep_focus(&u, &c2, paint);
        } else {
            untrack(paint);
        }
    })
}

/// main.ts `paintSource`.
fn paint(ui: &Ui, ctx: &Ctx, root: &El, addon_error: &str, repo_error: &str) {
    let phone = ctx.width.get_untracked().unwrap_or(1280.0) <= 720.0;
    let flash = st(|s| std::mem::take(&mut s.flash));
    let source_filter = filter_field(ui, "Filter video add-ons");
    let repo_filter = filter_field(ui, "Filter this repository");
    let repo_videos = ui.el("p_src_videos").column().fill_width();
    let v = Rc::new(View {
        ui: ui.clone(),
        ctx: ctx.clone(),
        loaded: Signal::new(0),
        updates: Signal::new(0),
        repos: Signal::new(0),
        videos: Signal::new(0),
        note: Signal::new(flash),
        installing: Signal::new(false),
        source_filter: source_filter.clone(),
        repo_filter: repo_filter.clone(),
        repo_videos: repo_videos.clone(),
        root: root.clone(),
    });
    VIEW.with(|x| *x.borrow_mut() = Some(v.clone()));

    // Video add-ons.
    let current = st(|s| s.source.as_ref().and_then(|x| x.name.clone()).filter(|n| !n.is_empty()));
    let current = match current {
        Some(n) => format!("In use: {}", display_name(&n, "Chosen add-on")),
        None => "No add-on in use yet. Choose Use on one below.".into(),
    };
    let lv = v.clone();
    let s1 = source_filter.clone().on_change({
        let t = v.loaded;
        move || bump(t)
    });
    let mut loaded = sections::card(ui, "Video add-ons")
        .child(sections::h2(ui, "Video add-ons"))
        .child(sections::para(ui, "Portal browses and plays from the add-on you use. Settings opens that add-on's own options.", "muted"))
        .child(sections::para(ui, current, ""));
    if !addon_error.is_empty() {
        loaded = loaded.child(sections::para(ui, addon_error, "error"));
    }
    let loaded = loaded.child(search_row(ui, s1.el())).child(live(ui, v.loaded, false, move || loaded_rows(&lv)));

    // Updates.
    let uv = v.clone();
    let check = crate::widgets::button(ui, "Check now", || check_updates(true)).variant("quiet").disabled(move || {
        uv.updates.get();
        st(|s| s.updates_checking)
    });
    let uv = v.clone();
    let updates = sections::card(ui, "Updates")
        .child(
            w::row(ui)
                .align(Align::Center)
                .gap(9.6)
                .fill_width()
                .layout(|l| {
                    l.wrap = true;
                    l.margin.bottom = 9.6;
                })
                .child(w::heading(ui, 2, "Updates").layout(|l| l.grow = 1.0))
                .child(check),
        )
        .child(sections::para(ui, "Newer versions of the add-ons and modules on this PC, from their repositories.", "muted"))
        .child(live(ui, v.updates, false, move || update_rows(&uv)));

    // Repositories.
    let rv = v.clone();
    let vv = v.clone();
    let r1 = repo_filter.clone().on_change({
        let t = v.videos;
        move || bump(t)
    });
    let vv2 = v.clone();
    let videos = repo_videos
        .clone()
        .child(ui.el("p_src_rule").height(1.0).fill_width().layout(|l| {
            l.margin.top = 12.8;
            l.margin.bottom = 9.6;
        }))
        .child(live(ui, v.videos, false, move || {
            let name = st(|s| if s.open_repo_name.is_empty() { "Repository".to_owned() } else { s.open_repo_name.clone() });
            vec![w::heading(&vv2.ui, 3, name)]
        }))
        .child(search_row(ui, r1.el()))
        .child(live(ui, v.videos, false, move || video_rows(&vv)));
    let (rvv, vt) = (repo_videos.clone(), v.videos);
    let videos = videos.bind(move || {
        vt.get();
        let _ = rvv.clone().hidden(st(|s| s.open_repo.is_empty()));
    });
    let mut repos = sections::card(ui, "Repositories").child(sections::h2(ui, "Repositories")).child(sections::para(
        ui,
        "Open a repository to install video add-ons from it, and any module they need. Removing a repository keeps the add-ons installed from it.",
        "muted",
    ));
    if !repo_error.is_empty() {
        repos = repos.child(sections::para(ui, repo_error, "error"));
    }
    let repos = repos.child(live(ui, v.repos, true, move || repo_rows(&rv))).child(videos);

    // Add a repository.
    let url = w::text_input(ui, "Repository site or zip link").placeholder("Site or zip link").purpose(InputPurpose::Url).enter_key(EnterKey::Next);
    let name = w::text_input(ui, "Name (optional)").placeholder("Name (optional)").enter_key(EnterKey::Go);
    let (u2, n2, v2) = (url.clone(), name.clone(), v.clone());
    let submit = Rc::new(move || {
        let (link, label) = (u2.value(), n2.value());
        // `required`: an empty link is not sent.
        if link.trim().is_empty() {
            v2.note.set("Paste a link to the repository's site or its zip file.".into());
            u2.el().focus();
            return;
        }
        submit_zip(&v2, &link, &label);
    });
    let (s2, s3) = (submit.clone(), submit.clone());
    let n3 = name.clone();
    let url_el = url.on_submit(move || n3.el().focus()).el();
    let name_el = name.on_submit(move || s2()).el();
    let iv = v.installing;
    let install = crate::widgets::button(ui, "Install", move || s3()).disabled(move || iv.get());
    let field = |e: El, basis: f32| {
        e.variant("portal_search").layout(move |l| {
            l.grow = 1.0;
            l.shrink = 1.0;
            l.basis = Length::Px(basis);
            l.min_width = Length::Px(0.0);
            l.max_width = Length::Px(448.0);
        })
    };
    let nv = v.note;
    let add = sections::card(ui, "Add a repository")
        .child(sections::h2(ui, "Add a repository"))
        .child(sections::para(ui, "Paste a link to the repository's site or its zip file. The name is optional.", "muted"))
        .child(
            ui.el("p_src_search")
                .row()
                .align(Align::Center)
                .gap(8.0)
                .fill_width()
                .layout(|l| {
                    l.wrap = true;
                    l.margin.top = 6.4;
                    l.margin.bottom = 9.6;
                })
                .child(field(url_el, 224.0))
                .child(field(name_el, 224.0))
                .child(install),
        )
        .child(
            w::text(ui, "")
                .variant("muted")
                .text(move || remedy_ui::view::RichText::plain(nv.get()))
                .semantics(remedy_ui::a11y::Semantics::new(remedy_ui::a11y::Role::Status))
                .layout(|l| l.margin.bottom = 5.6),
        );

    let jump = sections::jump_bar(
        ui,
        phone,
        &[("Video add-ons", loaded.clone()), ("Updates", updates.clone()), ("Repositories", repos.clone()), ("Add a repository", add.clone())],
    );
    untrack(|| {
        root.clear();
        let _ = root.clone().child(h1(ui, "Source")).child(jump).child(loaded).child(updates).child(repos).child(add);
    });
    if st(|s| s.updates.is_none()) {
        check_updates(false);
    }
}

/// `.addon`: a row on the card's well; the name grows, the buttons wrap under it on a
/// narrow screen.
fn addon_row(ui: &Ui, current: bool, art: Option<El>, name: El, lines: Vec<El>, actions: Vec<El>) -> El {
    let mut grow = w::column(ui)
        .layout(|l| {
            l.grow = 1.0;
            l.shrink = 1.0;
            l.basis = Length::Px(176.0);
            l.min_width = Length::Px(0.0);
        })
        .child(name);
    for l in lines {
        grow = grow.child(l);
    }
    let mut acts = ui.el("p_row_actions").row().align(Align::Center).gap(5.6).layout(|l| {
        l.wrap = true;
        l.margin.start = remedy_ui::layout::AUTO;
    });
    for a in actions {
        acts = acts.child(a);
    }
    let mut row = ui.el("p_addon").row().align(Align::Center).gap(12.8).fill_width().layout(|l| l.wrap = true);
    if let Some(a) = art {
        row = row.child(a);
    }
    let row = row.child(grow).child(acts);
    row.state(States::SELECTED, current);
    row
}

fn strong(ui: &Ui, text: String) -> El {
    w::text(ui, text).variant("p_strong")
}

fn muted(ui: &Ui, text: String) -> El {
    w::text(ui, text).variant("muted")
}

/// main.ts `artBlock` at 64 px.
fn art_block(ui: &Ui, art: Option<&str>, name: &str) -> El {
    let frame = ui.el("p_addon_art").width(64.0).height(64.0).clip().column().layout(|l| l.shrink = 0.0);
    match art_url(art) {
        None => frame.child(letter_tile(ui, name)),
        Some(url) => {
            let (u, n) = (ui.clone(), name.to_owned());
            let opts = ImageOptions::new().fallback(move || letter_tile(&u, &n));
            frame.child(image_ext(ui, ImgSource::Url(url), "", Fit::Cover, opts).layout(|l| {
                l.width = Length::Percent(1.0);
                l.height = Length::Percent(1.0);
            }))
        }
    }
}

fn matches(query: &str, parts: &[&str]) -> bool {
    query.is_empty() || parts.join(" ").to_lowercase().contains(query)
}

/// main.ts `paintLoadedRows` and `addonRow`.
fn loaded_rows(v: &Rc<View>) -> Vec<El> {
    let ui = &v.ui;
    let query = v.source_filter.value().trim().to_lowercase();
    let (addons, source, updates) = st(|s| (s.addons.clone(), s.source.clone(), s.updates.clone()));
    if addons.is_empty() {
        return vec![muted(ui, "No video add-ons are installed. Install one from a repository below.".into())];
    }
    let items: Vec<&AddonCard> = addons.iter().filter(|a| matches(&query, &[&a.name, &a.id])).collect();
    if items.is_empty() {
        return vec![muted(ui, "No video add-ons match.".into())];
    }
    items
        .into_iter()
        .map(|a| {
            let current = source.as_ref().and_then(|s| s.path.as_deref()) == Some(a.path.as_str());
            let name = display_name(&a.name, "Add-on");
            let mut title = w::row(ui).align(Align::Center).layout(|l| l.wrap = true).child(strong(ui, name.clone()));
            if current {
                title = title.child(ui.el("p_in_use").text("In use").layout(|l| l.margin.start = 5.6));
            }
            let update = updates.as_ref().and_then(|u| u.iter().find(|x| x.id == a.id)).map(|u| format!(" · Update available: {}", u.available)).unwrap_or_default();
            let (vc, ac) = (v.ctx.clone(), a.clone());
            let use_b = crate::widgets::button(ui, if current { "Open" } else { "Use" }, move || choose_source(&vc, &ac)).variant(if current { "in_use" } else { "" });
            let mut acts = vec![use_b, crate::widgets::link(ui, &v.ctx.router, &Page::AddonSettings(a.id.clone()).path(), "Settings", "action", None)];
            if a.removable {
                let (vv, id) = (v.clone(), a.id.clone());
                acts.push(crate::widgets::button(ui, "Remove", move || remove_addon(&vv, &id)));
            }
            addon_row(ui, current, Some(art_block(ui, a.art.as_deref(), &name)), title, vec![muted(ui, format!("{}{update}", a.id))], acts)
        })
        .collect()
}

/// main.ts `paintUpdateRows` and `updateRow`.
fn update_rows(v: &Rc<View>) -> Vec<El> {
    let ui = &v.ui;
    let (items, checking, error, flash, known) = st(|s| (s.updates.clone().unwrap_or_default(), s.updates_checking, s.updates_error.clone(), std::mem::take(&mut s.update_flash), s.updates.is_some()));
    let note = if checking {
        "Checking the repositories…".to_owned()
    } else if !error.is_empty() {
        error.clone()
    } else if !flash.is_empty() {
        flash
    } else if !known {
        String::new()
    } else if items.is_empty() {
        "Every add-on is up to date.".into()
    } else if items.len() == 1 {
        "1 add-on has an update.".into()
    } else {
        format!("{} add-ons have an update.", items.len())
    };
    let note_el = w::text(ui, note)
        .variant(if !error.is_empty() && !checking { "error" } else { "muted" })
        .semantics(remedy_ui::a11y::Semantics::new(remedy_ui::a11y::Role::Status))
        .layout(|l| l.margin.bottom = 5.6);
    let mut out = vec![note_el];
    for item in items {
        let name = display_name(&item.name, "Add-on");
        let kind = if item.kind == "module" { "Module" } else { "Video add-on" };
        let vv = v.clone();
        let id = item.id.clone();
        let b = crate::widgets::button(ui, "Update", move || update_addon(&vv, &id))
            .semantics(remedy_ui::a11y::Semantics::new(remedy_ui::a11y::Role::Button).label(format!("Update {name} to {}", item.available)).action(remedy_ui::a11y::Action::Click));
        out.push(addon_row(ui, false, None, strong(ui, name.clone()), vec![muted(ui, format!("{kind} · {} → {}", item.installed, item.available))], vec![b]));
    }
    out
}

/// main.ts `paintRepoRows`.
fn repo_rows(v: &Rc<View>) -> Vec<El> {
    let ui = &v.ui;
    let (repos, open) = st(|s| (s.repos.clone(), s.open_repo.clone()));
    if repos.is_empty() {
        return vec![muted(ui, "No repositories are installed.".into())];
    }
    repos
        .into_iter()
        .map(|r| {
            let is_open = r.id == open;
            let id = r.id.clone();
            let toggle = if is_open {
                crate::widgets::button(ui, "Close", close_repo)
            } else {
                crate::widgets::button(ui, "Open", move || open_repo(&id))
            };
            let mut acts = vec![toggle];
            if r.removable {
                let (vv, id) = (v.clone(), r.id.clone());
                acts.push(crate::widgets::button(ui, "Remove", move || remove_repo(&vv, &id)));
            }
            addon_row(ui, is_open, None, strong(ui, display_name(&r.name, "Repository")), vec![muted(ui, r.id.clone())], acts)
        })
        .collect()
}

/// main.ts `paintRepoVideoRows` and `repoAddonRow`.
fn video_rows(v: &Rc<View>) -> Vec<El> {
    let ui = &v.ui;
    let (open, items, state, show_mods) = st(|s| (s.open_repo.clone(), s.open_repo_items.clone(), s.open_repo_state.clone(), s.open_repo_modules));
    if open.is_empty() {
        return vec![];
    }
    match state {
        RepoState::Loading => return vec![w::text(ui, "Loading…")],
        RepoState::Error(e) => return vec![w::text(ui, e).variant("error")],
        RepoState::Ready => {}
    }
    let query = v.repo_filter.value().trim().to_lowercase();
    let matching: Vec<&RepoAddon> = items.iter().filter(|i| matches(&query, &[&i.name, &i.id, &i.summary])).collect();
    let videos: Vec<&RepoAddon> = matching.iter().copied().filter(|i| i.kind != "module").collect();
    let modules: Vec<&RepoAddon> = matching.iter().copied().filter(|i| i.kind == "module").collect();
    let has_videos = items.iter().any(|i| i.kind != "module");
    // A repository of modules only shows them straight away.
    let show_modules = show_mods || !has_videos || (!query.is_empty() && videos.is_empty());
    if videos.is_empty() && modules.is_empty() {
        return vec![muted(ui, if items.is_empty() { "This repository has no video add-ons." } else { "No add-ons match." }.into())];
    }
    let mut out: Vec<El> = videos.iter().map(|i| repo_addon_row(v, &open, i)).collect();
    if videos.is_empty() && has_videos {
        out = vec![muted(ui, "No video add-ons match.".into())];
    }
    if !modules.is_empty() {
        let count = format!("{} {}", modules.len(), if modules.len() == 1 { "module" } else { "modules" });
        if show_modules {
            let mut head = w::row(ui).align(Align::Center).gap(9.6).layout(|l| {
                l.margin.top = 19.2;
                l.margin.bottom = 3.2;
            });
            head = head.child(w::heading(ui, 3, "Modules"));
            // Hide only what the viewer opened; a filter can hold modules open.
            if has_videos && show_mods {
                let b = crate::widgets::button(ui, "Hide", || toggle_modules(false));
                MODS_BUTTON.with(|m| *m.borrow_mut() = Some(b.clone()));
                head = head.child(b);
            }
            out.push(head);
            out.push(muted(ui, "Some video add-ons rely on a module from another repository. Install one when its add-on asks for it.".into()));
            out.extend(modules.iter().map(|i| repo_addon_row(v, &open, i)));
        } else {
            let b = crate::widgets::button(ui, format!("Show {count}"), || toggle_modules(true)).layout(|l| {
                l.align_self = Some(Align::Start);
                l.margin.top = 9.6;
            });
            MODS_BUTTON.with(|m| *m.borrow_mut() = Some(b.clone()));
            out.push(b);
        }
    }
    out
}

fn repo_addon_row(v: &Rc<View>, repo: &str, item: &RepoAddon) -> El {
    let ui = &v.ui;
    let module = item.kind == "module";
    let action = if !item.installed {
        let (vv, r, id) = (v.clone(), repo.to_owned(), item.id.clone());
        crate::widgets::button(ui, "Install", move || install_listed(&vv, &r, &id))
    } else if module {
        muted(ui, "Installed".into())
    } else {
        let (c, it) = (v.ctx.clone(), item.clone());
        crate::widgets::button(ui, "Use", move || use_installed(&c, &it))
    };
    let mut acts = vec![action];
    if item.removable {
        let (vv, id) = (v.clone(), item.id.clone());
        acts.push(crate::widgets::button(ui, "Remove", move || remove_addon(&vv, &id)));
    }
    let version = if item.version.is_empty() { String::new() } else { format!(" · {}", item.version) };
    let mut lines = vec![muted(ui, format!("{}{version}", item.id))];
    if !item.summary.is_empty() {
        lines.push(muted(ui, item.summary.clone()));
    }
    addon_row(ui, false, None, strong(ui, display_name(&item.name, if module { "Module" } else { "Add-on" })), lines, acts)
}

fn toggle_modules(show: bool) {
    st(|s| s.open_repo_modules = show);
    MODS_BUTTON.with(|m| m.borrow_mut().take());
    with_view(|v| {
        bump(v.videos);
        // The pressed button was repainted; keep the keyboard on its successor.
        if let Some(b) = MODS_BUTTON.with(|m| m.borrow().clone()) {
            b.focus();
        } else {
            v.repo_videos.focus_first();
        }
    });
}

/// main.ts `checkUpdates`: the server keeps the answer for an hour; `fresh` asks again now.
fn check_updates(fresh: bool) {
    if st(|s| std::mem::replace(&mut s.updates_checking, true)) {
        return;
    }
    st(|s| s.updates_error.clear());
    let mut ctx = None;
    with_view(|v| {
        bump(v.updates);
        ctx = Some(v.ctx.clone());
    });
    let Some(ctx) = ctx else {
        st(|s| s.updates_checking = false);
        return;
    };
    ctx.api.run(api::addon_updates(fresh), &Cancel::new(), move |out| {
        st(|s| {
            match out {
                Ok(u) => s.updates = Some(u.items),
                Err(e) => s.updates_error = shown_detail(&e),
            }
            s.updates_checking = false;
        });
        with_view(|v| {
            bump(v.updates);
            bump(v.loaded);
        });
    });
}

fn update_note(r: &InstallResult, version: &str) -> String {
    let name = display_name(&r.name, "That add-on");
    let to = if version.is_empty() { String::new() } else { format!(" to {version}") };
    if r.applies.as_deref() == Some("next-start") {
        return format!("{name} is updated{to}. The new version loads after the next start.");
    }
    format!("{name} is updated{to}.")
}

fn install_note(r: &InstallResult) -> String {
    if !r.wrote {
        return "That add-on is already installed.".into();
    }
    let name = display_name(&r.name, "That add-on");
    if r.applies.as_deref() == Some("next-start") {
        return format!("{name} is installed. It loads after the next start.");
    }
    match r.kind.as_str() {
        "repository" => format!("{name} is installed. Open it to choose a video add-on."),
        "video" => format!("{name} is installed. Choose Use when it appears in the list."),
        "module" => format!("{name} is installed. Video add-ons that use it can now find it."),
        _ => format!("{name} is installed."),
    }
}

fn remove_note(r: &RemoveResult) -> String {
    let name = display_name(&r.name, if r.kind == "repository" { "That repository" } else { "That add-on" });
    if r.applies.as_deref() == Some("next-start") {
        return format!("{name} is removed. It disappears after the next start.");
    }
    if r.kind == "repository" {
        return format!("{name} is removed. Add-ons already installed from it stay.");
    }
    format!("{name} is removed.")
}

/// main.ts `updateAddon`.
fn update_addon(v: &Rc<View>, id: &str) {
    if st(|s| s.busy) || !addon_id_ok(id) {
        return;
    }
    let item = st(|s| s.updates.as_ref().and_then(|u| u.iter().find(|x| x.id == id).cloned()));
    let name = display_name(item.as_ref().map_or("", |i| i.name.as_str()), "");
    let available = item.map(|i| i.available).unwrap_or_default();
    st(|s| {
        s.busy = true;
        s.updates_error.clear();
    });
    let ticket = v.ctx.activity.begin(Owner::Page, describe_addon_update(&name), 0);
    let (c, id) = (v.ctx.clone(), id.to_owned());
    v.ctx.api.run(api::update_addon(&id), &Cancel::new(), move |out| {
        st(|s| s.busy = false);
        match out {
            Ok(r) => {
                st(|s| {
                    s.update_flash = update_note(&r, &available);
                    if let Some(u) = s.updates.as_mut() {
                        u.retain(|x| x.id != id);
                    }
                });
                rerender();
            }
            Err(e) => {
                c.activity.end(ticket);
                st(|s| s.updates_error = shown_detail(&e));
                with_view(|v| bump(v.updates));
            }
        }
    });
}

/// main.ts `chooseSource`: the add-on in use, then its top folder.
fn choose_source(ctx: &Ctx, a: &AddonCard) {
    let body = Source { addon_id: Some(a.id.clone()), name: Some(a.name.clone()), path: Some(a.path.clone()) };
    let (c, a) = (ctx.clone(), a.clone());
    ctx.api.run(api::set_source(&body), &Cancel::new(), move |out| match out {
        Ok(_) => {
            // Pages that name or link the add-on look it up again.
            crate::screens::browse::forget_source();
            crate::widgets::shows::note_path_label(&a.path, &a.name);
            c.go(&Page::Browse(a.path.clone()));
        }
        Err(e) => c.activity.notice(shown_detail(&e)),
    });
}

/// main.ts `useInstalled`.
fn use_installed(ctx: &Ctx, item: &RepoAddon) {
    if !addon_id_ok(&item.id) {
        with_view(|v| v.note.set("Unknown add-on.".into()));
        return;
    }
    choose_source(ctx, &AddonCard { id: item.id.clone(), name: item.name.clone(), path: format!("plugin://{}/", item.id), art: None, removable: false });
}

/// main.ts `submitZip`.
fn submit_zip(v: &Rc<View>, url: &str, name: &str) {
    if st(|s| std::mem::replace(&mut s.busy, true)) {
        return;
    }
    v.note.set(String::new());
    v.installing.set(true);
    let ticket = v.ctx.activity.begin(Owner::Page, describe_repository_install(), 0);
    let (vv, c) = (v.clone(), v.ctx.clone());
    let name = Some(name).filter(|n| !n.trim().is_empty());
    v.ctx.api.run(api::install_zip(url, name), &Cancel::new(), move |out| {
        st(|s| s.busy = false);
        match out {
            Ok(r) => {
                st(|s| {
                    s.flash = install_note(&r);
                    if r.kind == "repository" && r.wrote {
                        s.open_repo.clear();
                    }
                });
                rerender();
            }
            Err(e) => {
                let m = shown_detail(&e);
                let _ = vv.note.try_set(m.clone());
                c.activity.notice(m);
                let _ = vv.installing.try_set(false);
                c.activity.end(ticket);
                let repo = st(|s| s.open_repo.clone());
                if !repo.is_empty() {
                    open_repo(&repo);
                }
            }
        }
    });
}

fn close_repo() {
    st(|s| {
        s.open_repo.clear();
        s.open_repo_name.clear();
        s.open_repo_items.clear();
        s.open_repo_modules = false;
    });
    with_view(|v| {
        v.repo_filter.set_value("");
        v.note.set(String::new());
        bump(v.videos);
        bump(v.repos);
    });
}

/// main.ts `openRepo`.
fn open_repo(id: &str) {
    if st(|s| s.busy) {
        return;
    }
    if !addon_id_ok(id) {
        with_view(|v| v.note.set("Unknown add-on.".into()));
        return;
    }
    let Some(v) = VIEW.with(|v| v.borrow().clone()) else { return };
    let name = st(|s| {
        s.busy = true;
        if s.open_repo != id {
            s.open_repo_modules = false;
        }
        s.open_repo = id.to_owned();
        s.open_repo_name = s.repos.iter().find(|r| r.id == id).map(|r| display_name(&r.name, "Repository")).unwrap_or_default();
        s.open_repo_items.clear();
        s.open_repo_state = RepoState::Loading;
        s.open_repo_name.clone()
    });
    let ticket = v.ctx.activity.begin(Owner::Page, describe_repository_open(&name), 0);
    bump(v.videos);
    bump(v.repos);
    let (c, id) = (v.ctx.clone(), id.to_owned());
    v.ctx.api.run(api::repo(&id), &Cancel::new(), move |out| {
        st(|s| s.busy = false);
        c.activity.end(ticket);
        if Page::from_route(&c.router.route_untracked()) != Page::Source || st(|s| s.open_repo != id) {
            return;
        }
        st(|s| match out {
            Ok(r) => {
                let was = if s.open_repo_name.is_empty() { "Repository".to_owned() } else { s.open_repo_name.clone() };
                s.open_repo_name = display_name(&r.name, &was);
                s.open_repo_items = r.items;
                s.open_repo_state = RepoState::Ready;
            }
            Err(e) => {
                s.open_repo_items.clear();
                s.open_repo_state = RepoState::Error(shown_detail(&e));
            }
        });
        with_view(|v| bump(v.videos));
    });
}

/// main.ts `installListed`.
fn install_listed(v: &Rc<View>, repo: &str, addon: &str) {
    if st(|s| s.busy) {
        return;
    }
    if !addon_id_ok(repo) || !addon_id_ok(addon) {
        v.note.set("Unknown add-on.".into());
        return;
    }
    let name = st(|s| {
        s.busy = true;
        s.open_repo_items.iter().find(|i| i.id == addon).map(|i| display_name(&i.name, "")).unwrap_or_default()
    });
    v.note.set(String::new());
    let ticket = v.ctx.activity.begin(Owner::Page, describe_addon_install(&name), 0);
    let (vv, c, repo) = (v.clone(), v.ctx.clone(), repo.to_owned());
    v.ctx.api.run(api::install_addon(&repo, addon), &Cancel::new(), move |out| {
        st(|s| s.busy = false);
        match out {
            Ok(r) => {
                st(|s| {
                    s.flash = install_note(&r);
                    s.open_repo = repo.clone();
                });
                rerender();
            }
            Err(e) => {
                let _ = vv.note.try_set(shown_detail(&e));
                c.activity.end(ticket);
                if st(|s| s.open_repo == repo) {
                    open_repo(&repo);
                }
            }
        }
    });
}

/// main.ts `removeRepo`: asks first, in Portal's theme.
fn remove_repo(v: &Rc<View>, id: &str) {
    if st(|s| s.busy) {
        return;
    }
    if !addon_id_ok(id) {
        v.note.set("Unknown add-on.".into());
        return;
    }
    let Some(known) = st(|s| s.repos.iter().find(|r| r.id == id).cloned()) else { return };
    if !known.removable {
        return;
    }
    let name = display_name(&known.name, "Repository");
    let (vv, id) = (v.clone(), id.to_owned());
    crate::screens::prompts::confirm(&v.ui, &format!("Remove {name}? Add-ons already installed from it stay."), move || {
        if st(|s| std::mem::replace(&mut s.busy, true)) {
            return;
        }
        vv.note.set(String::new());
        let ticket = vv.ctx.activity.begin(Owner::Page, describe_remove(Removed::Repository, &name), 0);
        let (v2, id2) = (vv.clone(), id.clone());
        vv.ctx.api.run(api::remove_repo(&id), &Cancel::new(), move |out| {
            st(|s| s.busy = false);
            match out {
                Ok(r) => {
                    st(|s| {
                        if s.open_repo == id2 {
                            s.open_repo.clear();
                            s.open_repo_name.clear();
                            s.open_repo_items.clear();
                        }
                        s.flash = remove_note(&r);
                    });
                    rerender();
                }
                Err(e) => {
                    let _ = v2.note.try_set(shown_detail(&e));
                    v2.ctx.activity.end(ticket);
                }
            }
        });
    });
}

/// main.ts `removeAddon`: asks first, in Portal's theme.
fn remove_addon(v: &Rc<View>, id: &str) {
    if st(|s| s.busy) {
        return;
    }
    if !addon_id_ok(id) {
        v.note.set("Unknown add-on.".into());
        return;
    }
    let (loaded, listed) = st(|s| (s.addons.iter().find(|a| a.id == id).cloned(), s.open_repo_items.iter().find(|i| i.id == id).cloned()));
    if loaded.as_ref().is_some_and(|a| !a.removable) || (loaded.is_none() && listed.as_ref().is_some_and(|i| !i.removable)) {
        return;
    }
    let raw = loaded.as_ref().map(|a| a.name.clone()).filter(|n| !n.is_empty()).or(listed.map(|i| i.name)).unwrap_or_default();
    let name = display_name(&raw, "Add-on");
    let (vv, id) = (v.clone(), id.to_owned());
    crate::screens::prompts::confirm(&v.ui, &format!("Remove {name}? Its settings on this PC are deleted."), move || {
        if st(|s| std::mem::replace(&mut s.busy, true)) {
            return;
        }
        vv.note.set(String::new());
        let ticket = vv.ctx.activity.begin(Owner::Page, describe_remove(Removed::Video, &name), 0);
        let (v2, id2) = (vv.clone(), id.clone());
        vv.ctx.api.run(api::remove_addon(&id), &Cancel::new(), move |out| {
            st(|s| s.busy = false);
            match out {
                Ok(r) => {
                    let was_source = st(|s| {
                        s.flash = remove_note(&r);
                        s.source.as_ref().is_some_and(|x| x.addon_id.as_deref() == Some(id2.as_str()) || x.path.as_deref() == Some(format!("plugin://{id2}/").as_str()))
                    });
                    if was_source {
                        crate::screens::browse::forget_source();
                    }
                    rerender();
                }
                Err(e) => {
                    let _ = v2.note.try_set(shown_detail(&e));
                    v2.ctx.activity.end(ticket);
                    let repo = st(|s| s.open_repo.clone());
                    if !repo.is_empty() {
                        open_repo(&repo);
                    }
                }
            }
        });
    });
}

pub fn rules(s: &mut StyleSheet, k: &Look) {
    let pad = |y: f32, x: f32| remedy_ui::core::Insets { top: y, bottom: y, start: x, end: x };
    // .source-card .addon: a row on the card's well, a hairline, 14 px corners; gold when current.
    s.rule("p_addon", None, States::NONE, StylePatch::default().background(k.well).border(k.line).border_width(1.0).radius(14.0).padding(pad(13.6, 14.4)));
    s.rule("p_addon", None, States::HOVER, StylePatch::default().border(mix(k.gold, 0.48, k.line)));
    s.rule("p_addon", None, States::SELECTED, StylePatch::default().border(k.gold));
    s.rule("p_src_rows", None, States::NONE, StylePatch::default());
    s.rule("p_src_rule", None, States::NONE, StylePatch::default().background(k.line));
    s.rule("p_addon_art", None, States::NONE, StylePatch::default().radius(8.0).background(k.well));
    s.rule("label", Some("p_strong"), States::NONE, StylePatch::default().foreground(k.text));
    // .in-use: a gold pill.
    s.rule(
        "p_in_use",
        None,
        States::NONE,
        StylePatch::default().foreground(k.gold).border(k.gold).border_width(1.0).radius(99.0).padding(pad(0.8, 7.2)).text_size(12.8),
    );
    // .addon .row-actions button.primary: a gold ring and gold words.
    s.rule("p_button", Some("in_use"), States::NONE, StylePatch::default().border(k.gold).foreground(k.gold));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(kind: &str, wrote: bool, applies: Option<&str>) -> InstallResult {
        InstallResult { id: "x".into(), name: "[B]X[/B]".into(), kind: kind.into(), wrote, applies: applies.map(str::to_owned) }
    }

    #[test]
    fn notes_as_main_ts() {
        assert_eq!(install_note(&result("video", false, None)), "That add-on is already installed.");
        assert_eq!(install_note(&result("repository", true, None)), "X is installed. Open it to choose a video add-on.");
        assert_eq!(install_note(&result("module", true, Some("next-start"))), "X is installed. It loads after the next start.");
        assert_eq!(update_note(&result("video", true, None), "1.2"), "X is updated to 1.2.");
        let r = RemoveResult { id: "r".into(), name: "".into(), kind: "repository".into(), applies: None };
        assert_eq!(remove_note(&r), "That repository is removed. Add-ons already installed from it stay.");
    }

    #[test]
    fn ids_checked_as_main_ts() {
        assert!(addon_id_ok("plugin.video.x-1_2"));
        assert!(!addon_id_ok("a..b"));
        assert!(!addon_id_ok("a/b"));
        assert!(!addon_id_ok(""));
        assert!(!addon_id_ok(&"a".repeat(81)));
    }
}
