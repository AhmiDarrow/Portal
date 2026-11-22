//! Subtitles in the dock (player.ts 1295-1863, subtitles.ts; plan P9): the job's own tracks,
//! the online search (`POST /api/subtitles/search`), Auto's pick in the person's languages,
//! and on or off with the language last picked shared through `/api/prefs`.
//!
//! The player's captions control is the CC button (improvement I1): one button that says
//! what shows ("Off", "Auto · English", "Loading…") and opens the menu of Off, Auto, the
//! tracks loaded, every other track and online result as a choice with its details, the
//! search's note, and the timing half a second at a time. A track is fetched when it is to
//! show and handed to the player, which draws the cues over the picture in every layout.
//! Online files come uncut (offset 0) and sit on the work's time, so they stay right after a
//! skip; the job's own tracks are cut to their stream and go on the work's time from it.
//!
//! ENGINE GAP (RemedyUI `remedy-ui-media-playback/src/player.rs`): a track the app added can
//! be neither replaced nor removed (only `add_subtitles` / `add_subtitles_stream`, 1946-1974),
//! and `replace_source` keeps the app's tracks (`load(.., same_work: true ..)`, 1576-1584,
//! 1106-1118). So (1) a video track cut to a stream that started later (a resume) has no
//! cues before that point after a skip back, where player.ts attached the new stream's
//! file; and (2) playing another title while one plays (not as its next, which uses
//! `start_next` and drops them) leaves the last title's loaded tracks in the menu. Auto and
//! the dock only ever show tracks loaded for the current item.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::Duration;

use portal_api::{
    CastSubtitle, MediaInfo, MediaKind, PlayJob, SubtitleCandidate, SubtitleMatch, SubtitleMode, SubtitleResults,
    SubtitleSearch, SubtitleSettings, SubtitleTrack as JobTrack,
};
use remedy_ui::playback::{parse_subtitles, CaptionChoice, CaptionPick, SubtitleFormat, SubtitleTrack};
use remedy_ui::reactive::{untrack, Cancel};
use remedy_ui::Ui;

use super::{view, Dock};
use crate::screens::settings::{browser_languages, primary_language};
use crate::{api, prefs};

/// How long the saved subtitle settings are trusted before they are read again.
pub const SETTINGS_FOR: Duration = Duration::from_secs(60);
/// How often the job is asked for its own tracks while it still reads them.
pub const TRACKS_POLL: Duration = Duration::from_secs(3);

thread_local! {
    static SETTINGS: RefCell<Option<(f64, SubtitleSettings)>> = const { RefCell::new(None) };
    static TRACKS_TRIES: Cell<u32> = const { Cell::new(100) };
}

/// How many times the job is asked for its own tracks before giving up (default 100).
pub fn set_tracks_tries(n: u32) {
    TRACKS_TRIES.with(|c| c.set(n));
}

/// subtitles.ts `rememberSubtitleSettings`: after the Settings page saves, the next search
/// uses the new languages.
pub fn remember_settings(s: Option<SubtitleSettings>) {
    SETTINGS.with(|c| *c.borrow_mut() = s.map(|s| (now_ms(), s)));
}

fn now_ms() -> f64 {
    thread_local!(static T0: remedy_ui::core::time::Instant = remedy_ui::core::time::Instant::now());
    T0.with(|t| t.elapsed().as_secs_f64() * 1000.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Search {
    #[default]
    Idle,
    Loading,
    Done,
    Error,
}

#[derive(Default)]
struct Online {
    /// The job the results belong to.
    key: String,
    state: Search,
    items: Vec<SubtitleCandidate>,
    best: Option<String>,
    notes: Vec<String>,
    error: String,
}

/// One played item's subtitles.
#[derive(Default)]
struct State {
    /// The dock generation this belongs to.
    item: u64,
    token: u64,
    tracks: Vec<JobTrack>,
    online: Online,
    settings: Option<SubtitleSettings>,
    /// This item's explicit choice: "e:<id>" or "o:<id>".
    pick: Option<String>,
    /// Choices handed to the player: key and track index.
    loaded: Vec<(String, usize)>,
    /// The choice being fetched.
    loading: String,
    failed: HashSet<String>,
    /// The status line this module last wrote, and whether it stays until the next action.
    note: String,
    sticky: bool,
}

#[derive(Default)]
pub(crate) struct Subs {
    st: RefCell<State>,
    /// Set while the dock moves the player's pick itself, so it is not taken as the person's.
    quiet: Cell<bool>,
}

fn on() -> bool {
    prefs::now().subtitles != SubtitleMode::Off
}

impl Dock {
    fn subs(&self) -> std::cell::RefMut<'_, State> {
        self.0.subs.st.borrow_mut()
    }

    /// Moves the player's pick without hearing it as the person's.
    pub(super) fn quietly(&self, f: impl FnOnce()) {
        let was = self.0.subs.quiet.replace(true);
        f();
        self.0.subs.quiet.set(was);
    }

    /// Listens to the captions control; called once when the dock is made.
    pub(super) fn wire_subs(&self) {
        let w = self.weak();
        self.0.player.on_caption_pick(move |pick| {
            if let Some(i) = w.upgrade() {
                let d = Dock(i);
                if !d.0.subs.quiet.get() {
                    d.picked(pick.clone());
                }
            }
        });
        // A choice made on another screen or device (`/api/prefs`) shows here too.
        let (w, ps) = (self.weak(), prefs::signal());
        let _ = self.0.ui.root().bind(move || {
            let mode = ps.with(|p| p.subtitles);
            let Some(i) = w.upgrade() else { return };
            let d = Dock(i);
            untrack(|| {
                if !d.0.st.borrow().attached {
                    return;
                }
                let shown = d.0.player.caption_pick() != CaptionPick::Off;
                if shown != (mode != SubtitleMode::Off) {
                    d.match_prefs();
                    d.apply_subs();
                }
            });
        });
    }

    /// The player's pick as `/api/prefs` says: Auto or Off.
    fn match_prefs(&self) {
        let p = &self.0.player;
        self.quietly(|| p.set_caption_pick(if on() { CaptionPick::Auto } else { CaptionPick::Off }));
    }

    /// player.ts `resetSubtitles`: the last item's subtitles are forgotten and a late answer
    /// for them is dropped.
    pub(super) fn reset_subs(&self) {
        {
            let mut s = self.subs();
            let token = s.token + 1;
            *s = State { token, ..State::default() };
        }
        self.clear_sub_note(false);
        let p = &self.0.player;
        p.set_caption_choices(Vec::new());
        p.set_caption_note(None);
        p.set_captions_loading(false);
    }

    /// At each attach: a new item starts its subtitles (its tracks, the wait for more, the
    /// search); a new stream of the same item keeps them, and only learns the job's tracks.
    pub(super) fn attach_subs(&self, job: &PlayJob, new_work: bool) {
        let generation = self.generation();
        let fresh = self.subs().item != generation;
        if fresh {
            self.reset_subs();
            self.subs().item = generation;
            // Timing fixes one file; the next episode keeps it, another title starts in step.
            if !new_work {
                self.0.player.set_caption_delay(0);
            }
            self.match_prefs();
        }
        self.subs().tracks = job.subtitles.clone();
        self.paint_choices();
        self.apply_subs();
        self.await_tracks(job);
        self.search_subs(job);
    }

    /// The person's pick in the captions menu, or C.
    fn picked(&self, pick: CaptionPick) {
        let set = |m: SubtitleMode, lang: Option<Option<String>>| {
            prefs::set(&self.0.ctx, prefs::Patch { subtitles: Some(m), subtitle_language: lang, ..Default::default() });
        };
        match pick {
            CaptionPick::Off => {
                self.subs().pick = None;
                set(SubtitleMode::Off, None);
                self.clear_sub_note(false);
            }
            CaptionPick::Auto => {
                self.subs().pick = None;
                set(SubtitleMode::Auto, Some(None));
                self.clear_sub_note(false);
            }
            CaptionPick::Track(i) => {
                let key = self.subs().loaded.iter().find(|(_, at)| *at == i).map(|(k, _)| k.clone());
                let lang = key.as_deref().and_then(|k| self.choice_language(k));
                self.subs().pick = key;
                set(SubtitleMode::Auto, lang.map(Some));
                self.clear_sub_note(false);
            }
            CaptionPick::Choice(key) => {
                // A video track's language is kept at once, an online one's once it loads.
                let lang = key.starts_with("e:").then(|| self.choice_language(&key)).flatten();
                {
                    let mut s = self.subs();
                    s.failed.remove(&key);
                    s.pick = Some(key);
                }
                set(SubtitleMode::Auto, lang.map(Some));
                self.clear_sub_note(false);
            }
            _ => {}
        }
        self.apply_subs();
        if self.0.cast.current().is_some() {
            self.sync_device_subs();
        } else if on() && self.chosen_sub().is_none() {
            let text = self.no_subs_text(true);
            self.sub_note(&text, false);
        }
    }

    /// player.ts `syncDeviceSubtitles`: while a TV plays, it shows what this page would.
    fn sync_device_subs(&self) {
        let Some(dev) = self.0.cast.current() else { return };
        let choice = if on() { self.cast_subtitle() } else { None };
        if on() && choice.is_none() {
            let text = self.no_subs_text(true);
            return self.sub_note(&text, false);
        }
        let shown = choice.is_some();
        self.run(api::cast_device_subtitles(&dev.id, choice.as_ref()), move |me, out| {
            match out {
                Ok(_) if shown => me.sub_note("Subtitles are on for the TV.", false),
                Ok(_) => me.clear_sub_note(false),
                Err(e) => me.sub_note(&super::cast::cast_fail(&e), true),
            }
        });
    }

    fn choice_language(&self, key: &str) -> Option<String> {
        let s = self.subs();
        let id = key.get(2..)?;
        let lang = match &key[..2] {
            "e:" => s.tracks.iter().find(|t| t.id.to_string() == id)?.lang.clone(),
            _ => s.online.items.iter().find(|c| c.id == id).map(|c| c.language.clone()),
        };
        lang.map(|l| l.trim().to_owned()).filter(|l| !l.is_empty())
    }

    /// player.ts `chosenSubtitle`: what should show now, as "e:<id>" or "o:<id>".
    fn chosen_sub(&self) -> Option<String> {
        if !on() {
            return None;
        }
        let s = self.subs();
        let known = |k: &str| match k.split_at(2) {
            ("e:", id) => s.tracks.iter().any(|t| t.id.to_string() == id),
            (_, id) => s.online.items.iter().any(|c| c.id == id),
        };
        if let Some(p) = s.pick.as_deref().filter(|p| known(p)) {
            return Some(p.to_owned());
        }
        let tracks: Vec<JobTrack> = s.tracks.iter().filter(|t| !s.failed.contains(&format!("e:{}", t.id))).cloned().collect();
        let usable: Vec<&SubtitleCandidate> =
            s.online.items.iter().filter(|c| !s.failed.contains(&format!("o:{}", c.id))).collect();
        // The language of the last pick comes first, in the video, then online.
        if let Some(lang) = prefs::now().subtitle_language {
            if let Some(t) = preferred_track(&tracks, std::slice::from_ref(&lang)) {
                return Some(format!("e:{}", t.id));
            }
            if let Some(c) = usable.iter().find(|c| same_language(&c.language, &lang)) {
                return Some(format!("o:{}", c.id));
            }
        }
        let langs = preferred_languages(s.settings.as_ref(), &self.0.ctx.app.preferred_locales());
        auto_subtitle(&tracks, &usable, &langs)
    }

    /// player.ts `applySubtitles`: shows what `chosen_sub` says, fetching it first.
    fn apply_subs(&self) {
        let attached = self.0.st.borrow().attached;
        let want = if attached { self.chosen_sub() } else { None };
        let p = self.0.player.clone();
        let Some(key) = want else {
            self.subs().loading.clear();
            p.set_captions_loading(false);
            let mine = untrack(|| p.caption_track()).is_some_and(|i| self.subs().loaded.iter().any(|(_, at)| *at == i));
            if mine {
                p.set_caption_track(None);
            }
            return;
        };
        let (loaded, loading, explicit) = {
            let s = self.subs();
            (s.loaded.iter().find(|(k, _)| *k == key).map(|(_, i)| *i), s.loading == key, s.pick.as_deref() == Some(&key))
        };
        if let Some(i) = loaded {
            if explicit {
                // The menu marks the track itself, not the choice it came from.
                if untrack(|| p.caption_pick()) != CaptionPick::Track(i) {
                    self.quietly(|| p.set_caption_pick(CaptionPick::Track(i)));
                }
            } else if untrack(|| p.caption_track()) != Some(i) {
                p.set_caption_track(Some(i));
            }
            return;
        }
        if loading {
            return;
        }
        self.fetch_sub(key);
    }

    fn fetch_sub(&self, key: String) {
        let (token, url, label, lang, online) = {
            let mut s = self.subs();
            s.loading = key.clone();
            let id = &key[2..];
            if key.starts_with("e:") {
                let Some(t) = s.tracks.iter().find(|t| t.id.to_string() == id).cloned() else { return };
                (s.token, t.url.clone(), track_name(&self.0.ui, &t), t.lang.clone(), false)
            } else {
                let c = s.online.items.iter().find(|c| c.id == id).cloned();
                // The CC button shows this name: the language, unless one loaded already has it.
                let taken = |l: &str| untrack(|| self.0.player.subtitle_tracks()).iter().any(|t| t.label == l);
                let label = match &c {
                    Some(c) if !taken(&c.language_name) => c.language_name.clone(),
                    Some(c) => candidate_title(c),
                    None => "Subtitles".to_owned(),
                };
                // Uncut: the whole file on the work's time stays right after any skip.
                (s.token, api::subtitle_file_url(id, 0.0), label, c.map(|c| c.language), true)
            }
        };
        self.0.player.set_captions_loading(true);
        let session = self.0.st.borrow().last_ready.as_ref().and_then(|j| j.session.clone());
        let w = self.weak();
        self.0.ctx.api.run_text(api::text_file(&url), &Cancel::new(), move |out| {
            let Some(i) = w.upgrade() else { return };
            let me = Dock(i);
            {
                let s = me.subs();
                if s.token != token || s.loading != key {
                    return;
                }
            }
            me.subs().loading.clear();
            let cues = out.as_ref().ok().map(|text| {
                let format = SubtitleFormat::sniff(text).unwrap_or(SubtitleFormat::WebVtt);
                parse_subtitles(text, format)
            });
            match (out, cues) {
                (Ok(_), Some(cues)) if !cues.is_empty() => {
                    // A video track is cut to its stream: one that changed meanwhile is fetched again.
                    let now = me.0.st.borrow().last_ready.as_ref().and_then(|j| j.session.clone());
                    if !online && now != session {
                        me.apply_subs();
                        return;
                    }
                    let track = SubtitleTrack { label, language: lang.clone(), cues };
                    let p = &me.0.player;
                    let at = me.quiet_add(|| if online { p.add_subtitles(track) } else { p.add_subtitles_stream(track) });
                    me.subs().loaded.push((key.clone(), at));
                    // An online pick is remembered by its language once it loads, for the next video.
                    if online && me.subs().pick.as_deref() == Some(&key) {
                        if let Some(l) = lang {
                            prefs::set(&me.0.ctx, prefs::Patch { subtitle_language: Some(Some(l)), ..Default::default() });
                        }
                    }
                    me.paint_choices();
                    me.apply_subs();
                    me.0.player.set_captions_loading(false);
                    me.clear_sub_note(true);
                }
                (out, _) => {
                    {
                        let mut s = me.subs();
                        s.failed.insert(key.clone());
                        if s.pick.as_deref() == Some(&key) {
                            s.pick = None;
                        }
                    }
                    let text = match out {
                        Err(e) if online && !e.detail.trim().is_empty() => e.detail,
                        _ if online => "Those subtitles could not be loaded.".to_owned(),
                        _ => "These subtitles could not be loaded.".to_owned(),
                    };
                    me.0.player.set_captions_loading(false);
                    me.sub_note(&text, true);
                    // Auto tries the next one; an explicit pick falls back to Auto.
                    if untrack(|| me.0.player.caption_pick()) != CaptionPick::Off {
                        me.quietly(|| me.0.player.set_caption_pick(CaptionPick::Auto));
                    }
                    me.paint_choices();
                    me.apply_subs();
                }
            }
        });
    }

    /// Adds a track without its name being taken as a pick carried from the last media.
    fn quiet_add(&self, f: impl FnOnce() -> usize) -> usize {
        let was = self.0.subs.quiet.replace(true);
        let at = f();
        self.0.subs.quiet.set(was);
        at
    }

    /// The captions menu's choices (every track and result not loaded yet) and its note.
    fn paint_choices(&self) {
        let ui = &self.0.ui;
        let s = self.subs();
        let loaded = |k: &str| s.loaded.iter().any(|(l, _)| l == k);
        let mut out = Vec::new();
        for t in &s.tracks {
            let key = format!("e:{}", t.id);
            if loaded(&key) {
                continue;
            }
            let name = track_name(ui, t);
            let language = language_name(ui, t.lang.as_deref().unwrap_or(""));
            let mut c = CaptionChoice::new(key.clone(), name.clone());
            let mut detail = vec![];
            if !language.is_empty() && language != name {
                detail.push(language);
            }
            if s.failed.contains(&key) {
                detail.push("Failed to load".to_owned());
            }
            if !detail.is_empty() {
                c = c.detail(detail.join(" · "));
            }
            out.push(c);
        }
        for item in &s.online.items {
            let key = format!("o:{}", item.id);
            if loaded(&key) {
                continue;
            }
            let mut detail = candidate_detail(item, s.online.best.as_deref() == Some(&item.id));
            if s.failed.contains(&key) {
                detail.push_str(" · Failed to load");
            }
            out.push(CaptionChoice::new(key, candidate_title(item)).detail(detail));
        }
        let mut notes = Vec::new();
        match s.online.state {
            Search::Loading => notes.push("Searching online…".to_owned()),
            Search::Error => notes.push(format!("Search failed: {}", s.online.error)),
            Search::Done if s.online.items.is_empty() => notes.push("No online subtitles found.".to_owned()),
            Search::Idle => notes.push("Not searched yet.".to_owned()),
            Search::Done => {}
        }
        notes.extend(s.online.notes.iter().cloned());
        drop(s);
        let p = &self.0.player;
        p.set_caption_choices(out);
        p.set_caption_note((!notes.is_empty()).then(|| notes.join(" ")));
    }

    /// player.ts `searchSubtitles`: online, once per played item.
    fn search_subs(&self, job: &PlayJob) {
        let Some(target) = self.0.target.get_untracked().ok().flatten() else { return };
        let (token, key) = {
            let mut s = self.subs();
            if s.online.key == job.id {
                return;
            }
            s.online = Online { key: job.id.clone(), state: Search::Loading, ..Online::default() };
            (s.token, job.id.clone())
        };
        self.paint_choices();
        let release = job.release.clone();
        let fresh = move |me: &Dock| {
            let s = me.subs();
            s.token == token && s.online.key == key
        };
        self.with_settings(move |me, settings| {
            if !fresh(me) {
                return;
            }
            me.subs().settings = settings.clone();
            let seed = view::info_seed(&target);
            let locales = me.0.ctx.app.preferred_locales();
            let Some(body) = search_for(&seed, release.as_deref(), settings.as_ref(), &locales) else {
                me.searched(Online { state: Search::Done, ..Online::default() });
                return;
            };
            me.run(api::search_subtitles(&body), move |me, out| {
                if !fresh(me) {
                    return;
                }
                me.searched(match out {
                    Ok(r) => {
                        let (items, best, notes) = clean_results(r);
                        Online { state: Search::Done, items, best, notes, ..Online::default() }
                    }
                    Err(e) => Online { state: Search::Error, error: api::shown_detail(&e), ..Online::default() },
                });
            });
        });
    }

    fn searched(&self, mut next: Online) {
        let waiting = {
            let mut s = self.subs();
            next.key = std::mem::take(&mut s.online.key);
            s.online = next;
            on() && s.loading.is_empty() && !s.note.is_empty()
        };
        self.paint_choices();
        self.apply_subs();
        if waiting && self.chosen_sub().is_none() {
            let text = self.no_subs_text(false);
            self.sub_note(&text, false);
        }
    }

    /// subtitles.ts `subtitleSettings`: kept for a minute; none when they cannot be read.
    fn with_settings(&self, done: impl FnOnce(&Dock, Option<SubtitleSettings>) + 'static) {
        let kept = SETTINGS.with(|c| c.borrow().clone()).filter(|(at, _)| now_ms() - at < SETTINGS_FOR.as_secs_f64() * 1000.0);
        if let Some((_, s)) = kept {
            return done(self, Some(s));
        }
        self.run(api::subtitle_settings(), move |me, out| {
            let s = out.ok();
            remember_settings(s.clone());
            done(me, s);
        });
    }

    /// player.ts `awaitSubtitles`: playback starts before Portal has read the file's own
    /// tracks; they join the menu when ready. A new stream of the item checks for itself.
    fn await_tracks(&self, job: &PlayJob) {
        if !job.subtitles_pending {
            return;
        }
        let (token, id, session) = (self.generation(), job.id.clone(), job.session.clone());
        let tries = TRACKS_TRIES.with(Cell::get);
        fn ask(me: &Dock, token: u64, id: String, session: Option<String>, left: u32) {
            if left == 0 {
                return;
            }
            me.after(TRACKS_POLL, move |me| {
                let current = move |me: &Dock| me.generation() == token && me.0.st.borrow().attached;
                if !current(me) {
                    return;
                }
                let id2 = id.clone();
                me.run(api::job(&id), move |me, out| {
                    if !current(me) {
                        return;
                    }
                    let Ok(now) = out else { return ask(me, token, id2, session, left - 1) };
                    if now.session != session {
                        return;
                    }
                    if now.subtitles_pending {
                        return ask(me, token, id2, session, left - 1);
                    }
                    if !now.subtitles.is_empty() {
                        me.0.st.borrow_mut().last_ready = Some(now.clone());
                        me.subs().tracks = now.subtitles;
                        me.paint_choices();
                        me.apply_subs();
                    }
                });
            });
        }
        ask(self, token, id, session, tries);
    }

    fn no_subs_text(&self, asked: bool) -> String {
        let s = self.subs();
        match s.online.state {
            Search::Loading | Search::Idle if asked => "Looking for subtitles…".to_owned(),
            Search::Error => format!("Portal could not search for subtitles: {}", s.online.error),
            _ => s.online.notes.first().cloned().unwrap_or_else(|| "No subtitles found for this video.".to_owned()),
        }
    }

    fn sub_note(&self, text: &str, sticky: bool) {
        {
            let mut s = self.subs();
            s.note = text.to_owned();
            s.sticky = sticky;
        }
        self.set_status(text);
    }

    /// Clears the status line if it is still this module's (an error stays when `keep_errors`).
    fn clear_sub_note(&self, keep_errors: bool) {
        let note = {
            let mut s = self.subs();
            if s.note.is_empty() || (keep_errors && s.sticky) {
                return;
            }
            std::mem::take(&mut s.note)
        };
        if self.0.status.get_untracked().ok().as_deref() == Some(note.as_str()) {
            self.set_status("");
        }
    }

    /// player.ts `castSubtitle`: the subtitle this page would show, as a TV needs it.
    pub(crate) fn cast_subtitle(&self) -> Option<CastSubtitle> {
        let key = self.chosen_sub()?;
        let s = self.subs();
        let id = &key[2..];
        if key.starts_with("e:") {
            let t = s.tracks.iter().find(|t| t.id.to_string() == id)?;
            return Some(CastSubtitle::Embedded { id: t.id, lang: t.lang.clone() });
        }
        let c = s.online.items.iter().find(|c| c.id == id)?;
        Some(CastSubtitle::Online { id: c.id.clone(), lang: Some(c.language.clone()) })
    }
}

// ---- Languages (subtitles.ts); the language list, `primary_language` and the browser's
// languages are the Settings page's ([`crate::screens::settings`]). ----

/// Primary code, plus the region for Portuguese and Chinese, the only ones told apart.
fn identity(tag: &str) -> (String, &'static str) {
    let text = tag.trim().to_lowercase().replace('_', "-");
    let primary = primary_language(&text);
    let region = match primary.as_str() {
        "pt" if text.contains("br") => "pt-BR",
        "pt" if text.split('-').count() > 1 => "pt-PT",
        "zh" if ["tw", "hk", "mo", "hant"].iter().any(|r| text.contains(r)) => "zh-TW",
        "zh" if ["cn", "sg", "hans"].iter().any(|r| text.contains(r)) => "zh-CN",
        _ => "",
    };
    (primary, region)
}

/// subtitles.ts `sameLanguage`: pt-BR and pt-PT differ, as do Simplified and Traditional
/// Chinese; a tag with no region matches either; other regions are the same language.
pub fn same_language(a: &str, b: &str) -> bool {
    let (l, lr) = identity(a);
    let (r, rr) = identity(b);
    !l.is_empty() && l == r && (lr.is_empty() || rr.is_empty() || lr == rr)
}

/// subtitles.ts `preferredLanguages`: the saved ones, else the browser's.
pub fn preferred_languages(settings: Option<&SubtitleSettings>, locales: &[String]) -> Vec<String> {
    match settings.map(|s| s.languages.clone()).filter(|l| !l.is_empty()) {
        Some(l) => l,
        None => browser_languages(locales),
    }
}

/// subtitles.ts `preferredTrack`: a full track (not forced) in the first language that has one.
pub fn preferred_track<'a>(tracks: &'a [JobTrack], languages: &[String]) -> Option<&'a JobTrack> {
    languages
        .iter()
        .find_map(|l| tracks.iter().find(|t| !t.forced && same_language(t.lang.as_deref().unwrap_or(""), l)))
}

/// subtitles.ts `autoSubtitle`: language by language, the video's own track and then an
/// online one; then the best online result, then any full track, then any track.
pub fn auto_subtitle(tracks: &[JobTrack], online: &[&SubtitleCandidate], languages: &[String]) -> Option<String> {
    for l in languages {
        if let Some(t) = preferred_track(tracks, std::slice::from_ref(l)) {
            return Some(format!("e:{}", t.id));
        }
        if let Some(c) = online.iter().find(|c| same_language(&c.language, l)) {
            return Some(format!("o:{}", c.id));
        }
    }
    if let Some(c) = online.first() {
        return Some(format!("o:{}", c.id));
    }
    tracks.iter().find(|t| !t.forced).or(tracks.first()).map(|t| format!("e:{}", t.id))
}

/// subtitles.ts `searchFor`: none when there is nothing to search by (a song, a file with
/// no title). Languages go along only when none are saved.
pub fn search_for(
    seed: &MediaInfo,
    release: Option<&str>,
    settings: Option<&SubtitleSettings>,
    locales: &[String],
) -> Option<SubtitleSearch> {
    let clean = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned);
    let episode = seed.kind == Some(MediaKind::Episode)
        || (seed.season.is_some() && seed.episode.is_some_and(|e| e > 0) && seed.kind != Some(MediaKind::Movie));
    let (title, show) = (clean(&seed.title), clean(&seed.show_title));
    if episode && !(show.is_some() && seed.season.is_some() && seed.episode.is_some_and(|e| e > 0)) {
        return None;
    }
    if !episode && (seed.kind != Some(MediaKind::Movie) || title.is_none()) {
        return None;
    }
    let mut body = SubtitleSearch {
        kind: Some(if episode { MediaKind::Episode } else { MediaKind::Movie }),
        title,
        show_title: show,
        year: seed.year.filter(|y| *y > 0),
        imdb_id: clean(&seed.imdb_id),
        tmdb_id: clean(&seed.tmdb_id),
        tvdb_id: clean(&seed.tvdb_id),
        release: release.map(str::trim).filter(|r| !r.is_empty()).map(str::to_owned),
        ..SubtitleSearch::default()
    };
    if episode {
        body.season = seed.season;
        body.episode = seed.episode;
    }
    if settings.is_none_or(|s| s.languages.is_empty()) {
        body.languages = browser_languages(locales);
    }
    Some(body)
}

/// subtitles.ts `cleanResults`: rows with an id, source and language; the best one named.
pub fn clean_results(r: SubtitleResults) -> (Vec<SubtitleCandidate>, Option<String>, Vec<String>) {
    let text = |s: &str| !s.trim().is_empty();
    let items: Vec<SubtitleCandidate> = r
        .items
        .into_iter()
        .filter(|c| text(&c.id) && text(&c.source) && text(&c.language))
        .map(|mut c| {
            if !text(&c.language_name) {
                c.language_name = c.language.clone();
            }
            c
        })
        .collect();
    let best = r.best.filter(|b| items.iter().any(|c| &c.id == b)).or_else(|| items.first().map(|c| c.id.clone()));
    let notes = r.notes.into_iter().filter(|n| text(n)).collect();
    (items, best, notes)
}

/// "English · 720p.BluRay.MACRO".
pub fn candidate_title(c: &SubtitleCandidate) -> String {
    match c.release.trim() {
        "" => c.language_name.clone(),
        r => format!("{} · {r}", c.language_name),
    }
}

/// "Gestdown · Hearing impaired · Best match": details in words (I1), not "HI".
pub fn candidate_detail(c: &SubtitleCandidate, best: bool) -> String {
    let mut parts = vec![c.source.clone()];
    if c.hearing_impaired {
        parts.push("Hearing impaired".into());
    }
    match c.matched {
        SubtitleMatch::Hash => parts.push("Exact match".into()),
        SubtitleMatch::Release => parts.push("Matches this release".into()),
        SubtitleMatch::Title => {}
    }
    if best {
        parts.push("Best match".into());
    }
    parts.join(" · ")
}

/// subtitles.ts `languageName`: the name of a 2- or 3-letter code, or "" when it has none.
pub fn language_name(ui: &Ui, code: &str) -> String {
    let code = code.trim();
    let ok = {
        let mut p = code.splitn(2, ['-', '_']);
        let main = p.next().unwrap_or("");
        (2..=3).contains(&main.len())
            && main.chars().all(|c| c.is_ascii_alphabetic())
            && p.next().is_none_or(|r| (2..=8).contains(&r.len()) && r.chars().all(|c| c.is_ascii_alphanumeric()))
    };
    if !ok {
        return String::new();
    }
    let name = untrack(|| remedy_ui::i18n::language_name(ui, code));
    if name.eq_ignore_ascii_case(code) { String::new() } else { name }
}

/// subtitles.ts `trackName`: the server's label, else its language named; "(forced)" added.
pub fn track_name(ui: &Ui, t: &JobTrack) -> String {
    let label = t.label.trim();
    let lang = t.lang.as_deref().unwrap_or("").trim();
    let base = if !label.is_empty() && !label.eq_ignore_ascii_case(lang) {
        label.to_owned()
    } else {
        let named = language_name(ui, if label.is_empty() { lang } else { label });
        match (named.is_empty(), label.is_empty(), lang.is_empty()) {
            (false, ..) => named,
            (true, false, _) => label.to_owned(),
            (true, true, false) => lang.to_owned(),
            _ => format!("Track {}", t.id),
        }
    };
    if t.forced && !base.to_lowercase().contains("forced") { format!("{base} (forced)") } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(id: u32, lang: &str, forced: bool) -> JobTrack {
        JobTrack { id, label: String::new(), lang: Some(lang.into()), forced, url: String::new() }
    }

    fn online(id: &str, lang: &str) -> SubtitleCandidate {
        SubtitleCandidate {
            id: id.into(),
            source: "Gestdown".into(),
            language: lang.into(),
            language_name: String::new(),
            release: String::new(),
            hearing_impaired: false,
            downloads: None,
            rating: None,
            score: 0,
            matched: SubtitleMatch::Title,
        }
    }

    #[test]
    fn languages_as_subtitles_ts() {
        assert_eq!(primary_language("eng"), "en");
        assert_eq!(primary_language("EN-us"), "en");
        assert!(same_language("pt", "pt-BR"));
        assert!(!same_language("pt-BR", "pt-PT"));
        assert!(same_language("en-US", "en-GB"));
        assert!(!same_language("zh-Hant", "zh-CN"));
        assert!(!same_language("", ""));
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(browser_languages(&l(&["en-US", "pt-BR", "en", "zh-TW", "de", "fr"])), l(&["en", "pt-BR", "zh-TW", "de"]));
        assert_eq!(browser_languages(&[]), l(&["en"]));
    }

    #[test]
    fn auto_picks_in_language_order() {
        let tracks = vec![track(1, "ger", false), track(2, "eng", true), track(3, "eng", false)];
        let (fr, en) = (online("a", "fr"), online("b", "en"));
        let langs = vec!["fr".to_string(), "en".to_string()];
        assert_eq!(auto_subtitle(&tracks, &[&en, &fr], &langs).as_deref(), Some("o:a"));
        assert_eq!(auto_subtitle(&tracks, &[&en], &langs).as_deref(), Some("e:3"));
        assert_eq!(auto_subtitle(&tracks[..2], &[], &["it".into()]).as_deref(), Some("e:1"));
        assert_eq!(auto_subtitle(&[], &[&en], &["it".into()]).as_deref(), Some("o:b"));
        assert_eq!(auto_subtitle(&[], &[], &langs), None);
    }

    #[test]
    fn search_body_as_subtitles_ts() {
        let ep = MediaInfo {
            kind: Some(MediaKind::Episode),
            title: Some("Pilot".into()),
            show_title: Some("Show".into()),
            season: Some(1),
            episode: Some(2),
            ..MediaInfo::default()
        };
        let b = search_for(&ep, Some(" Show.S01E02 "), None, &["de-DE".into()]).unwrap();
        assert_eq!((b.kind, b.season, b.episode), (Some(MediaKind::Episode), Some(1), Some(2)));
        assert_eq!(b.release.as_deref(), Some("Show.S01E02"));
        assert_eq!(b.languages, vec!["de".to_string()]);
        let saved = SubtitleSettings { languages: vec!["en".into()], ..SubtitleSettings::default() };
        assert!(search_for(&ep, None, Some(&saved), &[]).unwrap().languages.is_empty());
        let no_show = MediaInfo { show_title: None, ..ep };
        assert!(search_for(&no_show, None, None, &[]).is_none());
        let song = MediaInfo { title: Some("Song".into()), ..MediaInfo::default() };
        assert!(search_for(&song, None, None, &[]).is_none());
    }

    #[test]
    fn results_cleaned_and_named() {
        let mut bad = online("", "en");
        bad.source = "X".into();
        let r = SubtitleResults { items: vec![bad, online("b", "en")], best: Some("zz".into()), notes: vec![" ".into(), "Note".into()] };
        let (items, best, notes) = clean_results(r);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].language_name, "en");
        assert_eq!(best.as_deref(), Some("b"));
        assert_eq!(notes, vec!["Note".to_string()]);
        let mut c = online("c", "en");
        c.language_name = "English".into();
        c.release = "720p".into();
        c.hearing_impaired = true;
        c.matched = SubtitleMatch::Release;
        assert_eq!(candidate_title(&c), "English · 720p");
        assert_eq!(candidate_detail(&c, true), "Gestdown · Hearing impaired · Matches this release · Best match");
    }
}
