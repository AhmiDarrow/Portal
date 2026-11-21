//! The next episode's job, started shortly before the current one ends.
//!
//! Resolving an add-on source takes many seconds and used to start only
//! after the episode ended. Now the player (or a TV cast's watch) asks for
//! the next job while the end is near, and the play request that follows,
//! if it asks for exactly the same thing, gets that job instead of a new one.
//! The early job is dropped when the add-on would ask a question (the play
//! request then asks it as before), when autoplay is turned off, when the
//! playing job goes, when nobody has asked for it lately (a seek back, a
//! pause), or when another play request comes.
//!
//! Kodi has one dialog for everyone, so a background resolve yields to the
//! user: none starts while a user-started play is resolving, a user request
//! that drives Kodi is in flight, or any dialog is open; and every one stops
//! the moment such a request begins. It only ever cancels a dialog it is
//! sure is its own.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use portal_api::{JobState, PlayRequest, PromptAction, PromptAnswer, PromptKind};

use crate::AppState;

/// Seconds before the end when the next job starts: resolving takes ~20 s,
/// and the up-next card shows at 20 s, so the job is ready before either ends.
pub(crate) const LEAD: f64 = 60.0;
/// Asks arrive every few seconds while the end is near; a longer silence
/// means the viewer seeked back or paused.
const UNASKED: Duration = Duration::from_secs(20);
/// The player stops the old job just before it asks for the next one.
const ORPHAN: Duration = Duration::from_secs(15);
const TICK: Duration = Duration::from_secs(1);

struct Early {
    for_job: String,
    request: serde_json::Value,
    job_id: Option<String>,
    asked: Instant,
    /// The add-on asked a question: not tried again for this request.
    declined: bool,
}

static EARLY: Mutex<Vec<Early>> = Mutex::new(Vec::new());
/// User requests that drive Kodi (browse, search, settings, a prompt
/// answer, a play starting) in flight.
static USER_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// Bumped as each begins: a background resolve that saw another value yields.
static USER_GEN: AtomicU64 = AtomicU64::new(0);
/// User-started play jobs; busy while they resolve.
static USER_JOBS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Held for the length of a user request that drives Kodi.
pub(crate) struct UserAction;

impl Drop for UserAction {
    fn drop(&mut self) {
        USER_ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Makes a background "is this dialog ours? then cancel it" and a user
/// request beginning mutually exclusive. The background holds it across its
/// check and its cancel; a user request takes it only to mark its start, so
/// it waits at most for one cancel already under way.
static KODI_DIALOG: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn begin_user() -> UserAction {
    USER_ACTIVE.fetch_add(1, Ordering::SeqCst);
    USER_GEN.fetch_add(1, Ordering::SeqCst);
    UserAction
}

/// Marks a user request's start, after any background cancel in progress.
async fn begin_user_locked() -> UserAction {
    let _dialog = KODI_DIALOG.lock().await;
    begin_user()
}

/// Runs `cancel` only if no user request has begun since `gen`, none is in
/// flight and `idle` agrees, all under the dialog lock, so no user request
/// can begin between the check and the cancel. True when it ran.
async fn as_background<F: std::future::Future<Output = ()>>(
    gen: u64,
    idle: impl FnOnce() -> bool,
    cancel: F,
) -> bool {
    let _dialog = KODI_DIALOG.lock().await;
    if USER_GEN.load(Ordering::SeqCst) != gen || USER_ACTIVE.load(Ordering::SeqCst) != 0 || !idle()
    {
        return false;
    }
    cancel.await;
    true
}

/// Who may close a dialog a stopped background resolve left open.
#[derive(Clone, Copy)]
enum Dialogs {
    /// Nobody: the open dialog may be the one the user is answering.
    Leave,
    /// The user's request, before it touches Kodi: every dialog open now is the background's.
    User,
    /// The background, only while no user request has begun since this generation.
    Background(u64),
}

/// Every background entry, removed; their jobs are for the caller to stop.
fn drain() -> Vec<Option<String>> {
    std::mem::take(&mut *lock())
        .into_iter()
        .map(|early| early.job_id)
        .collect()
}

/// A user request that drives Kodi begins: every background resolve stops
/// now, and none starts until it ends. With `dialogs`, a dialog the stopped
/// resolve left open is closed before the request goes on; a prompt answer
/// passes false, since the open dialog is the one the user is answering.
pub(crate) async fn user_action(state: &AppState, dialogs: bool) -> UserAction {
    let action = begin_user_locked().await;
    let dialogs = if dialogs {
        Dialogs::User
    } else {
        Dialogs::Leave
    };
    for job in drain().into_iter().flatten() {
        let begun = Instant::now();
        release(state, &job, dialogs).await;
        tracing::info!(job = %job, took_ms = begun.elapsed().as_millis() as u64, "Early next episode dropped: the user is using Kodi");
    }
    action
}

/// A play the user (or the cast's own autoplay) started.
pub(crate) fn user_job(id: &str) {
    USER_JOBS
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .push(id.to_string());
}

/// Whether a user-started play is still resolving; finished ones are forgotten.
fn user_jobs_busy(state_of: impl Fn(&str) -> Option<JobState>) -> bool {
    let mut jobs = USER_JOBS.lock().unwrap_or_else(|err| err.into_inner());
    jobs.retain(|id| matches!(state_of(id), Some(JobState::Working | JobState::Prompt)));
    !jobs.is_empty()
}

/// Nothing of the user's is using Kodi, so a background resolve may run.
async fn user_idle(state: &AppState) -> bool {
    if USER_ACTIVE.load(Ordering::SeqCst) > 0
        || user_jobs_busy(|id| state.0.player.job(id).map(|job| job.state))
    {
        return false;
    }
    // Any dialog at all, or one that cannot be read, may be the user's.
    matches!(state.0.prompts.read().await, Ok(None))
}

fn lock() -> std::sync::MutexGuard<'static, Vec<Early>> {
    EARLY.lock().unwrap_or_else(|err| err.into_inner())
}

fn key(request: &PlayRequest) -> serde_json::Value {
    serde_json::to_value(request).unwrap_or_default()
}

/// No user request began since `gen` or is in flight, and no user play is
/// resolving: a dialog open now came from the background resolve.
fn still_ours(state: &AppState, gen: u64) -> bool {
    USER_GEN.load(Ordering::SeqCst) == gen
        && USER_ACTIVE.load(Ordering::SeqCst) == 0
        && !user_jobs_busy(|id| state.0.player.job(id).map(|job| job.state))
}

async fn cancel_open_dialog(state: &AppState) {
    if let Ok(Some(prompt)) = state.0.prompts.read().await {
        if prompt.cancellable {
            let answer = PromptAnswer {
                id: prompt.id,
                action: PromptAction::Cancel,
                text: None,
                index: None,
            };
            let _ = state.0.prompts.answer(&answer).await;
        }
    }
}

/// Stops a background job. The add-on goes on inside Kodi after its job
/// stops, so with `dialogs` a dialog it has open is closed too, leaving
/// nothing of a background resolve on screen.
async fn release(state: &AppState, job_id: &str, dialogs: Dialogs) {
    let resolving = state
        .0
        .player
        .job(job_id)
        .is_some_and(|job| matches!(job.state, JobState::Working | JobState::Prompt));
    let _ = state.0.player.stop(job_id).await;
    if matches!(dialogs, Dialogs::Leave) || !resolving {
        return;
    }
    // The add-on can be between two dialogs as its job stops, so it gets a
    // second look; a short one, since a user request may be waiting on this.
    for look in 0..2 {
        if look > 0 {
            tokio::time::sleep(DIALOG_RELOOK).await;
        }
        match dialogs {
            Dialogs::User => cancel_open_dialog(state).await,
            Dialogs::Background(gen) => {
                let idle = || !user_jobs_busy(|id| state.0.player.job(id).map(|job| job.state));
                if !as_background(gen, idle, cancel_open_dialog(state)).await {
                    return;
                }
            }
            Dialogs::Leave => return,
        }
    }
}

const DIALOG_RELOOK: Duration = Duration::from_millis(400);

fn stop_later(state: &AppState, job_id: Option<String>, dialogs: Dialogs) {
    if let Some(job_id) = job_id {
        let state = state.clone();
        tokio::spawn(async move { release(&state, &job_id, dialogs).await });
    }
}

/// `for_job` is near its end and `request` is what plays next. Starts that
/// job, or keeps the one already started for it.
pub(crate) fn want(state: &AppState, for_job: &str, request: PlayRequest) {
    if !crate::prefs::current(state).autoplay {
        return;
    }
    let wanted = key(&request);
    let mut list = lock();
    if let Some(at) = list.iter().position(|early| early.for_job == for_job) {
        if list[at].request == wanted {
            list[at].asked = Instant::now();
            return;
        }
        // Another next (a quality change, a next season found late).
        let old = list.remove(at);
        stop_later(
            state,
            old.job_id,
            Dialogs::Background(USER_GEN.load(Ordering::SeqCst)),
        );
    }
    list.push(Early {
        for_job: for_job.to_string(),
        request: wanted.clone(),
        job_id: None,
        asked: Instant::now(),
        declined: false,
    });
    drop(list);
    tokio::spawn(run(state.clone(), for_job.to_string(), request, wanted));
}

/// The early job for exactly this request, if there is a usable one. Every
/// other early job is dropped, so it does not hold Kodi from this request.
pub(crate) async fn claim(state: &AppState, request: &PlayRequest) -> Option<String> {
    let wanted = key(request);
    let taken = std::mem::take(&mut *lock());
    let mut found = None;
    for early in taken {
        if found.is_none() && early.request == wanted && !early.declined {
            let usable = early
                .job_id
                .as_deref()
                .and_then(|id| state.0.player.job(id));
            if usable.is_some_and(|job| job.state != JobState::Failed) {
                found = early.job_id;
                continue;
            }
        }
        // The user's play has not touched Kodi yet: any dialog is the background's.
        if let Some(job) = early.job_id {
            release(state, &job, Dialogs::User).await;
        }
    }
    if let Some(id) = &found {
        tracing::info!(job = %id, "Playing the next episode resolved early");
    }
    found
}

/// Removes this early entry (if it is still the same one) and stops its job.
fn cancel(
    state: &AppState,
    for_job: &str,
    wanted: &serde_json::Value,
    why: &str,
    dialogs: Dialogs,
) {
    let mut list = lock();
    let Some(at) = list
        .iter()
        .position(|early| early.for_job == for_job && early.request == *wanted)
    else {
        return;
    };
    let old = list.remove(at);
    drop(list);
    tracing::info!(job = ?old.job_id, "Early next episode dropped: {why}");
    stop_later(state, old.job_id, dialogs);
}

async fn run(state: AppState, for_job: String, request: PlayRequest, wanted: serde_json::Value) {
    let gen = USER_GEN.load(Ordering::SeqCst);
    if !user_idle(&state).await {
        // Asked again in a few seconds; it starts once the user is done.
        return cancel(
            &state,
            &for_job,
            &wanted,
            "the user is using Kodi",
            Dialogs::Leave,
        );
    }
    let started = crate::begin_play_job(&state, request).await;
    if USER_GEN.load(Ordering::SeqCst) != gen {
        if let Ok(id) = &started {
            stop_later(&state, Some(id.clone()), Dialogs::Leave);
        }
        return cancel(
            &state,
            &for_job,
            &wanted,
            "the user began using Kodi",
            Dialogs::Leave,
        );
    }
    let job_id = {
        let mut list = lock();
        let entry = list
            .iter_mut()
            .find(|early| early.for_job == for_job && early.request == wanted);
        match (entry, started) {
            (Some(entry), Ok(id)) => {
                entry.job_id = Some(id.clone());
                id
            }
            (None, Ok(id)) => {
                // Claimed too soon or dropped while starting.
                drop(list);
                stop_later(&state, Some(id), Dialogs::Leave);
                return;
            }
            (entry, Err(_)) => {
                // Not tried again for this request; the play request at the end tries as before.
                if let Some(entry) = entry {
                    entry.declined = true;
                }
                return;
            }
        }
    };
    tracing::info!(job = %job_id, for_job = %for_job, "Resolving the next episode early");
    let mut orphan_since: Option<Instant> = None;
    loop {
        tokio::time::sleep(TICK).await;
        let asked = {
            let list = lock();
            match list
                .iter()
                .find(|early| early.job_id.as_deref() == Some(job_id.as_str()))
            {
                Some(early) => early.asked,
                None => return, // claimed or dropped
            }
        };
        if USER_GEN.load(Ordering::SeqCst) != gen {
            return cancel(
                &state,
                &for_job,
                &wanted,
                "the user began using Kodi",
                Dialogs::Leave,
            );
        }
        if !crate::prefs::current(&state).autoplay {
            return cancel(
                &state,
                &for_job,
                &wanted,
                "autoplay is off",
                Dialogs::Background(gen),
            );
        }
        if asked.elapsed() > UNASKED {
            return cancel(
                &state,
                &for_job,
                &wanted,
                "no longer near the end",
                Dialogs::Background(gen),
            );
        }
        if state.0.player.job(&for_job).is_none() {
            let since = *orphan_since.get_or_insert_with(Instant::now);
            if since.elapsed() > ORPHAN {
                return cancel(
                    &state,
                    &for_job,
                    &wanted,
                    "the playing job ended",
                    Dialogs::Background(gen),
                );
            }
        }
        let Some(job) = state.0.player.job(&job_id) else {
            return cancel(
                &state,
                &for_job,
                &wanted,
                "it went away",
                Dialogs::Background(gen),
            );
        };
        if job.state != JobState::Working {
            continue;
        }
        // A question nobody asked for is never shown: cancel it, and let the
        // play request at the end ask it as before.
        let Ok(Some(prompt)) = state.0.prompts.read().await else {
            continue;
        };
        let prompt = crate::picking_view(&state, prompt);
        if prompt.kind == PromptKind::Progress {
            continue;
        }
        // Only a dialog that can be nobody else's is cancelled: no user
        // request began or is in flight, no user play is resolving, and the
        // same dialog is still the one open.
        let ours = still_ours(&state, gen)
            && matches!(state.0.prompts.read().await, Ok(Some(open)) if open.id == prompt.id);
        if !ours {
            return cancel(
                &state,
                &for_job,
                &wanted,
                "a dialog that may be the user's is open",
                Dialogs::Leave,
            );
        }
        {
            let mut list = lock();
            if let Some(early) = list
                .iter_mut()
                .find(|early| early.job_id.as_deref() == Some(job_id.as_str()))
            {
                early.declined = true;
                early.job_id = None;
            }
        }
        tracing::info!(job = %job_id, "Early next episode dropped: the add-on asked a question");
        release(&state, &job_id, Dialogs::Background(gen)).await;
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_request_same_key() {
        let request: PlayRequest = serde_json::from_value(serde_json::json!({
            "kind": "episode", "id": "show", "file": "plugin://x/?e=2", "title": "Two",
            "season": 4, "episode": 2,
        }))
        .unwrap();
        let mut other = request.clone();
        assert_eq!(key(&request), key(&other));
        other.resume_seconds = Some(120.0);
        assert_ne!(key(&request), key(&other));
    }

    fn entry(job: &str) -> Early {
        Early {
            for_job: "playing".into(),
            request: serde_json::Value::Null,
            job_id: Some(job.into()),
            asked: Instant::now(),
            declined: false,
        }
    }

    /// The guard is process-wide state, so its tests take turns.
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn background_resolves_yield_to_the_user_in_either_order() {
        let _serial = SERIAL.lock().unwrap_or_else(|err| err.into_inner());
        // The user first: a request in flight, or a user play still resolving,
        // keeps a background resolve from starting.
        let action = begin_user();
        assert!(USER_ACTIVE.load(Ordering::SeqCst) > 0);
        drop(action);
        assert_eq!(USER_ACTIVE.load(Ordering::SeqCst), 0);
        user_job("user-play");
        assert!(user_jobs_busy(|_| Some(JobState::Working)));
        assert!(user_jobs_busy(|_| Some(JobState::Prompt)));
        assert!(!user_jobs_busy(|_| Some(JobState::Ready)));
        assert!(
            !user_jobs_busy(|_| Some(JobState::Working)),
            "a finished user play is forgotten"
        );

        // The background first: a user request stops it at once and tells a
        // running one (by the generation) to yield, never touching the user's.
        lock().push(entry("background"));
        let gen = USER_GEN.load(Ordering::SeqCst);
        let action = begin_user();
        assert_eq!(drain(), vec![Some("background".to_string())]);
        assert!(lock().is_empty());
        assert_ne!(USER_GEN.load(Ordering::SeqCst), gen);
        drop(action);
    }

    /// A background check-and-cancel and a user request beginning, raced
    /// many times on several threads: a cancel never lands after a user
    /// request has begun, and a user request waits at most for one cancel.
    #[test]
    fn a_background_cancel_never_lands_after_a_user_request_began() {
        use std::sync::Arc;
        let _serial = SERIAL.lock().unwrap_or_else(|err| err.into_inner());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let (mut cancelled, mut yielded) = (0, 0);
        runtime.block_on(async {
            for round in 0..2000u64 {
                let clock = Arc::new(AtomicU64::new(1));
                let cancel_at = Arc::new(AtomicU64::new(0));
                let gen = USER_GEN.load(Ordering::SeqCst);
                let background = {
                    let (clock, cancel_at) = (clock.clone(), cancel_at.clone());
                    tokio::spawn(async move {
                        for _ in 0..round % 4 {
                            tokio::task::yield_now().await;
                        }
                        as_background(gen, || true, async move {
                            // A cancel takes a while: room for a user request to slip in.
                            tokio::task::yield_now().await;
                            cancel_at.store(clock.fetch_add(1, Ordering::SeqCst), Ordering::SeqCst);
                        })
                        .await
                    })
                };
                let user = {
                    let clock = clock.clone();
                    tokio::spawn(async move {
                        for _ in 0..(round / 4) % 4 {
                            tokio::task::yield_now().await;
                        }
                        let action = begin_user_locked().await;
                        // The user's request touches Kodi from here on.
                        let began = clock.fetch_add(1, Ordering::SeqCst);
                        drop(action);
                        began
                    })
                };
                let ran = background.await.unwrap();
                let began = user.await.unwrap();
                if ran {
                    cancelled += 1;
                    assert!(
                        cancel_at.load(Ordering::SeqCst) < began,
                        "a cancel landed after the user began"
                    );
                } else {
                    yielded += 1;
                }
            }
        });
        assert_eq!(cancelled + yielded, 2000);
        assert!(cancelled > 0 && yielded > 0, "both orders were raced");
        eprintln!(
            "raced 2000 times: {cancelled} cancels before the user, {yielded} yielded to the user"
        );
    }
}
