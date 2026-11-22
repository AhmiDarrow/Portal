//! Player choices shared by every browser and TV (prefs.ts): quality, subtitles on or off
//! and the language last picked, the audio language last picked and Autoplay next, kept by
//! Portal (`/api/prefs`); the volume stays in each browser. The old page's own copy
//! (`portal-playback` in localStorage) is read at start, so this page starts with the
//! choices that browser last saw.
//!
//! ENGINE GAP: the copy cannot be written back (see `stored.rs`), so a volume set here lasts
//! until the page reloads, and an offline page does not remember a choice made here.

use std::cell::{Cell, RefCell};
use std::time::Duration;

use portal_api::{PrefsBody, PrefsPut, Quality, SubtitleMode};
use remedy_ui::reactive::{Cancel, Scope, Signal};

use crate::{api, Ctx};

/// The old page's localStorage item (prefs.ts `KEY`).
pub const KEY: &str = "portal-playback";
/// How long a choice waits before it goes to Portal, so quick changes go as one.
pub const PUT_DELAY: Duration = Duration::from_millis(300);
/// How long a play start waits for Portal's choices before going on with these.
pub const SYNC_WAIT: Duration = Duration::from_millis(1500);

/// prefs.ts `Prefs`.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub quality: Quality,
    pub subtitles: SubtitleMode,
    /// The user chose; before this a saved "auto" was only the old default.
    pub subtitles_set: bool,
    pub subtitle_language: Option<String>,
    pub audio_language: Option<String>,
    pub autoplay: bool,
    pub volume: f32,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            quality: Quality::Auto,
            subtitles: SubtitleMode::Off,
            subtitles_set: false,
            subtitle_language: None,
            audio_language: None,
            autoplay: true,
            volume: 1.0,
        }
    }
}

/// A change: each `Some` replaces that choice.
#[derive(Clone, Debug, Default)]
pub struct Patch {
    pub quality: Option<Quality>,
    pub subtitles: Option<SubtitleMode>,
    pub subtitle_language: Option<Option<String>>,
    pub audio_language: Option<Option<String>>,
    pub autoplay: Option<bool>,
    pub volume: Option<f32>,
}

/// prefs.ts `clampVolume`.
pub fn clamp_volume(v: f64) -> f32 {
    if !v.is_finite() {
        return 1.0;
    }
    v.clamp(0.0, 1.0) as f32
}

/// prefs.ts `cleanLanguage`: `en`, `pt-BR`; not `und`.
pub fn clean_language(v: Option<&str>) -> Option<String> {
    let text = v?.trim().replace('_', "-");
    let mut parts = text.splitn(2, '-');
    let main = parts.next().unwrap_or("");
    let ok_main = (2..=3).contains(&main.len()) && main.chars().all(|c| c.is_ascii_alphabetic());
    let ok_rest = parts.next().is_none_or(|r| (2..=8).contains(&r.len()) && r.chars().all(|c| c.is_ascii_alphanumeric()));
    (ok_main && ok_rest && !text.eq_ignore_ascii_case("und")).then_some(text)
}

/// The old page's copy (prefs.ts `readLocal`): subtitles start off unless chosen.
pub fn parse_local(raw: Option<&str>) -> Prefs {
    let Some(v) = raw.and_then(|r| serde_json::from_str::<serde_json::Value>(r).ok()) else {
        return Prefs::default();
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str());
    let saved = s("subtitles").unwrap_or("off");
    let set = v.get("subtitlesSet").and_then(|x| x.as_bool()) == Some(true);
    let on = saved != "off" && (saved != "auto" || set);
    let quality = s("quality").map(Quality::parse).filter(|q| s("quality") == Some(q.as_str())).unwrap_or_default();
    Prefs {
        quality,
        subtitles: if on { SubtitleMode::Auto } else { SubtitleMode::Off },
        subtitles_set: set || (saved != "off" && saved != "auto"),
        subtitle_language: clean_language(s("subtitleLanguage")),
        audio_language: clean_language(s("audioLanguage")),
        autoplay: v.get("autoplay").and_then(|x| x.as_bool()) != Some(false),
        volume: v.get("volume").and_then(|x| x.as_f64()).map_or(1.0, clamp_volume),
    }
}

struct Sync {
    /// Bumped by every change here, so an older answer from Portal cannot undo it.
    version: u64,
    pending: PrefsPut,
    timer: Option<u64>,
    first_done: bool,
}

thread_local! {
    static PREFS: Cell<Option<Signal<Prefs>>> = const { Cell::new(None) };
    static SYNC: RefCell<Sync> = RefCell::new(Sync { version: 0, pending: PrefsPut::default(), timer: None, first_done: false });
}

/// The choices, as a signal that lives as long as the page.
pub fn signal() -> Signal<Prefs> {
    PREFS.with(|p| {
        p.get().unwrap_or_else(|| {
            let (s, _) = Scope::detached(|| Signal::new(parse_local(crate::stored::item(KEY).as_deref())));
            p.set(Some(s));
            s
        })
    })
}

/// The choices now (prefs.ts `prefs()`), untracked.
pub fn now() -> Prefs {
    signal().get_untracked().unwrap_or_default()
}

fn pending_empty(p: &PrefsPut) -> bool {
    *p == PrefsPut::default()
}

/// Takes Portal's answer (prefs.ts `adopt`).
fn adopt(b: &PrefsBody) {
    let s = signal();
    let mut next = now();
    next.quality = b.prefs.quality;
    next.subtitles = b.prefs.subtitles;
    next.subtitles_set = true;
    next.subtitle_language = clean_language(b.prefs.subtitle_language.as_deref());
    next.audio_language = clean_language(b.prefs.audio_language.as_deref());
    next.autoplay = b.prefs.autoplay;
    s.set_if_changed(next);
}

/// This browser's own choices, for a Portal that has none saved yet (prefs.ts `migration`).
fn migration(l: &Prefs) -> PrefsPut {
    let mut out = PrefsPut::default();
    if l.quality != Quality::Auto {
        out.quality = Some(l.quality.as_str().into());
    }
    if !l.autoplay {
        out.autoplay = Some(false);
    }
    if l.subtitles_set {
        out.subtitles = Some(mode_str(l.subtitles).into());
        if let Some(lang) = &l.subtitle_language {
            out.subtitle_language = Some(Some(lang.clone()));
        }
    }
    if let Some(lang) = &l.audio_language {
        out.audio_language = Some(Some(lang.clone()));
    }
    out
}

fn mode_str(m: SubtitleMode) -> &'static str {
    match m {
        SubtitleMode::Off => "off",
        SubtitleMode::Auto => "auto",
    }
}

/// Reads Portal's choices (the first call also moves this browser's over when Portal has
/// none), then `done`, after at most [`SYNC_WAIT`]: a slow Portal never holds up playback,
/// and its late answer still applies when it comes (prefs.ts `syncPrefs`).
pub fn sync(ctx: &Ctx, done: impl FnOnce() + 'static) {
    let done = std::rc::Rc::new(RefCell::new(Some(Box::new(done) as Box<dyn FnOnce()>)));
    let d2 = done.clone();
    let ui = ctx.activity.ui();
    ui.after(SYNC_WAIT, move || {
        if let Some(f) = d2.borrow_mut().take() {
            f();
        }
    });
    let asked = SYNC.with(|s| s.borrow().version);
    let c = ctx.clone();
    ctx.api.run(api::prefs(), &Cancel::new(), move |out| {
        let finish = move || {
            if let Some(f) = done.borrow_mut().take() {
                f();
            }
        };
        let Ok(answer) = out else { return finish() };
        let (version, busy, first) = SYNC.with(|s| {
            let mut s = s.borrow_mut();
            let first = !s.first_done;
            s.first_done = true;
            (s.version, !pending_empty(&s.pending), first)
        });
        if asked != version || busy {
            return finish();
        }
        if !answer.stored && first {
            let moved = migration(&now());
            if !pending_empty(&moved) {
                c.api.run(api::set_prefs(&moved), &Cancel::new(), move |saved| {
                    if let Ok(saved) = saved {
                        if SYNC.with(|s| s.borrow().version) == asked {
                            adopt(&saved);
                        }
                    }
                    finish();
                });
                return;
            }
        }
        adopt(&answer);
        finish();
    });
}

/// Changes choices here at once and saves the shared ones to Portal shortly after
/// (prefs.ts `setPrefs`).
pub fn set(ctx: &Ctx, patch: Patch) {
    let mut p = now();
    let mut shared = PrefsPut::default();
    if let Some(q) = patch.quality {
        p.quality = q;
        shared.quality = Some(q.as_str().into());
    }
    if let Some(m) = patch.subtitles {
        p.subtitles = m;
        // Every caller of prefs.ts `setPrefs` that sets `subtitles` also sets `subtitlesSet`.
        p.subtitles_set = true;
        shared.subtitles = Some(mode_str(m).into());
    }
    if let Some(l) = patch.subtitle_language {
        p.subtitle_language = clean_language(l.as_deref());
        shared.subtitle_language = Some(p.subtitle_language.clone());
    }
    if let Some(l) = patch.audio_language {
        p.audio_language = clean_language(l.as_deref());
        shared.audio_language = Some(p.audio_language.clone());
    }
    if let Some(a) = patch.autoplay {
        p.autoplay = a;
        shared.autoplay = Some(a);
    }
    if let Some(v) = patch.volume {
        p.volume = v.clamp(0.0, 1.0);
    }
    SYNC.with(|s| s.borrow_mut().version += 1);
    signal().set_if_changed(p);
    if pending_empty(&shared) {
        return;
    }
    let ui = ctx.activity.ui();
    let old = SYNC.with(|s| {
        let mut s = s.borrow_mut();
        let pend = &mut s.pending;
        pend.quality = shared.quality.or(pend.quality.take());
        pend.subtitles = shared.subtitles.or(pend.subtitles.take());
        pend.subtitle_language = shared.subtitle_language.or(pend.subtitle_language.take());
        pend.audio_language = shared.audio_language.or(pend.audio_language.take());
        pend.autoplay = shared.autoplay.or(pend.autoplay.take());
        s.timer.take()
    });
    if let Some(t) = old {
        ui.cancel_timer(t);
    }
    let c = ctx.clone();
    let t = ui.after(PUT_DELAY, move || flush(&c));
    SYNC.with(|s| s.borrow_mut().timer = Some(t));
}

fn flush(ctx: &Ctx) {
    let (body, sent) = SYNC.with(|s| {
        let mut s = s.borrow_mut();
        s.timer = None;
        (std::mem::take(&mut s.pending), s.version)
    });
    if pending_empty(&body) {
        return;
    }
    ctx.api.run(api::set_prefs(&body), &Cancel::new(), move |out| {
        let Ok(saved) = out else { return };
        let fresh = SYNC.with(|s| {
            let s = s.borrow();
            s.version == sent && pending_empty(&s.pending)
        });
        if fresh {
            adopt(&saved);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_old_pages_copy_as_prefs_ts() {
        let p = parse_local(Some(r#"{"quality":"720","subtitles":"auto","autoplay":false,"volume":0.35,"audioLanguage":"pt_BR"}"#));
        assert_eq!(p.quality, Quality::P720);
        // A saved "auto" without subtitlesSet was only the old default.
        assert_eq!(p.subtitles, SubtitleMode::Off);
        assert!(!p.autoplay);
        assert!((p.volume - 0.35).abs() < 1e-6);
        assert_eq!(p.audio_language.as_deref(), Some("pt-BR"));
        assert_eq!(parse_local(Some("not json")), Prefs::default());
        assert_eq!(parse_local(Some(r#"{"quality":"9000","volume":7}"#)).quality, Quality::Auto);
        assert_eq!(parse_local(Some(r#"{"volume":7}"#)).volume, 1.0);
        assert_eq!(clean_language(Some("und")), None);
        assert_eq!(clean_language(Some("english")), None);
    }
}
