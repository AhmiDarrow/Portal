use super::*;
use portal_api::JobState;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const MEDIA: &str = "http://192.0.2.46:8490/media/aaa/index.m3u8";
const NEXT_MEDIA: &str = "http://192.0.2.46:8490/media/bbb/index.m3u8";

fn status(state: &str, reason: Option<&str>, at: f64, length: f64, content: &str) -> DeviceStatus {
    DeviceStatus {
        state: state.into(),
        idle_reason: reason.map(str::to_string),
        current_time: Some(at),
        duration: Some(length),
        content_id: Some(content.into()),
        volume: None,
        muted: None,
    }
}

fn playing(at: f64) -> DeviceStatus {
    status("PLAYING", None, at, 1200.0, MEDIA)
}

#[test]
fn helper_lines_parse() {
    // The shape `cast_agent.py status` and `watch` print.
    let line = serde_json::json!({
        "ok": true, "app": "CC1AD845", "state": "PLAYING", "idle_reason": null,
        "current_time": 61.25, "duration": 1320.5, "content_id": MEDIA,
        "volume": 0.35, "muted": false
    });
    let parsed = status_from_value(&line).unwrap();
    assert_eq!(parsed.state, "PLAYING");
    assert_eq!(parsed.idle_reason, None);
    assert_eq!(parsed.current_time, Some(61.25));
    assert_eq!(parsed.duration, Some(1320.5));
    assert_eq!(parsed.content_id.as_deref(), Some(MEDIA));
    assert_eq!(parsed.volume, Some(0.35));
    assert_eq!(parsed.muted, Some(false));

    let idle = serde_json::json!({"ok": true, "app": null, "state": "UNKNOWN", "idle_reason": "finished",
            "current_time": -3, "duration": null, "content_id": ""});
    let parsed = status_from_value(&idle).unwrap();
    assert_eq!(parsed.state, "");
    assert_eq!(parsed.idle_reason.as_deref(), Some("FINISHED"));
    assert_eq!(parsed.current_time, None);
    assert_eq!(parsed.content_id, None);
    // An older helper prints no volume.
    assert_eq!(parsed.volume, None);
    assert_eq!(parsed.muted, None);

    let err = serde_json::json!({"ok": false, "error": "The Cast device is not answering.", "code": "unavailable"});
    assert_eq!(
        status_from_value(&err).unwrap_err(),
        "The Cast device is not answering."
    );
    assert!(status_from_value(&serde_json::json!({})).is_err());
}

#[test]
fn finished_needs_the_item_to_have_played() {
    let mut tracker = Tracker::new(MEDIA);
    // An old FINISHED from before the load is not the end of this one.
    assert_eq!(
        tracker.observe(&status("IDLE", Some("FINISHED"), 0.0, 0.0, MEDIA)),
        None
    );
    assert_eq!(
        tracker.observe(&status("BUFFERING", None, 0.0, 1200.0, MEDIA)),
        None
    );
    assert_eq!(tracker.observe(&playing(600.0)), None);
    assert_eq!(
        tracker.observe(&status("IDLE", Some("FINISHED"), 0.0, 1200.0, MEDIA)),
        Some(Ending::Finished)
    );
}

#[test]
fn idle_without_a_reason_is_finished_only_near_the_end() {
    let mut near = Tracker::new(MEDIA);
    near.observe(&playing(1190.0));
    assert_eq!(
        near.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)),
        Some(Ending::Finished)
    );

    // Some TVs close the receiver app at the end: no media, no content id.
    let mut closed = Tracker::new(MEDIA);
    closed.observe(&playing(1195.0));
    assert_eq!(
        closed.observe(&DeviceStatus::default()),
        Some(Ending::Finished)
    );

    // A growing playlist plays as live with no duration; the job's length stands in.
    let mut live = Tracker::new(MEDIA).with_length(Some(1200.0));
    live.observe(&status("PLAYING", None, 1192.0, 0.0, MEDIA));
    assert_eq!(
        live.observe(&DeviceStatus::default()),
        Some(Ending::Finished)
    );

    let mut early = Tracker::new(MEDIA);
    early.observe(&playing(300.0));
    assert_eq!(early.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)), None);
    assert_eq!(early.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)), None);
    assert_eq!(
        early.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)),
        Some(Ending::Closed)
    );
    let mut gone = Tracker::new(MEDIA);
    gone.observe(&playing(300.0));
    assert_eq!(gone.observe(&DeviceStatus::default()), None);
    assert_eq!(gone.observe(&DeviceStatus::default()), Some(Ending::Closed));
}

#[test]
fn a_stop_error_or_other_cast_is_not_finished() {
    let mut stopped = Tracker::new(MEDIA);
    stopped.observe(&playing(1195.0));
    assert_eq!(
        stopped.observe(&status("IDLE", Some("CANCELLED"), 0.0, 1200.0, MEDIA)),
        Some(Ending::Stopped)
    );
    let mut error = Tracker::new(MEDIA);
    error.observe(&playing(10.0));
    assert_eq!(
        error.observe(&status("IDLE", Some("ERROR"), 0.0, 1200.0, MEDIA)),
        Some(Ending::Failed)
    );
    let mut other = Tracker::new(MEDIA);
    other.observe(&playing(1195.0));
    assert_eq!(
        other.observe(&status(
            "PLAYING",
            None,
            5.0,
            60.0,
            "http://example.invalid/other.mp4"
        )),
        Some(Ending::Replaced)
    );
    let mut interrupted = Tracker::new(MEDIA);
    interrupted.observe(&playing(20.0));
    let interrupt = status("IDLE", Some("INTERRUPTED"), 0.0, 0.0, MEDIA);
    // A seek reloads the stream: one INTERRUPTED line is not the end.
    assert_eq!(interrupted.observe(&interrupt), None);
    assert_eq!(interrupted.observe(&playing(300.0)), None);
    assert_eq!(interrupted.observe(&interrupt), None);
    assert_eq!(interrupted.observe(&interrupt), None);
    assert_eq!(interrupted.observe(&interrupt), Some(Ending::Replaced));
    // The same file with another query string is still ours.
    let mut query = Tracker::new(MEDIA);
    assert_eq!(
        query.observe(&status(
            "PLAYING",
            None,
            1.0,
            1200.0,
            &format!("{MEDIA}?t=1")
        )),
        None
    );
}

#[test]
fn a_short_cast_file_can_finish_unseen_and_a_reload_is_not_an_ending() {
    let file = "http://192.0.2.46:8490/media/abc/cast.mp4?v=8.000";
    let older = "http://192.0.2.46:8490/media/abc/cast.mp4?v=4.000";
    // The file before this one finishing is not this one finishing.
    let mut tracker = Tracker::new(file);
    assert_eq!(
        tracker.observe(&status("IDLE", Some("FINISHED"), 0.0, 0.0, older)),
        None
    );
    // Loading the longer file interrupts the short one.
    assert_eq!(
        tracker.observe(&status("IDLE", Some("INTERRUPTED"), 0.0, 0.0, older)),
        None
    );
    assert_eq!(
        tracker.observe(&status("IDLE", Some("FINISHED"), 0.0, 0.0, file)),
        Some(Ending::Finished)
    );
}

#[test]
fn a_silent_device_is_lost_and_a_stuck_load_gives_up() {
    let mut tracker = Tracker::new(MEDIA);
    tracker.observe(&playing(10.0));
    for _ in 1..LOST_POLLS {
        assert_eq!(tracker.failed(), None);
    }
    assert_eq!(tracker.failed(), Some(Ending::Lost));
    // A good line in between starts the count again.
    let mut again = Tracker::new(MEDIA);
    for _ in 1..LOST_POLLS {
        again.failed();
    }
    again.observe(&playing(11.0));
    assert_eq!(again.failed(), None);

    let mut stuck = Tracker::new(MEDIA);
    for _ in 1..START_POLLS {
        assert_eq!(stuck.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)), None);
    }
    assert_eq!(
        stuck.observe(&status("IDLE", None, 0.0, 0.0, MEDIA)),
        Some(Ending::NeverStarted)
    );
}

fn next_item(n: u32) -> NextItem {
    NextItem {
        kind: PlayKind::Episode,
        id: format!("ep{n}"),
        file: format!("plugin://plugin.video.example/?action=play&ep={n}"),
        title: format!("Episode {n}"),
        season: Some(1),
        episode: Some(n),
        show_id: Some("show-1".into()),
        show_title: Some("Example Show".into()),
        art: None,
    }
}

fn ready_job(id: &str, next: Option<NextItem>) -> PlayJob {
    PlayJob {
        id: id.into(),
        state: JobState::Ready,
        error: None,
        session: Some("bbb".into()),
        playlist: Some("/media/bbb/index.m3u8".into()),
        subtitles: Vec::new(),
        subtitles_pending: false,
        next,
        browse_path: None,
        title: Some("Example Show S01E03".into()),
        position: Some(0.0),
        duration: Some(1300.0),
        release: None,
        audio: Vec::new(),
        audio_index: None,
        prompt: None,
        prompt_known: false,
    }
}

fn watched() -> Watched {
    Watched {
        device: DeviceRef {
            id: "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c".into(),
            name: "Living room".into(),
            endpoint: None,
        },
        job_id: "job-2".into(),
        media_url: MEDIA.into(),
        origin: 0.0,
        job_duration: Some(1200.0),
        item: Some(replay_from_next(&next_item(2))),
        next: Some(next_item(3)),
        quality: Quality::P720,
    }
}

#[derive(Default)]
struct Fake {
    autoplay: bool,
    /// Status scripts, one per `watch` call.
    scripts: Mutex<VecDeque<Vec<Result<DeviceStatus, String>>>>,
    /// Steps per job id; the last one repeats.
    steps: Mutex<HashMap<String, VecDeque<JobStep>>>,
    started: Mutex<Vec<PlayRequest>>,
    stopped: Mutex<Vec<String>>,
    casts: Mutex<Vec<(String, String, String)>>,
    cast_fails: bool,
    dismissed: Mutex<u32>,
    reports: Mutex<Vec<ProgressReport>>,
    updates: Mutex<Vec<Update>>,
}

struct Script(VecDeque<Result<DeviceStatus, String>>);

impl StatusSource for Script {
    async fn next(&mut self) -> Result<DeviceStatus, String> {
        match self.0.pop_front() {
            Some(line) => line,
            // Out of lines: the device keeps playing until the test cancels.
            None => std::future::pending().await,
        }
    }
}

impl Backend for Fake {
    type Source = Script;

    fn watch(&self, _device: &DeviceRef) -> Script {
        Script(
            self.scripts
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default()
                .into(),
        )
    }

    fn autoplay(&self) -> bool {
        self.autoplay
    }

    async fn job(&self, id: &str) -> JobStep {
        let mut steps = self.steps.lock().unwrap();
        let Some(queue) = steps.get_mut(id) else {
            return JobStep::Gone;
        };
        if queue.len() > 1 {
            queue.pop_front().unwrap()
        } else {
            queue.front().cloned().unwrap_or(JobStep::Gone)
        }
    }

    async fn start(&self, request: PlayRequest) -> Result<String, String> {
        let mut started = self.started.lock().unwrap();
        started.push(request);
        Ok(format!("job-{}", 2 + started.len()))
    }

    async fn stop(&self, id: &str) {
        self.stopped.lock().unwrap().push(id.to_string());
    }

    async fn dismiss_prompt(&self) {
        *self.dismissed.lock().unwrap() += 1;
    }

    async fn cast(
        &self,
        device: &DeviceRef,
        job: &PlayJob,
        title: &str,
        _art: Option<&str>,
    ) -> Result<String, String> {
        if self.cast_fails {
            return Err("The TV did not start playback.".into());
        }
        self.casts
            .lock()
            .unwrap()
            .push((device.id.clone(), job.id.clone(), title.to_string()));
        Ok(NEXT_MEDIA.into())
    }

    fn progress(&self, report: &ProgressReport) {
        self.reports.lock().unwrap().push(report.clone());
    }

    fn publish(&self, _gen: u64, update: Update) {
        self.updates.lock().unwrap().push(update);
    }
}

fn finish_script() -> Vec<Result<DeviceStatus, String>> {
    vec![
        Ok(status("BUFFERING", None, 0.0, 1200.0, MEDIA)),
        Ok(playing(600.0)),
        Err("The Cast device is not answering.".into()),
        Ok(playing(1190.0)),
        Ok(status("IDLE", Some("FINISHED"), 0.0, 1200.0, MEDIA)),
    ]
}

fn fake(autoplay: bool) -> Fake {
    let fake = Fake {
        autoplay,
        ..Fake::default()
    };
    fake.scripts.lock().unwrap().push_back(finish_script());
    fake
}

#[tokio::test(start_paused = true)]
async fn a_finished_episode_casts_the_next_one_to_the_same_device() {
    let fake = fake(true);
    // The next episode's own job leads on to episode 4, then its cast plays and is stopped on the TV.
    fake.steps.lock().unwrap().insert(
        "job-3".into(),
        VecDeque::from([
            JobStep::Working,
            JobStep::Prompt,
            JobStep::Working,
            JobStep::Ready(Box::new(ready_job("job-3", Some(next_item(4))))),
        ]),
    );
    fake.scripts.lock().unwrap().push_back(vec![
        Ok(status("PLAYING", None, 3.0, 1300.0, NEXT_MEDIA)),
        Ok(status("IDLE", Some("CANCELLED"), 0.0, 1300.0, NEXT_MEDIA)),
    ]);
    let (_tx, rx) = watch::channel(false);
    let outcome = drive(&fake, 7, watched(), rx).await;
    assert_eq!(
        outcome,
        Outcome::Ended {
            ending: Ending::Stopped,
            chained: 1
        }
    );

    let started = fake.started.lock().unwrap();
    assert_eq!(started.len(), 1);
    let request = &started[0];
    assert_eq!(request.kind, PlayKind::Episode);
    assert_eq!(request.id, "show-1");
    assert_eq!(request.show_id.as_deref(), Some("show-1"));
    assert_eq!(request.episode, Some(3));
    assert_eq!(request.quality, Quality::P720);
    assert_eq!(request.resume_seconds, None);
    assert!(!request.manual_links);

    let casts = fake.casts.lock().unwrap();
    assert_eq!(casts.len(), 1);
    assert_eq!(casts[0].0, "6b86b273-ff34-4b1a-9b0a-0c0c0c0c0c0c");
    assert_eq!(casts[0].1, "job-3");
    // The finished job is let go once the next one is on the TV.
    assert_eq!(*fake.stopped.lock().unwrap(), vec!["job-2".to_string()]);

    let updates = fake.updates.lock().unwrap();
    let next_at = updates
        .iter()
        .position(|update| matches!(update, Update::Next { job_id, .. } if job_id == "job-3"))
        .unwrap();
    let casting_at = updates.iter().position(|update| matches!(update, Update::Casting(watched) if watched.job_id == "job-3" && watched.media_url == NEXT_MEDIA && watched.next.as_ref().and_then(|next| next.episode) == Some(4))).unwrap();
    assert!(next_at < casting_at);
    assert!(updates.iter().any(|update| matches!(update, Update::Status { state: DeviceState::Playing, position: Some(at), .. } if *at == 600.0)));
    assert!(matches!(
        updates.last(),
        Some(Update::Ended {
            state: DeviceState::Stopped,
            ..
        })
    ));

    // The finished episode is saved as watched to the end.
    let reports = fake.reports.lock().unwrap();
    let last = reports.last().unwrap();
    assert_eq!(last.id, "show-1");
    assert_eq!(last.kind, TitleKind::Tv);
    assert_eq!(last.name, "Example Show");
    assert_eq!(last.episode, Some(2));
    assert_eq!(last.position, last.duration);
}

#[tokio::test(start_paused = true)]
async fn autoplay_off_ends_after_the_episode() {
    let fake = fake(false);
    let (_tx, rx) = watch::channel(false);
    let outcome = drive(&fake, 1, watched(), rx).await;
    assert_eq!(
        outcome,
        Outcome::Done {
            chained: 0,
            autoplay: false
        }
    );
    assert!(fake.started.lock().unwrap().is_empty());
    assert!(
        matches!(fake.updates.lock().unwrap().last(), Some(Update::Ended { state: DeviceState::Ended, note }) if note.contains("Autoplay next is off"))
    );
}

#[tokio::test(start_paused = true)]
async fn no_next_episode_ends_quietly() {
    let fake = fake(true);
    let mut last = watched();
    last.next = None;
    let (_tx, rx) = watch::channel(false);
    assert_eq!(
        drive(&fake, 1, last, rx).await,
        Outcome::Done {
            chained: 0,
            autoplay: true
        }
    );
    assert!(fake.started.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_stop_on_the_tv_never_plays_the_next_one() {
    let fake = Fake {
        autoplay: true,
        ..Fake::default()
    };
    fake.scripts.lock().unwrap().push_back(vec![
        Ok(playing(1190.0)),
        Ok(status("IDLE", Some("CANCELLED"), 0.0, 1200.0, MEDIA)),
    ]);
    let (_tx, rx) = watch::channel(false);
    assert_eq!(
        drive(&fake, 1, watched(), rx).await,
        Outcome::Ended {
            ending: Ending::Stopped,
            chained: 0
        }
    );
    assert!(fake.started.lock().unwrap().is_empty());

    let error = Fake {
        autoplay: true,
        ..Fake::default()
    };
    error.scripts.lock().unwrap().push_back(vec![
        Ok(playing(1190.0)),
        Ok(status("IDLE", Some("ERROR"), 0.0, 1200.0, MEDIA)),
    ]);
    let (_tx, rx) = watch::channel(false);
    assert_eq!(
        drive(&error, 1, watched(), rx).await,
        Outcome::Ended {
            ending: Ending::Failed,
            chained: 0
        }
    );
    assert!(error.started.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_stop_from_portal_cancels_the_watch() {
    let fake = Fake {
        autoplay: true,
        ..Fake::default()
    };
    // Plays on and on; only Portal's Stop ends it.
    fake.scripts
        .lock()
        .unwrap()
        .push_back(vec![Ok(playing(10.0))]);
    let (tx, rx) = watch::channel(false);
    let run = drive(&fake, 1, watched(), rx);
    let stop = async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        tx.send_replace(true);
    };
    let (outcome, ()) = tokio::join!(run, stop);
    assert_eq!(outcome, Outcome::Cancelled);
    assert!(fake.started.lock().unwrap().is_empty());
    assert!(!fake
        .updates
        .lock()
        .unwrap()
        .iter()
        .any(|update| matches!(update, Update::Ended { .. })));
}

#[tokio::test(start_paused = true)]
async fn a_stop_while_the_next_episode_resolves_stops_its_job() {
    let fake = fake(true);
    fake.steps
        .lock()
        .unwrap()
        .insert("job-3".into(), VecDeque::from([JobStep::Working]));
    let (tx, rx) = watch::channel(false);
    let run = drive(&fake, 1, watched(), rx);
    let stop = async {
        // Long enough for the finish and a few job polls.
        tokio::time::sleep(Duration::from_secs(10)).await;
        tx.send_replace(true);
    };
    let (outcome, ()) = tokio::join!(run, stop);
    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(fake.started.lock().unwrap().len(), 1);
    assert_eq!(*fake.stopped.lock().unwrap(), vec!["job-3".to_string()]);
    assert!(fake.casts.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_question_times_out_and_says_why() {
    let fake = fake(true);
    fake.steps
        .lock()
        .unwrap()
        .insert("job-3".into(), VecDeque::from([JobStep::Prompt]));
    let (_tx, rx) = watch::channel(false);
    let outcome = drive(&fake, 1, watched(), rx).await;
    assert_eq!(
        outcome,
        Outcome::NextFailed {
            note: PROMPT_NOTE.into(),
            chained: 0
        }
    );
    assert_eq!(*fake.dismissed.lock().unwrap(), 1);
    assert_eq!(*fake.stopped.lock().unwrap(), vec!["job-3".to_string()]);
    assert!(fake.casts.lock().unwrap().is_empty());
    assert!(
        matches!(fake.updates.lock().unwrap().last(), Some(Update::Ended { state: DeviceState::Failed, note }) if note == PROMPT_NOTE)
    );
}

#[tokio::test(start_paused = true)]
async fn a_failed_or_uncastable_next_episode_is_reported() {
    let failed = fake(true);
    failed.steps.lock().unwrap().insert(
        "job-3".into(),
        VecDeque::from([JobStep::Failed("No sources found.".into())]),
    );
    let (_tx, rx) = watch::channel(false);
    assert_eq!(
        drive(&failed, 1, watched(), rx).await,
        Outcome::NextFailed {
            note: "No sources found.".into(),
            chained: 0
        }
    );

    let browse = fake(true);
    browse
        .steps
        .lock()
        .unwrap()
        .insert("job-3".into(), VecDeque::from([JobStep::Browse]));
    let (_tx, rx) = watch::channel(false);
    assert!(matches!(
        drive(&browse, 1, watched(), rx).await,
        Outcome::NextFailed { .. }
    ));
    assert_eq!(*browse.stopped.lock().unwrap(), vec!["job-3".to_string()]);

    let mut uncastable = fake(true);
    uncastable.cast_fails = true;
    uncastable.steps.lock().unwrap().insert(
        "job-3".into(),
        VecDeque::from([JobStep::Ready(Box::new(ready_job("job-3", None)))]),
    );
    let (_tx, rx) = watch::channel(false);
    match drive(&uncastable, 1, watched(), rx).await {
        Outcome::NextFailed { note, .. } => {
            assert!(note.contains("could not be cast"), "{note}")
        }
        other => panic!("expected a failed next episode, got {other:?}"),
    }
    assert_eq!(
        *uncastable.stopped.lock().unwrap(),
        vec!["job-3".to_string()]
    );

    let slow = fake(true);
    slow.steps
        .lock()
        .unwrap()
        .insert("job-3".into(), VecDeque::from([JobStep::Working]));
    let (_tx, rx) = watch::channel(false);
    match drive(&slow, 1, watched(), rx).await {
        Outcome::NextFailed { note, .. } => assert!(note.contains("too long"), "{note}"),
        other => panic!("expected a timeout, got {other:?}"),
    }
}

#[test]
fn progress_reports_name_the_show() {
    let item = replay_from_next(&next_item(2));
    let report = progress_report(&item, 1300.0, 1200.0).unwrap();
    assert_eq!(report.kind, TitleKind::Tv);
    assert_eq!(report.id, "show-1");
    assert_eq!(report.position, 1200.0);
    assert_eq!(report.episode_name.as_deref(), Some("Episode 2"));
    let mut movie = item.clone();
    movie.kind = "movie".into();
    movie.id = "movie-1".into();
    movie.title = "Example Movie".into();
    let report = progress_report(&movie, 10.0, 100.0).unwrap();
    assert_eq!(
        (report.kind, report.id.as_str(), report.name.as_str()),
        (TitleKind::Movie, "movie-1", "Example Movie")
    );
    let mut nameless = item;
    nameless.show_title = Some("{\"poster\":\"x\"}".into());
    assert!(progress_report(&nameless, 10.0, 100.0).is_none());
}

#[test]
fn stalls_count_only_after_the_item_has_played() {
    let mut tracker = Tracker::new(MEDIA);
    let buffering = |at| status("BUFFERING", None, at, 1200.0, MEDIA);
    // Loading is not a stall.
    assert_eq!(tracker.observe(&buffering(0.0)), None);
    assert_eq!(tracker.observe(&playing(4.0)), None);
    assert_eq!(tracker.stalls, 0);
    assert!(tracker.stalled.is_none());
    assert_eq!(tracker.observe(&buffering(300.0)), None);
    assert_eq!(tracker.observe(&buffering(300.0)), None);
    assert_eq!(tracker.stalled.map(|(_, at)| at), Some(300.0));
    assert_eq!(tracker.observe(&playing(302.0)), None);
    assert_eq!(tracker.stalls, 1);
    assert!(tracker.stalled.is_none());
    // A pause is not a stall either.
    assert_eq!(
        tracker.observe(&status("PAUSED", None, 310.0, 1200.0, MEDIA)),
        None
    );
    assert_eq!(tracker.stalls, 1);
}
