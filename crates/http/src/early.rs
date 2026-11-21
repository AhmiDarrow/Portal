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

fn lock() -> std::sync::MutexGuard<'static, Vec<Early>> {
    EARLY.lock().unwrap_or_else(|err| err.into_inner())
}

fn key(request: &PlayRequest) -> serde_json::Value {
    serde_json::to_value(request).unwrap_or_default()
}

fn stop_later(state: &AppState, job_id: Option<String>) {
    if let Some(job_id) = job_id {
        let state = state.clone();
        tokio::spawn(async move {
            let _ = state.0.player.stop(&job_id).await;
        });
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
        stop_later(state, old.job_id);
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
pub(crate) fn claim(state: &AppState, request: &PlayRequest) -> Option<String> {
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
        stop_later(state, early.job_id);
    }
    if let Some(id) = &found {
        tracing::info!(job = %id, "Playing the next episode resolved early");
    }
    found
}

/// Removes this early entry (if it is still the same one) and stops its job.
fn cancel(state: &AppState, for_job: &str, wanted: &serde_json::Value, why: &str) {
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
    stop_later(state, old.job_id);
}

async fn run(state: AppState, for_job: String, request: PlayRequest, wanted: serde_json::Value) {
    let started = crate::begin_play_job(&state, request).await;
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
                stop_later(&state, Some(id));
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
        if !crate::prefs::current(&state).autoplay {
            return cancel(&state, &for_job, &wanted, "autoplay is off");
        }
        if asked.elapsed() > UNASKED {
            return cancel(&state, &for_job, &wanted, "no longer near the end");
        }
        if state.0.player.job(&for_job).is_none() {
            let since = *orphan_since.get_or_insert_with(Instant::now);
            if since.elapsed() > ORPHAN {
                return cancel(&state, &for_job, &wanted, "the playing job ended");
            }
        }
        let Some(job) = state.0.player.job(&job_id) else {
            return cancel(&state, &for_job, &wanted, "it went away");
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
        let _ = state.0.player.stop(&job_id).await;
        if prompt.cancellable {
            let answer = PromptAnswer {
                id: prompt.id,
                action: PromptAction::Cancel,
                text: None,
                index: None,
            };
            let _ = state.0.prompts.answer(&answer).await;
        }
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
}
