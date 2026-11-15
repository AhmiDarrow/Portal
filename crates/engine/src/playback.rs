//! Resolve an add-on item through Kodi, then publish browser HLS.
//!
//! The ffmpeg command is never logged, and logins are cut out of the error
//! text it leaves. Tests of the plan and the playlist names do not launch
//! ffmpeg.

use crate::catalog::plain_label;
use crate::error::{public_error, EngineError};
use crate::rpc::KodiClient;
use portal_api::{AudioTrack, JobState, NextItem, PlayJob, PlayRequest, Quality, SubtitleTrack};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x08000000;
const BELOW_NORMAL: u32 = 0x00004000;
const VIRTUAL_SCHEMES: &[&str] = &[
    "plugin", "special", "smb", "nfs", "ftp", "ftps", "sftp", "dav", "davs", "zip", "rar", "apk",
    "bluray", "dvd", "udf", "iso", "library", "addons", "upnp", "rss", "rsss", "pvr",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoPlan {
    pub copy: bool,
    pub scale: Option<i64>,
    /// The most bits a second a re-encoded picture may peak at; None for no limit.
    pub maxrate: Option<u64>,
}

struct JobSlot {
    job: PlayJob,
    /// The open handle keeps Windows from reusing the pid before Portal
    /// kills it, even after ffmpeg has finished on its own.
    ffmpeg: Option<Arc<Mutex<Child>>>,
    session: Option<String>,
    dir: Option<PathBuf>,
    /// The running ffmpeg's last stderr, for a failure after the job is Ready.
    stderr: Option<Arc<Mutex<String>>>,
    source: Option<MediaSource>,
    cut: u64,
    /// Embedded subtitles read once per job, before any shift, so a skip
    /// only rewrites them for the new start.
    subtitle_text: Option<Arc<Vec<SubtitleText>>>,
    extracting: bool,
    /// Last time the page asked for the job or one of its files.
    last_touch: Instant,
}

/// One embedded subtitle stream as WebVTT on the file's own timeline.
struct SubtitleText {
    id: u32,
    label: String,
    lang: Option<String>,
    forced: bool,
    text: String,
}

/// Resolved file kept for a later skip. It never leaves the engine.
#[derive(Clone)]
struct MediaSource {
    input: String,
    headers: Vec<(String, String)>,
    video: Option<StreamInfo>,
    /// The track mapped into the HLS stream.
    audio: Option<StreamInfo>,
    /// Every audio stream the file has, so another one can be chosen later.
    audios: Vec<StreamInfo>,
    subs: Vec<StreamInfo>,
    duration: Option<f64>,
    quality: Quality,
    title: String,
}

struct PlayerInner {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    root: PathBuf,
    jobs: Mutex<HashMap<String, JobSlot>>,
    sessions: Mutex<HashMap<String, String>>,
    zscale: Mutex<Option<bool>>,
    /// Only one job drives Kodi's player at a time.
    resolve: tokio::sync::Mutex<()>,
    sweeping: AtomicBool,
}

const SWEEP_EVERY: Duration = Duration::from_secs(60);
const IDLE_LIMIT: Duration = Duration::from_secs(10 * 60);
const READY_IDLE_LIMIT: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Clone)]
pub struct Player {
    inner: Arc<PlayerInner>,
}

impl Player {
    pub fn new(
        ffmpeg: impl Into<PathBuf>,
        ffprobe: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(PlayerInner {
                ffmpeg: ffmpeg.into(),
                ffprobe: ffprobe.into(),
                root: root.into(),
                jobs: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                zscale: Mutex::new(None),
                resolve: tokio::sync::Mutex::new(()),
                sweeping: AtomicBool::new(false),
            }),
        }
    }

    /// Returns a job id before Kodi or ffmpeg do any work.
    pub fn start(
        &self,
        request: PlayRequest,
        client: KodiClient,
        next: Option<NextItem>,
    ) -> Result<String, EngineError> {
        if request.file.trim().is_empty() {
            return Err(EngineError::BadRequest("Missing a file.".into()));
        }
        if let Some(detail) = crate::catalog::play_refusal(&request.file, &request.title) {
            return Err(EngineError::BadRequest(detail.into()));
        }
        let job_id = fresh_id();
        let job = PlayJob {
            id: job_id.clone(),
            state: JobState::Working,
            error: None,
            session: None,
            playlist: None,
            subtitles: Vec::new(),
            subtitles_pending: false,
            next,
            browse_path: None,
            title: Some(request.title.clone()),
            position: None,
            duration: None,
            release: None,
            audio: Vec::new(),
            audio_index: None,
            prompt: None,
            prompt_known: false,
        };
        self.lock_jobs().insert(
            job_id.clone(),
            JobSlot {
                job,
                ffmpeg: None,
                session: None,
                dir: None,
                stderr: None,
                source: None,
                cut: 0,
                subtitle_text: None,
                extracting: false,
                last_touch: Instant::now(),
            },
        );
        self.start_sweeper();
        let player = self.clone();
        let id = job_id.clone();
        tokio::spawn(async move {
            let mut played = player
                .play(id.clone(), request.clone(), client.clone())
                .await;
            // A timeout is usually a slow moment at the add-on, the file host
            // or the network: asked once more, the same item tends to play
            // within seconds. The page just waits a little longer.
            if let Err(err) = &played {
                if worth_retry(err) && !player.gone(&id) {
                    tracing::warn!(job = %id, error = %err, "Timed out; trying the play once more");
                    played = player.play(id.clone(), request, client).await;
                }
            }
            if let Err(err) = played {
                player.fail(&id, &err);
            }
        });
        Ok(job_id)
    }

    /// Gives a job that started without one the next episode it leads to,
    /// found once the job was already running. Returns whether it took.
    pub fn set_next(&self, id: &str, next: NextItem) -> bool {
        let mut jobs = self.lock_jobs();
        match jobs.get_mut(id) {
            Some(slot) if slot.job.next.is_none() => {
                slot.job.next = Some(next);
                true
            }
            _ => false,
        }
    }

    pub fn job(&self, id: &str) -> Option<PlayJob> {
        {
            let mut jobs = self.lock_jobs();
            if let Some(slot) = jobs.get_mut(id) {
                touch_unless_ready(slot);
                note_ffmpeg_exit(slot);
                return Some(slot.job.clone());
            }
        }
        let job_id = self.lock_sessions().get(id).cloned()?;
        self.lock_jobs().get_mut(&job_id).map(|slot| {
            touch_unless_ready(slot);
            note_ffmpeg_exit(slot);
            slot.job.clone()
        })
    }

    /// The quality the job was asked for, once its stream has been read.
    pub fn quality(&self, id: &str) -> Option<Quality> {
        let job_id = {
            let jobs = self.lock_jobs();
            if let Some(slot) = jobs.get(id) {
                return slot.source.as_ref().map(|source| source.quality);
            }
            drop(jobs);
            self.lock_sessions().get(id).cloned()?
        };
        self.lock_jobs()
            .get(&job_id)
            .and_then(|slot| slot.source.as_ref().map(|source| source.quality))
    }

    /// True while a play job is still inside Kodi. A finished job keeps playing
    /// from Portal's own stream and does not block a later engine restart.
    pub fn resolving(&self) -> bool {
        self.lock_jobs()
            .values()
            .any(|slot| blocks_engine_restart(slot.job.state))
    }

    /// Stops anything Kodi is playing while no play job is resolving. Portal
    /// only has Kodi play for the seconds it takes to learn a stream's
    /// address; an add-on that keeps searching after a job gave up can still
    /// start playback later, heard but not seen. Returns whether it stopped one.
    pub async fn stop_stray_playback(&self, client: &KodiClient) -> bool {
        let Ok(_gate) = self.inner.resolve.try_lock() else {
            return false;
        };
        let playing = active_players(client)
            .await
            .is_ok_and(|players| !players.is_empty());
        if playing {
            stop_players(client).await;
        }
        playing
    }

    pub async fn stop(&self, id: &str) -> Result<(), EngineError> {
        let (ffmpeg, dir) = {
            let mut jobs = self.lock_jobs();
            let mut sessions = self.lock_sessions();
            let job_id = if jobs.contains_key(id) {
                id.to_string()
            } else if let Some(found) = sessions.get(id) {
                found.clone()
            } else {
                return Err(EngineError::NotFound);
            };
            let Some(slot) = jobs.remove(&job_id) else {
                return Err(EngineError::NotFound);
            };
            if let Some(session) = &slot.session {
                sessions.remove(session);
            }
            (slot.ffmpeg, slot.dir)
        };
        if let Some(ffmpeg) = ffmpeg {
            let _ = tokio::task::spawn_blocking(move || end_process(&ffmpeg)).await;
        }
        if let Some(dir) = dir {
            let _ = tokio::task::spawn_blocking(move || remove_dir(&dir)).await;
        }
        Ok(())
    }

    pub fn session_file(&self, session: &str, name: &str) -> Result<PathBuf, EngineError> {
        let path = resolve_session_file(&self.inner.root, session, name)?;
        // A paused player keeps reloading the playlist. A segment or a subtitle
        // is someone still watching, so only those refresh the idle clock.
        if name != "index.m3u8" {
            let job_id = self.lock_sessions().get(session).cloned();
            if let Some(job_id) = job_id {
                if let Some(slot) = self.lock_jobs().get_mut(&job_id) {
                    slot.last_touch = Instant::now();
                }
            }
        }
        Ok(path)
    }

    /// Someone is still watching this job, by a way other than fetching its files.
    pub fn keep_alive(&self, job_id: &str) {
        if let Some(slot) = self.lock_jobs().get_mut(job_id) {
            slot.last_touch = Instant::now();
        }
    }

    /// True while this session's ffmpeg is still writing. A segment named in
    /// the playlist can be requested a moment before the file is renamed in.
    pub fn session_producing(&self, session: &str) -> bool {
        let job_id = self.lock_sessions().get(session).cloned();
        let Some(job_id) = job_id else {
            return false;
        };
        let jobs = self.lock_jobs();
        let Some(slot) = jobs.get(&job_id) else {
            return false;
        };
        slot.ffmpeg
            .as_ref()
            .is_some_and(|child| !child_exited(child))
    }

    /// Starts the idle sweep once, from the first play. The sweep ends with
    /// the last `Player` handle.
    fn start_sweeper(&self) {
        if self.inner.sweeping.swap(true, Ordering::SeqCst) {
            return;
        }
        let inner = Arc::downgrade(&self.inner);
        tokio::spawn(sweep_loop(inner));
    }

    /// Stops every job nobody has asked about for too long, through `stop`,
    /// so ffmpeg is killed and the session folder removed.
    async fn sweep(&self, now: Instant) {
        let idle: Vec<String> = self
            .lock_jobs()
            .iter()
            .filter(|(_, slot)| {
                idle_expired(
                    slot.job.state,
                    now.saturating_duration_since(slot.last_touch),
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in idle {
            let _ = self.stop(&id).await;
        }
    }

    async fn play(
        &self,
        job_id: String,
        request: PlayRequest,
        client: KodiClient,
    ) -> Result<(), EngineError> {
        if self.gone(&job_id) {
            return Ok(());
        }
        let begun = Instant::now();
        let mut media = request.file.clone();
        let mut title = request.title.clone();
        let via_addon = needs_kodi(&media);
        if via_addon {
            let resolved = {
                // A second job would stop this one's player and restore the
                // wrong mute state.
                let _gate = self.inner.resolve.lock().await;
                if self.gone(&job_id) {
                    return Ok(());
                }
                if request.manual_links {
                    // Some add-ons list sources as a folder with
                    // Container.Update, which does nothing on Kodi's home
                    // screen. On the Videos screen the folder opens, and the
                    // job hands it to the page to browse. Start from the
                    // top: still showing the last source folder, the same
                    // folder opening again would not count as opened.
                    // A leftover resolve dialog refuses this activate.
                    dismiss_leftover_modals(&client).await;
                    let _ = rpc(
                        &client,
                        "GUI.ActivateWindow",
                        json!({"window": "videos", "parameters": ["sources://video/"]}),
                        Duration::from_secs(5),
                    )
                    .await;
                }
                drive_player(&client, &media, &|| self.gone(&job_id)).await?
            };
            tracing::info!(
                job = %job_id,
                seconds = begun.elapsed().as_secs(),
                "The add-on answered"
            );
            match resolved {
                Resolved::Browse(path) => {
                    self.publish_browse(&job_id, &path, &title);
                    return Ok(());
                }
                Resolved::File {
                    file,
                    title: resolved,
                } => {
                    media = file;
                    if title.trim().is_empty() {
                        if let Some(resolved) = resolved {
                            title = resolved;
                        }
                    }
                }
            }
        }
        if self.gone(&job_id) {
            return Ok(());
        }
        if let Some((season, episode)) = requested_episode(&request) {
            media = self.correct_episode(&job_id, media, via_addon, season, episode)?;
        }
        if self.gone(&job_id) {
            return Ok(());
        }
        let (input, headers) = to_input(&media)?;
        assert_safe_input(&input)?;
        let seek = seek_seconds(request.resume_seconds);
        let ffprobe = self.inner.ffprobe.clone();
        let probe_input = input.clone();
        let probe_headers = headers.clone();
        let probed =
            tokio::task::spawn_blocking(move || probe_file(&ffprobe, &probe_input, &probe_headers))
                .await
                .map_err(|_| EngineError::Message("Could not read the stream".into()))??;
        let video = probed.video.first().cloned().map(|mut video| {
            video.bitrate = video_bitrate(&probed, &video);
            video
        });
        if let Some(video) = &video {
            tracing::info!(
                job = %job_id,
                title = %title,
                codec = %video.codec,
                height = video.height,
                kbps = video.bitrate.map(|rate| rate / 1000),
                seconds = begun.elapsed().as_secs(),
                "Read the stream"
            );
        }
        let audios = probed.audio;
        let audio = pick_audio(&audios, request.audio_language.as_deref()).cloned();
        if let Some(track) = &audio {
            tracing::info!(
                job = %job_id,
                index = track.index,
                lang = track.lang.as_deref().unwrap_or(""),
                title = track.title.as_deref().unwrap_or(""),
                "Chose an audio track"
            );
        }
        if video.is_none() && audio.is_none() {
            return Err(EngineError::Message(
                "No video stream found in this item.".into(),
            ));
        }
        let duration = probed
            .duration
            .filter(|value| value.is_finite() && *value >= 0.0);
        let seek = before_end(seek, duration);
        let source = MediaSource {
            input,
            headers,
            video,
            audio,
            audios,
            subs: probed.subs,
            duration,
            quality: request.quality,
            title,
        };
        let Some(cut) = self.store_source(&job_id, source) else {
            return Ok(());
        };
        self.open_at(&job_id, cut, seek).await
    }

    /// When the opened file names a different episode of the same season, it
    /// is refused instead of transcoding the wrong file. A file numbered in
    /// another season still plays, since sources number some shows
    /// differently.
    fn correct_episode(
        &self,
        job_id: &str,
        media: String,
        via_addon: bool,
        season: u32,
        episode: u32,
    ) -> Result<String, EngineError> {
        if !crate::episode_name::episode_conflict(&media, season, episode) {
            return Ok(media);
        }
        let Some((found_season, found_episode)) = crate::episode_name::named_episode(&media) else {
            return Ok(media);
        };
        let asked = episode_code(season, episode);
        let opened = episode_code(found_season, found_episode);
        if via_addon {
            tracing::info!(job = %job_id, "The add-on opened {opened} instead of {asked}");
        } else {
            tracing::info!(job = %job_id, "the file is {opened} instead of {asked}");
        }
        if self.gone(job_id) {
            return Ok(media);
        }
        if !refuses_episode(season, found_season) {
            tracing::info!(job = %job_id, "{opened} is numbered in another season, so it plays");
            return Ok(media);
        }
        let detail = if via_addon {
            format!("The add-on opened {opened} instead of {asked}.")
        } else {
            format!("The file is {opened} instead of {asked}.")
        };
        Err(EngineError::Message(detail))
    }

    /// Move an already-playing video to `seconds` on the full timeline.
    /// The add-on is not asked again. Values of a minute or less still count.
    pub async fn seek(&self, id: &str, seconds: f64) -> Result<PlayJob, EngineError> {
        self.restart(id, seconds, None, None).await
    }

    /// Restart an already-playing video at `seconds` in another quality. The
    /// stream that was read for the job is reused, so the add-on is not asked
    /// again, and later skips keep the new quality.
    pub async fn set_quality(
        &self,
        id: &str,
        quality: Quality,
        seconds: f64,
    ) -> Result<PlayJob, EngineError> {
        self.restart(id, seconds, Some(quality), None).await
    }

    /// Restart an already-playing video at `seconds` on another audio track.
    /// The same index is left playing. The add-on is not asked again.
    pub async fn set_audio(
        &self,
        id: &str,
        index: u32,
        seconds: f64,
    ) -> Result<PlayJob, EngineError> {
        self.restart(id, seconds, None, Some(index)).await
    }

    async fn restart(
        &self,
        id: &str,
        seconds: f64,
        quality: Option<Quality>,
        audio: Option<u32>,
    ) -> Result<PlayJob, EngineError> {
        let (job_id, at, cut, ffmpeg, dir) = {
            let mut jobs = self.lock_jobs();
            let mut sessions = self.lock_sessions();
            let job_id = if jobs.contains_key(id) {
                id.to_string()
            } else if let Some(found) = sessions.get(id) {
                found.clone()
            } else {
                return Err(EngineError::NotFound);
            };
            let Some(slot) = jobs.get_mut(&job_id) else {
                return Err(EngineError::NotFound);
            };
            if !can_seek(slot) {
                return Err(EngineError::BadRequest(
                    "The time bar is available once the video is playing.".into(),
                ));
            }
            if let Some(index) = audio {
                let wanted = i64::from(index);
                let already = slot
                    .source
                    .as_ref()
                    .and_then(|source| source.audio.as_ref())
                    .and_then(|stream| stream.index)
                    == Some(wanted);
                if already {
                    return Ok(slot.job.clone());
                }
            }
            let at = skip_to(
                seconds,
                slot.source.as_ref().and_then(|source| source.duration),
            )?;
            if let (Some(quality), Some(source)) = (quality, slot.source.as_mut()) {
                source.quality = quality;
            }
            if let Some(index) = audio {
                let Some(source) = slot.source.as_mut() else {
                    return Err(EngineError::BadRequest(
                        "The time bar is available once the video is playing.".into(),
                    ));
                };
                let wanted = i64::from(index);
                let Some(chosen) = source
                    .audios
                    .iter()
                    .find(|stream| stream.index == Some(wanted))
                    .cloned()
                else {
                    return Err(EngineError::BadRequest(
                        "That audio track is not in this file.".into(),
                    ));
                };
                source.audio = Some(chosen);
            }
            slot.cut = slot.cut.saturating_add(1);
            let cut = slot.cut;
            let ffmpeg = slot.ffmpeg.take();
            let dir = slot.dir.take();
            if let Some(session) = slot.session.take() {
                sessions.remove(&session);
            }
            slot.job.session = None;
            slot.job.playlist = None;
            slot.job.state = JobState::Working;
            slot.job.error = None;
            (job_id, at, cut, ffmpeg, dir)
        };
        // The job is Working now. A dropped request must not leave it there.
        let player = self.clone();
        let id = job_id.clone();
        let reopen = tokio::spawn(async move {
            if let Some(ffmpeg) = ffmpeg {
                let _ = tokio::task::spawn_blocking(move || end_process(&ffmpeg)).await;
            }
            if let Some(dir) = dir {
                retire_dir(dir, RETIRE_GRACE);
            }
            let mut opened = player.open_at(&id, cut, at).await;
            // The same file, started once more at the same place.
            if let Err(err) = &opened {
                if worth_retry(err) && !player.stale(&id, cut) {
                    tracing::warn!(job = %id, error = %err, "Timed out; starting the stream once more");
                    opened = player.open_at(&id, cut, at).await;
                }
            }
            if let Err(err) = &opened {
                player.fail(&id, err);
            }
            opened
        });
        match reopen.await {
            Ok(opened) => opened?,
            Err(_) => {
                let err = EngineError::Message("Could not move the video.".into());
                self.fail(&job_id, &err);
                return Err(err);
            }
        }
        self.job(&job_id).ok_or(EngineError::NotFound)
    }

    fn store_source(&self, job_id: &str, source: MediaSource) -> Option<u64> {
        let mut jobs = self.lock_jobs();
        let slot = jobs.get_mut(job_id)?;
        slot.cut = slot.cut.saturating_add(1);
        slot.source = Some(source);
        Some(slot.cut)
    }

    fn stale(&self, job_id: &str, cut: u64) -> bool {
        match self.lock_jobs().get(job_id) {
            Some(slot) => slot.cut != cut,
            None => true,
        }
    }

    async fn open_at(&self, job_id: &str, cut: u64, at: u64) -> Result<(), EngineError> {
        let source = {
            let jobs = self.lock_jobs();
            let Some(slot) = jobs.get(job_id) else {
                return Ok(());
            };
            if slot.cut != cut {
                return Ok(());
            }
            let Some(source) = slot.source.clone() else {
                return Err(EngineError::Message(
                    "The time bar is available once the video is playing.".into(),
                ));
            };
            source
        };
        let session = fresh_id();
        let dir = self.inner.root.join(&session);
        fs::create_dir_all(&dir)
            .map_err(|_| EngineError::Message("Could not open a playback folder.".into()))?;
        if !self.attach_session(job_id, &session, dir.clone(), cut) {
            remove_dir(&dir);
            return Ok(());
        }
        let plan = source
            .video
            .as_ref()
            .map(|stream| stream_plan(stream, source.quality));
        let hdr = source
            .video
            .as_ref()
            .is_some_and(|stream| is_hdr(&stream.transfer));
        let zscale = if plan.is_some_and(|plan| !plan.copy) && hdr {
            self.zscale_available().await
        } else {
            false
        };
        let args = ffmpeg_args(&source, at, &dir, plan.as_ref(), zscale)?;
        let begun = Instant::now();
        tracing::info!(
            job = %job_id,
            at,
            copy = plan.as_ref().is_some_and(|plan| plan.copy),
            max_kbps = plan.as_ref().and_then(|plan| plan.maxrate).map(|rate| rate / 1000),
            "Starting the stream"
        );
        let ffmpeg = self.inner.ffmpeg.clone();
        let mut child = spawn_ffmpeg(&ffmpeg, &args)
            .map_err(|_| EngineError::Message("Transcoding failed to start".into()))?;
        let stderr = drain_stderr(&mut child);
        let child = Arc::new(Mutex::new(child));
        if self.stale(job_id, cut) || !self.note_ffmpeg(job_id, child.clone(), stderr.clone(), cut)
        {
            discard(child, dir).await;
            return Ok(());
        }
        let wait =
            wait_for_segment(&child, &dir.join("index.m3u8"), &|| self.stale(job_id, cut)).await;
        if let Err(err) = wait {
            discard(child, dir).await;
            if self.stale(job_id, cut) {
                return Ok(());
            }
            tracing::warn!(
                job = %job_id,
                seconds = begun.elapsed().as_secs(),
                error = %err,
                "The stream's first part did not arrive"
            );
            let detail = failure_line(&stderr.lock().unwrap_or_else(|poison| poison.into_inner()));
            if err.to_string() == "Stopped" {
                return Ok(());
            }
            if detail.is_empty() || err.to_string() == SLOW_START {
                return Err(err);
            }
            return Err(EngineError::Message(clip(
                &format!("Transcoding failed: {detail}"),
                500,
            )));
        }
        if self.stale(job_id, cut) {
            discard(child, dir).await;
            return Ok(());
        }
        tracing::info!(
            job = %job_id,
            seconds = begun.elapsed().as_secs(),
            "The stream's first part is ready"
        );
        let mut position = at as f64;
        if copied_from_keyframe(at, plan.as_ref()) {
            let ffprobe = self.inner.ffprobe.clone();
            let segment = dir.join("seg00000.ts");
            let start = tokio::task::spawn_blocking(move || copied_start(&ffprobe, &segment, at))
                .await
                .ok()
                .flatten();
            if let Some(start) = start {
                position = start;
            }
        }
        let wants_subtitles = subtitle_streams(&source.subs).next().is_some();
        let release = release_name(&source.input);
        let published = self.publish_ready(
            job_id,
            ReadyUpdate {
                session,
                title: source.title.clone(),
                position,
                duration: source.duration,
                subtitles_pending: wants_subtitles,
                release,
            },
            cut,
        );
        if !published {
            discard(child, dir).await;
            return Ok(());
        }
        if wants_subtitles {
            if self.claim_extraction(job_id) {
                tokio::spawn(
                    self.clone()
                        .extract_in_background(job_id.to_string(), source),
                );
            }
            self.fill_subtitles(job_id).await;
        }
        Ok(())
    }

    /// Only one reader of embedded subtitles runs per job. False when the
    /// text is already kept or another reader is on it.
    fn claim_extraction(&self, job_id: &str) -> bool {
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return false;
        };
        if slot.subtitle_text.is_some() || slot.extracting {
            return false;
        }
        slot.extracting = true;
        true
    }

    /// Reads every embedded text subtitle once, keeps it on the job, and
    /// hands it to whichever session is playing by then. A stopped job is
    /// never written to, and job ids are never reused.
    async fn extract_in_background(self, job_id: String, source: MediaSource) {
        let player = self.clone();
        let id = job_id.clone();
        let ffmpeg = self.inner.ffmpeg.clone();
        let scratch = self.inner.root.join(format!("extract.{}", fresh_id()));
        let texts = tokio::task::spawn_blocking(move || {
            extract_subtitles(
                &ffmpeg,
                &source.input,
                &source.headers,
                &source.subs,
                &scratch,
                &|| player.gone(&id),
            )
        })
        .await
        .unwrap_or_default();
        {
            let mut jobs = self.lock_jobs();
            let Some(slot) = jobs.get_mut(&job_id) else {
                return;
            };
            slot.subtitle_text = Some(Arc::new(texts));
            slot.extracting = false;
        }
        self.fill_subtitles(&job_id).await;
    }

    /// Writes the kept subtitle text, shifted to the session's start, into
    /// the playing session and lists it on the job. Does nothing while the
    /// text is still being read. Two callers write the same files, so a
    /// repeat is harmless.
    async fn fill_subtitles(&self, job_id: &str) {
        let (texts, session, dir, position, cut) = {
            let jobs = self.lock_jobs();
            let Some(slot) = jobs.get(job_id) else {
                return;
            };
            if slot.job.state != JobState::Ready || !slot.job.subtitles_pending {
                return;
            }
            let (Some(texts), Some(session), Some(dir)) =
                (&slot.subtitle_text, &slot.session, &slot.dir)
            else {
                return;
            };
            (
                texts.clone(),
                session.clone(),
                dir.clone(),
                slot.job.position.unwrap_or(0.0),
                slot.cut,
            )
        };
        let tracks =
            tokio::task::spawn_blocking(move || write_subtitles(&texts, &dir, &session, position))
                .await
                .unwrap_or_default();
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return;
        };
        if slot.cut != cut || slot.job.state != JobState::Ready {
            return;
        }
        slot.job.subtitles = tracks;
        slot.job.subtitles_pending = false;
    }

    fn gone(&self, job_id: &str) -> bool {
        !self.lock_jobs().contains_key(job_id)
    }

    fn attach_session(&self, job_id: &str, session: &str, dir: PathBuf, cut: u64) -> bool {
        let mut jobs = self.lock_jobs();
        let mut sessions = self.lock_sessions();
        let Some(slot) = jobs.get_mut(job_id) else {
            return false;
        };
        if slot.cut != cut {
            return false;
        }
        // A retried start replaces the session of the try that failed.
        if let Some(old) = slot.session.replace(session.to_string()) {
            if old != session {
                sessions.remove(&old);
            }
        }
        if let Some(old) = slot.dir.replace(dir) {
            if Some(&old) != slot.dir.as_ref() {
                retire_dir(old, RETIRE_GRACE);
            }
        }
        sessions.insert(session.to_string(), job_id.to_string());
        true
    }

    fn note_ffmpeg(
        &self,
        job_id: &str,
        child: Arc<Mutex<Child>>,
        stderr: Arc<Mutex<String>>,
        cut: u64,
    ) -> bool {
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return false;
        };
        if slot.cut != cut {
            return false;
        }
        slot.ffmpeg = Some(child);
        slot.stderr = Some(stderr);
        true
    }

    fn publish_browse(&self, job_id: &str, path: &str, title: &str) {
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return;
        };
        slot.job.state = JobState::Browse;
        slot.job.browse_path = Some(path.to_string());
        if !title.is_empty() {
            slot.job.title = Some(title.to_string());
        }
    }

    fn publish_ready(&self, job_id: &str, update: ReadyUpdate, cut: u64) -> bool {
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return false;
        };
        if slot.cut != cut {
            return false;
        }
        slot.job.state = JobState::Ready;
        slot.job.session = Some(update.session.clone());
        slot.job.playlist = Some(format!("/media/{}/index.m3u8", update.session));
        slot.job.subtitles = Vec::new();
        slot.job.subtitles_pending = update.subtitles_pending;
        slot.job.title = Some(update.title);
        slot.job.position = Some(update.position);
        slot.job.duration = update.duration;
        slot.job.release = update.release;
        slot.job.error = None;
        if let Some(source) = slot.source.as_ref() {
            let (audio, audio_index) = audio_listing(&source.audios, source.audio.as_ref());
            slot.job.audio = audio;
            slot.job.audio_index = audio_index;
        }
        true
    }

    fn fail(&self, job_id: &str, err: &EngineError) {
        let raw = failure_text(err);
        let mut jobs = self.lock_jobs();
        let Some(slot) = jobs.get_mut(job_id) else {
            return;
        };
        if slot.job.state == JobState::Ready {
            return;
        }
        let message = raw;
        tracing::warn!(job = %job_id, error = %message, "Play failed");
        slot.job.state = JobState::Failed;
        slot.job.error = Some(message);
    }

    async fn zscale_available(&self) -> bool {
        if let Some(cached) = *self
            .inner
            .zscale
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
        {
            return cached;
        }
        let ffmpeg = self.inner.ffmpeg.clone();
        let found = tokio::task::spawn_blocking(move || ffmpeg_has_zscale(&ffmpeg))
            .await
            .unwrap_or(false);
        *self
            .inner
            .zscale
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(found);
        found
    }

    fn lock_jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, JobSlot>> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn lock_sessions(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.inner
            .sessions
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

struct ReadyUpdate {
    session: String,
    title: String,
    position: f64,
    duration: Option<f64>,
    subtitles_pending: bool,
    release: Option<String>,
}

/// A Ready job whose ffmpeg exited with an error before the playlist was
/// finished lost its source mid-play, as on a network drop. The job fails with
/// ffmpeg's last line, redacted. A normal finish exits 0 and is left alone.
fn note_ffmpeg_exit(slot: &mut JobSlot) {
    if slot.job.state != JobState::Ready {
        return;
    }
    let Some(ffmpeg) = &slot.ffmpeg else {
        return;
    };
    let Ok(Some(status)) = lock_child(ffmpeg).try_wait() else {
        return;
    };
    // Every poll of a finished job lands here with the job lock held. A clean
    // exit needs no read of the whole playlist from disk.
    if status.success() {
        return;
    }
    let playlist = slot
        .dir
        .as_ref()
        .and_then(|dir| fs::read_to_string(dir.join("index.m3u8")).ok());
    if !stopped_early(status, playlist.as_deref()) {
        return;
    }
    let detail = slot.stderr.as_ref().map_or_else(String::new, |stderr| {
        failure_line(&stderr.lock().unwrap_or_else(|poison| poison.into_inner()))
    });
    tracing::warn!(job = %slot.job.id, error = %detail, "The stream stopped early");
    slot.job.state = JobState::Failed;
    slot.job.error = Some(stopped_message(&detail));
}

/// A failed exit before the playlist's end marker. A finished playlist means
/// ffmpeg wrote the whole video, whatever it said on the way out.
fn stopped_early(status: ExitStatus, playlist: Option<&str>) -> bool {
    !status.success() && !playlist.is_some_and(|text| text.contains("#EXT-X-ENDLIST"))
}

fn stopped_message(detail: &str) -> String {
    let message = if detail.is_empty() {
        "Transcoding stopped.".to_string()
    } else {
        format!("Transcoding stopped: {detail}")
    };
    clip(&public_error(&message), 500)
}

async fn sweep_loop(inner: Weak<PlayerInner>) {
    let mut tick = tokio::time::interval(SWEEP_EVERY);
    tick.tick().await;
    loop {
        tick.tick().await;
        let Some(inner) = inner.upgrade() else {
            return;
        };
        Player { inner }.sweep(Instant::now()).await;
    }
}

/// A playing video may sit paused for a while. Anything else left alone
/// for ten minutes was abandoned.
/// A playing video can move, and so can one whose last move failed, as long
/// as its probed source is still kept, so the time bar can try again.
fn can_seek(slot: &JobSlot) -> bool {
    matches!(slot.job.state, JobState::Ready | JobState::Failed) && slot.source.is_some()
}

/// A page asking about a job that is still being made keeps it alive. Once it
/// plays, only segment and subtitle fetches do, so a paused tab that keeps
/// polling still lets its ffmpeg go.
fn touch_unless_ready(slot: &mut JobSlot) {
    if slot.job.state != JobState::Ready {
        slot.last_touch = Instant::now();
    }
}

fn idle_expired(state: JobState, idle: Duration) -> bool {
    let limit = if state == JobState::Ready {
        READY_IDLE_LIMIT
    } else {
        IDLE_LIMIT
    };
    idle > limit
}

pub fn blocks_engine_restart(state: JobState) -> bool {
    matches!(
        state,
        JobState::Working | JobState::Prompt | JobState::Browse
    )
}

enum Resolved {
    File { file: String, title: Option<String> },
    Browse(String),
}

pub fn split_kodi_url(url: &str) -> (String, Vec<(String, String)>) {
    let Some(bar) = url.find('|') else {
        return (url.to_string(), Vec::new());
    };
    (url[..bar].to_string(), parse_pairs(&url[bar + 1..]))
}

pub fn input_args(input: &str, headers: &[(&str, &str)]) -> Vec<String> {
    let lower = input.to_ascii_lowercase();
    if !(lower.starts_with("http:") || lower.starts_with("https:")) {
        return Vec::new();
    }
    let mut args = Vec::new();
    let user_agent = headers.iter().find_map(|(key, value)| {
        if key.eq_ignore_ascii_case("user-agent") {
            let cleaned = clean_header(value);
            if cleaned.is_empty() {
                None
            } else {
                Some(cleaned)
            }
        } else {
            None
        }
    });
    if let Some(user_agent) = user_agent {
        args.push("-user_agent".into());
        args.push(user_agent);
    }
    let mut lines = String::new();
    for (key, value) in headers {
        if key.eq_ignore_ascii_case("user-agent") {
            continue;
        }
        let name = clean_header(key);
        if name.is_empty() {
            continue;
        }
        lines.push_str(&name);
        lines.push_str(": ");
        lines.push_str(&clean_header(value));
        lines.push_str("\r\n");
    }
    if !lines.is_empty() {
        args.push("-headers".into());
        args.push(lines);
    }
    // A host that stalls for half a minute should not kill a film that is
    // already playing. HLS inputs stay without reconnect: retrying a live
    // playlist from the start repeats segments.
    args.push("-rw_timeout".into());
    args.push("60000000".into());
    if !is_hls_playlist(input) {
        args.push("-reconnect".into());
        args.push("1".into());
        args.push("-reconnect_streamed".into());
        args.push("1".into());
        args.push("-reconnect_on_network_error".into());
        args.push("1".into());
        args.push("-reconnect_delay_max".into());
        args.push("5".into());
    }
    args
}

pub fn assert_safe_input(input: &str) -> Result<(), EngineError> {
    if input.starts_with('-') || input.contains(['\n', '\r']) {
        return Err(EngineError::BadRequest("Unsafe ffmpeg input".into()));
    }
    Ok(())
}

/// Copy browser-safe H.264, or transcode. HDR transfer is handled later, and only on the transcode path.
/// The picture is copied whatever the audio is; audio the browser cannot play is re-encoded on its own.
pub fn video_plan(codec: &str, pix_fmt: &str, height: i64, quality: Quality) -> VideoPlan {
    let tall = height.max(0);
    if quality == Quality::Source {
        if codec == "h264" && (pix_fmt == "yuv420p" || pix_fmt == "yuvj420p") {
            return VideoPlan {
                copy: true,
                scale: None,
                maxrate: None,
            };
        }
        return VideoPlan {
            copy: false,
            scale: None,
            maxrate: None,
        };
    }
    let cap = quality.cap_height().unwrap_or(1080);
    if tall > cap {
        return VideoPlan {
            copy: false,
            scale: Some(cap),
            maxrate: None,
        };
    }
    if codec == "h264" && (pix_fmt == "yuv420p" || pix_fmt == "yuvj420p") {
        return VideoPlan {
            copy: true,
            scale: None,
            maxrate: None,
        };
    }
    VideoPlan {
        copy: false,
        scale: None,
        maxrate: None,
    }
}

/// The plan for one probed stream. Browsers do not deinterlace, so an
/// interlaced picture is re-encoded through the deinterlacer unless the viewer
/// asked for the source as it is.
fn stream_plan(stream: &StreamInfo, quality: Quality) -> VideoPlan {
    let mut plan = video_plan(&stream.codec, &stream.pix_fmt, stream.height, quality);
    if quality == Quality::Source {
        return plan;
    }
    if stream.interlaced {
        plan.copy = false;
    }
    // A Blu-ray's picture can average 14 Mbit/s and peak past 30 in action
    // scenes, which stalls a Chromecast on Wi-Fi. Such a file is re-encoded
    // under a ceiling; a lighter one is still copied as it is.
    let height = plan.scale.unwrap_or(stream.height);
    let ceiling = max_bitrate(height);
    if plan.copy
        && stream
            .bitrate
            .is_some_and(|rate| rate > copy_limit(ceiling))
    {
        plan.copy = false;
    }
    if !plan.copy {
        plan.maxrate = Some(ceiling);
    }
    plan
}

/// The most bits a second a re-encoded picture of `height` lines may peak
/// at: plenty for its size, and steady enough for a TV on Wi-Fi.
fn max_bitrate(height: i64) -> u64 {
    match height {
        ..=480 => 2_500_000,
        481..=720 => 6_000_000,
        721..=1080 => 12_000_000,
        _ => 30_000_000,
    }
}

/// A copied picture averaging more than this peaks well past `ceiling`.
fn copy_limit(ceiling: u64) -> u64 {
    ceiling / 6 * 5
}

/// AAC and MP3 play from MPEG-TS in every browser. FLAC does not, so it is re-encoded.
fn copy_audio(codec: &str) -> bool {
    matches!(codec.to_ascii_lowercase().as_str(), "aac" | "mp3")
}

/// Add `offset` seconds to every cue. SRT commas become WebVTT dots. A negative start becomes 0.
pub fn shift_vtt(text: &str, offset: f64) -> String {
    let text = text
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut body = String::new();
    let mut saw_header = false;
    for line in text.split('\n') {
        let trimmed = line.trim();
        if !saw_header && trimmed.eq_ignore_ascii_case("WEBVTT") {
            saw_header = true;
            continue;
        }
        if trimmed.starts_with("WEBVTT") && trimmed.len() > 6 && !saw_header {
            saw_header = true;
            continue;
        }
        if let Some(rewritten) = rewrite_cue_line(line, offset) {
            body.push_str(&rewritten);
        } else {
            body.push_str(line);
        }
        body.push('\n');
    }
    let mut out = String::from("WEBVTT\n\n");
    out.push_str(body.trim_start_matches('\n'));
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "mov", "wmv", "webm", "ts", "m2ts", "mpg", "mpeg", "flv",
];

/// The file name, without its extension, at the end of a stream's address or
/// path, e.g. "Show.S01E01.720p". `None` unless it ends in a
/// video file name. It only helps rank online subtitles and never leaves
/// Portal.
pub fn release_name(input: &str) -> Option<String> {
    let path = if is_http(input) {
        let rest = input.split_once("://").map_or(input, |(_, rest)| rest);
        let rest = rest.split(['?', '#']).next().unwrap_or("");
        let (_, path) = rest.split_once('/')?;
        percent_decode(path)
    } else {
        input.to_string()
    };
    let name = path.rsplit(['/', '\\']).next()?.trim();
    let (stem, extension) = name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    if !VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return None;
    }
    let stem = stem.trim();
    let useful = stem.chars().filter(|ch| ch.is_alphanumeric()).count() >= 3;
    (useful && stem.chars().count() <= 240 && !stem.chars().any(char::is_control))
        .then(|| stem.to_string())
}

/// A point on the full timeline. Unlike resume, a short time is kept.
pub fn skip_to(seconds: f64, duration: Option<f64>) -> Result<u64, EngineError> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(EngineError::BadRequest("Choose a time on the bar.".into()));
    }
    if seconds > (u64::MAX as f64) {
        return Err(EngineError::BadRequest("Choose a time on the bar.".into()));
    }
    Ok(before_end(seconds.floor() as u64, duration))
}

/// Keeps a start point at least `END_MARGIN` seconds before the end once the
/// length is known, so a resume past the end of a shorter cut still plays.
pub fn before_end(at: u64, duration: Option<f64>) -> u64 {
    let Some(duration) = duration else {
        return at;
    };
    if !duration.is_finite() || duration <= 0.0 {
        return at;
    }
    let end = (duration - END_MARGIN).floor().max(0.0);
    if end < u64::MAX as f64 {
        at.min(end as u64)
    } else {
        at
    }
}

const END_MARGIN: f64 = 10.0;

pub fn seek_seconds(value: Option<f64>) -> u64 {
    let Some(value) = value else {
        return 0;
    };
    if !value.is_finite() || value <= 60.0 {
        return 0;
    }
    value.floor() as u64
}

pub fn resolve_session_file(
    root: &Path,
    session: &str,
    name: &str,
) -> Result<PathBuf, EngineError> {
    if !valid_session(session) || !valid_media_name(name) {
        return Err(EngineError::NotFound);
    }
    let path = root.join(session).join(name);
    if path.file_name().and_then(|value| value.to_str()) != Some(name) {
        return Err(EngineError::NotFound);
    }
    Ok(path)
}

fn valid_session(session: &str) -> bool {
    (1..=64).contains(&session.len())
        && session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_media_name(name: &str) -> bool {
    if name == "index.m3u8" || name == "cast.mp4" {
        return true;
    }
    numbered(name, "seg", ".ts") || numbered(name, "sub", ".vtt")
}

fn numbered(name: &str, prefix: &str, suffix: &str) -> bool {
    name.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .is_some_and(|digits| digits.len() == 5 && digits.bytes().all(|byte| byte.is_ascii_digit()))
}

fn requested_episode(request: &PlayRequest) -> Option<(u32, u32)> {
    let season = request.season.filter(|season| (1..=99).contains(season))?;
    let episode = request
        .episode
        .filter(|episode| (1..=999).contains(episode))?;
    Some((season, episode))
}

fn episode_code(season: u32, episode: u32) -> String {
    format!("S{season:02}E{episode:02}")
}

/// A wrong episode of the asked season is refused. Another season may be the
/// same episode under a different numbering.
fn refuses_episode(asked_season: u32, found_season: u32) -> bool {
    asked_season == found_season
}

fn needs_kodi(file: &str) -> bool {
    let base = file.split('|').next().unwrap_or(file).trim();
    let lower = base.to_ascii_lowercase();
    if lower.starts_with("plugin://") {
        return true;
    }
    let path = lower.split(['?', '#']).next().unwrap_or(&lower);
    if path.ends_with(".strm") {
        return true;
    }
    scheme_of(&lower).is_some_and(|scheme| VIRTUAL_SCHEMES.contains(&scheme))
}

fn scheme_of(url: &str) -> Option<&str> {
    let idx = url.find("://")?;
    let scheme = &url[..idx];
    if scheme.is_empty() || scheme.contains('/') {
        None
    } else {
        Some(scheme)
    }
}

fn to_input(file: &str) -> Result<(String, Vec<(String, String)>), EngineError> {
    let (url, headers) = split_kodi_url(&unwrap_stack(file));
    assert_safe_input(&url)?;
    // A network share would make Windows sign in to whatever host it names.
    if network_share(&url) {
        return Err(EngineError::Message(
            "This source needs a direct file or http URL.".into(),
        ));
    }
    if is_http(&url) {
        // A login in the address stays there: ffmpeg sends it only to that
        // host. As a header it would follow every redirect to other hosts.
        return Ok((url, headers));
    }
    if is_virtual(&url) {
        return Err(EngineError::Message(
            "This source needs a direct file or http URL.".into(),
        ));
    }
    if is_drive(&url) || Path::new(&url).is_file() {
        return Ok((url, headers));
    }
    Err(EngineError::Message(
        "This source needs a direct file or http URL.".into(),
    ))
}

/// `\\host\share\...` or `//host/share/...`, in any mix of slashes.
pub(crate) fn network_share(path: &str) -> bool {
    let mut chars = path.trim_start().chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('\\' | '/'), Some('\\' | '/'))
    )
}

fn unwrap_stack(file: &str) -> String {
    if !file.to_ascii_lowercase().starts_with("stack://") {
        return file.to_string();
    }
    let rest = &file[8..];
    let first = rest.split(" , ").next().unwrap_or(rest);
    first.replace(",,", ",")
}

fn is_http(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("https://") || lower.starts_with("http://")
}

fn is_virtual(url: &str) -> bool {
    scheme_of(&url.to_ascii_lowercase()).is_some_and(|scheme| VIRTUAL_SCHEMES.contains(&scheme))
}

fn is_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn parse_pairs(query: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if query.is_empty() {
        return pairs;
    }
    for part in query.split('&') {
        if part.is_empty() {
            continue;
        }
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        pairs.push((percent_decode(key), percent_decode(value)));
    }
    pairs
}

fn percent_decode(text: &str) -> String {
    let plus = text.replace('+', " ");
    let bytes = plus.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            ) {
                out.push(value);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn clean_header(value: &str) -> String {
    value.replace(['\r', '\n'], "")
}

fn is_hls_playlist(input: &str) -> bool {
    let lower = input.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = b".m3u8";
    let mut index = 0;
    while index + needle.len() <= bytes.len() {
        if &bytes[index..index + needle.len()] == needle {
            let after = index + needle.len();
            if after == bytes.len() || bytes[after] == b'?' {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn canon_plugin(url: &str) -> String {
    percent_decode(&url.trim().replace('+', " "))
        .trim_end_matches(['/', '\\'])
        .to_string()
}

const NO_PLAYABLE_STREAM: &str = "The add-on did not return a playable stream.";
const CANCELLED: &str = "The add-on's question was cancelled.";

/// The job's error text for a failure.
fn failure_text(err: &EngineError) -> String {
    clip(&public_error(&err.public_message()), 500)
}

/// Whether Portal picks the copy itself: every quality but Source has a
/// height to pick for.
pub fn picks_source(quality: Quality) -> bool {
    quality != Quality::Source
}

/// The tallest picture worth fetching for `quality`, for choosing a copy.
pub fn source_cap(quality: Quality) -> u32 {
    quality
        .cap_height()
        .and_then(|height| u32::try_from(height).ok())
        .unwrap_or(1080)
}

fn changed_plugin(requested: &str, previous: &str, folder: &str) -> bool {
    let folder = folder.trim();
    if !folder.to_ascii_lowercase().starts_with("plugin://") {
        return false;
    }
    let next = canon_plugin(folder);
    !next.is_empty() && next != canon_plugin(previous) && next != canon_plugin(requested)
}

async fn drive_player(
    client: &KodiClient,
    file: &str,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Resolved, EngineError> {
    let started = Instant::now();
    // Time a question spends waiting on the user does not count.
    let mut waited = Duration::ZERO;
    let mut cap = resolve_cap(started, waited);
    let mut prompt_open = false;
    let cancels = crate::prompts::CANCELS.load(Ordering::SeqCst);
    let previous = info_label(client, "Container.FolderPath")
        .await
        .unwrap_or_default();
    let muted = application_muted(client).await.unwrap_or(false);
    let result = async {
        rpc(
            client,
            "Application.SetMute",
            json!({"mute": true}),
            Duration::from_secs(8),
        )
        .await?;
        stop_players(client).await;
        // An add-on's resolve window can still be modal after the player stops,
        // and the next ActivateWindow or Player.Open is then refused.
        dismiss_leftover_modals(client).await;
        // Portal seeks the resolved file itself. Leaving resume unset makes
        // Kodi open its own resume dialog, and both Play and Resume then wait
        // on a question the page cannot show when the labels are empty.
        let mut blank_tries = 0u8;
        if dismiss_blank_yesno(client).await {
            blank_tries = blank_tries.saturating_add(1);
        }
        let open_limit = Duration::from_secs(120)
            .saturating_sub(started.elapsed())
            .max(Duration::from_secs(1));
        // Open can sit inside the add-on for the whole limit. A stopped job
        // stops waiting at once, so the next play is not held behind it; a
        // late reply to the dropped call is ignored by the client.
        let open = rpc(client, "Player.Open", player_open_params(file), open_limit);
        tokio::pin!(open);
        let open_error = loop {
            tokio::select! {
                result = &mut open => break result.err(),
                _ = tokio::time::sleep(RESOLVE_TICK) => {
                    if cancel() {
                        return Err(EngineError::Message("Stopped".into()));
                    }
                }
            }
        };
        // Open can spend the whole poll window inside the add-on. The look
        // for a stream starts when it returns, still inside the resolve cap.
        let mut poll_deadline = poll_window(Instant::now(), cap);
        let mut saw_player = false;
        let mut idle = 0.0_f64;
        let mut last_fraction: Option<f64> = None;
        let mut last_tick = Instant::now();
        while Instant::now() < poll_deadline {
            if cancel() {
                return Err(EngineError::Message("Stopped".into()));
            }
            let now = Instant::now();
            if prompt_open {
                waited += now.saturating_duration_since(last_tick);
                cap = resolve_cap(started, waited);
            }
            last_tick = now;
            let players = active_players(client).await?;
            if let Some(player) = players.first() {
                saw_player = true;
                idle = 0.0;
                if let Some(resolved) = resolved_item(client, player).await? {
                    return Ok(resolved);
                }
                // A blank confirm often appears after Player.Open while the
                // add-on is still on the plugin URL. Dismiss it or the stream
                // never arrives and resolve times out.
                let tries = blank_tries;
                let blocking = clear_blocking_prompt(client, &mut blank_tries).await;
                prompt_open = blocking && tries == blank_tries && question_waiting(client).await;
                if blocking {
                    idle = 0.0;
                }
                if prompt_open {
                    let extended = (Instant::now() + Duration::from_secs(20)).min(cap);
                    if extended > poll_deadline {
                        poll_deadline = extended;
                    }
                }
            } else if saw_player {
                return Err(EngineError::Message(
                    "The add-on stopped before it produced a stream.".into(),
                ));
            } else {
                let folder = info_label(client, "Container.FolderPath")
                    .await
                    .unwrap_or_default();
                if changed_plugin(file, &previous, &folder) {
                    return Ok(Resolved::Browse(folder.trim().to_string()));
                }
                let tries = blank_tries;
                let blocking = clear_blocking_prompt(client, &mut blank_tries).await;
                prompt_open = blocking && tries == blank_tries && question_waiting(client).await;
                if blocking {
                    idle = 0.0;
                    // Picking from a long source list can take a while.
                    let extended = (Instant::now() + Duration::from_secs(20)).min(cap);
                    if extended > poll_deadline {
                        poll_deadline = extended;
                    }
                }
                if let Some(fraction) = visible_progress(client).await {
                    if progress_advanced(last_fraction, fraction) && Instant::now() < cap {
                        idle = 0.0;
                        let extended = (Instant::now() + Duration::from_secs(20)).min(cap);
                        if extended > poll_deadline {
                            poll_deadline = extended;
                        }
                    }
                    last_fraction = Some(fraction);
                } else if addon_busy_window(client).await {
                    // An add-on's own progress window is not window 10101.
                    // Treat it as work under way so a slow lookup is not cut off.
                    idle = 0.0;
                    if Instant::now() < cap {
                        let extended = (Instant::now() + Duration::from_secs(45)).min(cap);
                        if extended > poll_deadline {
                            poll_deadline = extended;
                        }
                    }
                }
                idle += RESOLVE_TICK.as_secs_f64();
                // After a cancelled question the add-on usually just returns.
                let cancelled = crate::prompts::CANCELS.load(Ordering::SeqCst) != cancels;
                if idle > if cancelled { 5.0 } else { 45.0 } {
                    break;
                }
            }
            tokio::time::sleep(RESOLVE_TICK).await;
        }
        let timed_out = Instant::now() >= poll_deadline;
        if timed_out && prompt_open {
            return Err(EngineError::Message(NO_CHOICE.into()));
        }
        if let Some(err) = open_error {
            return Err(err);
        }
        if timed_out {
            return Err(EngineError::Message(
                "Timed out waiting for the add-on.".into(),
            ));
        }
        if crate::prompts::CANCELS.load(Ordering::SeqCst) != cancels {
            return Err(EngineError::Message(CANCELLED.into()));
        }
        Err(EngineError::Message(NO_PLAYABLE_STREAM.into()))
    }
    .await;
    stop_players(client).await;
    let _ = rpc(
        client,
        "Application.SetMute",
        json!({"mute": muted}),
        Duration::from_secs(8),
    )
    .await;
    if result.is_err() {
        dismiss_progress(client).await;
    }
    result
}

/// How often a resolve looks at Kodi. The stream is handed on that much sooner.
const RESOLVE_TICK: Duration = Duration::from_millis(250);
const RESOLVE_POLL: Duration = Duration::from_secs(90);
const RESOLVE_CAP: Duration = Duration::from_secs(180);
/// The most a resolve may take, counting time spent on the user's choices.
const RESOLVE_PROMPT_MAX: Duration = Duration::from_secs(10 * 60);
const NO_CHOICE: &str = "Timed out: no choice was made in the add-on's question.";

/// Latest moment a resolve may still be extended. Time a prompt waited on
/// the user is added back, up to `RESOLVE_PROMPT_MAX` in all.
fn resolve_cap(started: Instant, waited: Duration) -> Instant {
    started + RESOLVE_CAP.saturating_add(waited).min(RESOLVE_PROMPT_MAX)
}

/// The first look at the player starts when Open returns. A cap that has
/// already passed still gets one look, so a stream that just opened is kept.
fn poll_window(now: Instant, cap: Instant) -> Instant {
    let until = (now + RESOLVE_POLL).min(cap);
    if until > now {
        until
    } else {
        now + RESOLVE_TICK
    }
}

fn progress_advanced(previous: Option<f64>, current: f64) -> bool {
    previous
        .map(|value| current > value + 0.001)
        .unwrap_or(true)
}

fn should_dismiss_window(id: i64) -> bool {
    id == 10101 || id == 10138 || addon_window(id)
}

/// Kodi numbers an add-on's own windows, such as a source list, 13000 to 13099.
fn addon_window(id: i64) -> bool {
    (13000..=13099).contains(&id)
}

fn player_open_params(file: &str) -> Value {
    json!({
        "item": {"file": file},
        "options": {"resume": false}
    })
}

/// A question waiting on someone: one of Kodi's own, or an add-on window
/// lately read as a list to choose from. An add-on's progress overlay is
/// also its own window, but nobody is answering it, so its time counts.
async fn question_waiting(client: &KodiClient) -> bool {
    match current_window(client).await {
        Some(id) if addon_window(id) => crate::prompts::addon_list_showing(),
        Some(id) => user_is_answering(id),
        None => false,
    }
}

/// Built-in questions, and any window of the add-on's own. Portal shows
/// those as prompts too, and one may be a source list someone is reading.
fn user_is_answering(id: i64) -> bool {
    matches!(id, 10100 | 10103 | 10109 | 10147 | 12000 | 12002) || addon_window(id)
}

fn labels_are_blank(values: &[String]) -> bool {
    values.iter().all(|value| value.trim().is_empty())
}

async fn yesno_is_blank(client: &KodiClient) -> bool {
    if current_window(client).await != Some(10100) {
        return false;
    }
    // Title and message only. Button captions are skin defaults ("Yes" / "No")
    // and still leave the page with an empty question.
    let Some(labels) = control_labels(
        client,
        &[
            "Control.GetLabel(1)",
            "Control.GetLabel(9)",
            "Control.GetLabel(12)",
        ],
    )
    .await
    else {
        return false;
    };
    yesno_copy_is_blank(&labels[0], &labels[1], &labels[2])
}

fn yesno_copy_is_blank(title: &str, line: &str, extra: &str) -> bool {
    labels_are_blank(&[title.to_string(), line.to_string(), extra.to_string()])
}

/// Returns true when resolve should keep waiting: a blank yes/no was pressed,
/// or a real question is on screen for the user.
async fn clear_blocking_prompt(client: &KodiClient, blank_tries: &mut u8) -> bool {
    let Some(id) = current_window(client).await else {
        return false;
    };
    if id == 10100 {
        if yesno_is_blank(client).await {
            if *blank_tries < 5 && dismiss_blank_yesno(client).await {
                *blank_tries = blank_tries.saturating_add(1);
                return true;
            }
            return false;
        }
        return true;
    }
    user_is_answering(id)
}

/// Close a yes/no whose title and message are empty. A hidden Kodi never
/// finishes the dialog close animation, and Back is ignored until that
/// animation ends, so the window is shown without activating it first.
async fn dismiss_blank_yesno(client: &KodiClient) -> bool {
    // Prompt reads close blank dialogs too. Checking again under the shared
    // gate keeps a second Back from cancelling the add-on's work.
    let _gate = crate::prompts::DIALOG_ACTIONS.lock().await;
    if !yesno_is_blank(client).await {
        return false;
    }
    let shown = crate::process::reveal_player_window();
    // Labels for a real resume question land a moment after the window opens.
    tokio::time::sleep(Duration::from_millis(600)).await;
    if !yesno_is_blank(client).await {
        drop(shown);
        return false;
    }
    let _ = rpc(client, "Input.Back", json!({}), Duration::from_secs(8)).await;
    let closed = wait_yesno_closed(client).await;
    if !closed && yesno_is_blank(client).await {
        let _ = rpc(
            client,
            "Input.ExecuteAction",
            json!({"action": "previousmenu"}),
            Duration::from_secs(8),
        )
        .await;
        let _ = wait_yesno_closed(client).await;
    }
    drop(shown);
    true
}

/// The close animation only advances while the window is on screen.
async fn wait_yesno_closed(client: &KodiClient) -> bool {
    for _ in 0..16 {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if current_window(client).await != Some(10100) {
            return true;
        }
    }
    false
}

async fn control_labels(client: &KodiClient, names: &[&str]) -> Option<Vec<String>> {
    let value = rpc(
        client,
        "XBMC.GetInfoLabels",
        json!({ "labels": names }),
        Duration::from_secs(8),
    )
    .await
    .ok()?;
    Some(
        names
            .iter()
            .map(|name| {
                value
                    .get(*name)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            })
            .collect(),
    )
}

async fn visible_progress(client: &KodiClient) -> Option<f64> {
    let id = current_window(client).await?;
    if id != 10101 && id != 10138 && id != 10160 {
        return None;
    }
    if id != 10101 {
        return Some(0.0);
    }
    let value = rpc(
        client,
        "XBMC.GetInfoLabels",
        json!({ "labels": ["System.Progressbar"] }),
        Duration::from_secs(8),
    )
    .await
    .ok()?;
    let text = value
        .get("System.Progressbar")
        .and_then(Value::as_str)
        .unwrap_or("");
    Some(
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .unwrap_or(0.0)
            / 100.0,
    )
}

/// Add-on busy windows that are not built-in progress windows,
/// found in the installed add-ons by their shape. While any of these is
/// active, resolve must wait.
async fn addon_busy_window(client: &KodiClient) -> bool {
    let keys = crate::dialog_catalog::overlay_keys();
    if keys.is_empty() {
        return false;
    }
    let Ok(labels) = rpc(
        client,
        "XBMC.GetInfoBooleans",
        json!({ "booleans": keys }),
        Duration::from_secs(4),
    )
    .await
    else {
        return false;
    };
    keys.iter()
        .any(|name| labels.get(name).and_then(Value::as_bool) == Some(true))
}

async fn current_window(client: &KodiClient) -> Option<i64> {
    let props = rpc(
        client,
        "GUI.GetProperties",
        json!({"properties": ["currentwindow"]}),
        Duration::from_secs(8),
    )
    .await
    .ok()?;
    props
        .get("currentwindow")
        .and_then(|window| window.get("id"))
        .and_then(Value::as_i64)
}

async fn dismiss_progress(client: &KodiClient) {
    let Some(id) = current_window(client).await else {
        return;
    };
    if !should_dismiss_window(id) {
        return;
    }
    // An add-on window left open would greet the next play. It only takes
    // Back while on screen, and a prompt answer may be acting on it too.
    let addon = addon_window(id);
    let _gate = if addon {
        Some(crate::prompts::DIALOG_ACTIONS.lock().await)
    } else {
        None
    };
    let shown = if addon {
        crate::process::reveal_player_window()
    } else {
        None
    };
    if addon {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if current_window(client).await != Some(id) {
            return;
        }
    }
    let _ = rpc(client, "Input.Back", json!({}), Duration::from_secs(8)).await;
    if addon {
        for _ in 0..12 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if current_window(client).await != Some(id) {
                break;
            }
        }
    }
    if current_window(client).await == Some(id) {
        let _ = rpc(
            client,
            "Input.ExecuteAction",
            json!({"action": "previousmenu"}),
            Duration::from_secs(8),
        )
        .await;
    }
    drop(shown);
}

async fn rpc(
    client: &KodiClient,
    method: &str,
    params: Value,
    limit: Duration,
) -> Result<Value, EngineError> {
    client
        .call(method, params, limit)
        .await
        .map_err(EngineError::from)
}

async fn info_label(client: &KodiClient, name: &str) -> Option<String> {
    let value = rpc(
        client,
        "XBMC.GetInfoLabels",
        json!({"labels": [name]}),
        Duration::from_secs(8),
    )
    .await
    .ok()?;
    let text = value.get(name).and_then(Value::as_str)?.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

async fn application_muted(client: &KodiClient) -> Option<bool> {
    let value = rpc(
        client,
        "Application.GetProperties",
        json!({"properties": ["muted"]}),
        Duration::from_secs(8),
    )
    .await
    .ok()?;
    value.get("muted").and_then(Value::as_bool)
}

async fn active_players(client: &KodiClient) -> Result<Vec<Value>, EngineError> {
    let value = rpc(
        client,
        "Player.GetActivePlayers",
        json!({}),
        Duration::from_secs(8),
    )
    .await?;
    Ok(player_list(&value))
}

fn player_list(value: &Value) -> Vec<Value> {
    let list = if let Some(list) = value.as_array() {
        list
    } else if let Some(list) = value.get("players").and_then(Value::as_array) {
        list
    } else {
        return Vec::new();
    };
    list.iter().filter(|row| row.is_object()).cloned().collect()
}

/// True while a dialog would refuse `ActivateWindow` or `Player.Open`.
/// A close animation is visible and not active, and it blocks too.
fn modal_blocks_open(visible: bool, active: bool) -> bool {
    visible || active
}

/// Back out of a dialog left up by the previous play. A failed boolean
/// read does not stop this play. This runs only before the open, not while
/// the add-on is still working.
async fn dismiss_leftover_modals(client: &KodiClient) {
    const VISIBLE: &str = "System.HasVisibleModalDialog";
    const ACTIVE: &str = "System.HasActiveModalDialog";
    let _gate = crate::prompts::DIALOG_ACTIONS.lock().await;
    for _ in 0..4 {
        let Ok(flags) = rpc(
            client,
            "XBMC.GetInfoBooleans",
            json!({ "booleans": [VISIBLE, ACTIVE] }),
            Duration::from_secs(4),
        )
        .await
        else {
            // A failed read does not stop this play, and it does not give up
            // on the dialog that would refuse the next Open.
            tokio::time::sleep(Duration::from_millis(300)).await;
            continue;
        };
        let visible = flags.get(VISIBLE).is_some_and(flag);
        let active = flags.get(ACTIVE).is_some_and(flag);
        if !modal_blocks_open(visible, active) {
            return;
        }
        let shown = crate::process::reveal_player_window();
        let _ = rpc(client, "Input.Back", json!({}), Duration::from_secs(8)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut settled = false;
        for _ in 0..8 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let Ok(flags) = rpc(
                client,
                "XBMC.GetInfoBooleans",
                json!({ "booleans": [VISIBLE, ACTIVE] }),
                Duration::from_secs(4),
            )
            .await
            else {
                continue;
            };
            let visible = flags.get(VISIBLE).is_some_and(flag);
            let active = flags.get(ACTIVE).is_some_and(flag);
            if !modal_blocks_open(visible, active) {
                settled = true;
                break;
            }
            if active {
                break;
            }
        }
        drop(shown);
        if settled {
            return;
        }
    }
}

async fn stop_players(client: &KodiClient) {
    let Ok(players) = active_players(client).await else {
        return;
    };
    for player in players {
        let Some(id) = player.get("playerid").cloned() else {
            continue;
        };
        let _ = rpc(
            client,
            "Player.Stop",
            json!({"playerid": id}),
            Duration::from_secs(8),
        )
        .await;
    }
}

/// `file` stays on the plugin item. `dynpath` is the opened stream. ffmpeg
/// only accepts a direct file or http URL.
fn direct_item_path(file: &str, dynpath: &str, filename: &str) -> Option<String> {
    [dynpath, file, filename]
        .into_iter()
        .map(str::trim)
        .find(|candidate| !candidate.is_empty() && !needs_kodi(candidate))
        .map(str::to_string)
}

async fn resolved_item(
    client: &KodiClient,
    player: &Value,
) -> Result<Option<Resolved>, EngineError> {
    let Some(player_id) = player.get("playerid").cloned() else {
        return Ok(None);
    };
    let item_result = match rpc(
        client,
        "Player.GetItem",
        json!({
            "playerid": player_id,
            "properties": ["file", "dynpath", "title", "thumbnail"]
        }),
        Duration::from_secs(8),
    )
    .await
    {
        Ok(value) => value,
        Err(_) => {
            rpc(
                client,
                "Player.GetItem",
                json!({"playerid": player_id, "properties": ["file", "title", "thumbnail"]}),
                Duration::from_secs(8),
            )
            .await?
        }
    };
    let item = item_result
        .get("item")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or(item_result);
    let file = item.get("file").and_then(Value::as_str).unwrap_or("");
    let dynpath = item.get("dynpath").and_then(Value::as_str).unwrap_or("");
    let filename = if direct_item_path(file, dynpath, "").is_none() {
        info_label(client, "Player.Filenameandpath")
            .await
            .unwrap_or_default()
    } else {
        String::new()
    };
    let Some(resolved) = direct_item_path(file, dynpath, &filename) else {
        return Ok(None);
    };
    let title = plain_label(
        item.get("title")
            .and_then(Value::as_str)
            .or_else(|| item.get("label").and_then(Value::as_str))
            .unwrap_or(""),
    );
    let title = if title.is_empty() { None } else { Some(title) };
    Ok(Some(Resolved::File {
        file: resolved,
        title,
    }))
}

#[derive(Clone)]
struct StreamInfo {
    index: Option<i64>,
    codec: String,
    pix_fmt: String,
    height: i64,
    transfer: String,
    /// Woven from two fields, as in 1080i broadcast. Needs deinterlacing
    /// when re-encoded, or the picture combs on motion.
    interlaced: bool,
    is_default: bool,
    /// A forced subtitle stream only covers foreign-language parts.
    forced: bool,
    lang: Option<String>,
    title: Option<String>,
    /// Average bits a second, when the file says.
    bitrate: Option<u64>,
}

struct Probe {
    video: Vec<StreamInfo>,
    audio: Vec<StreamInfo>,
    subs: Vec<StreamInfo>,
    duration: Option<f64>,
    /// The whole file's average bits a second, every stream together.
    bitrate: Option<u64>,
}

fn probe_file(
    ffprobe: &Path,
    media: &str,
    headers: &[(String, String)],
) -> Result<Probe, EngineError> {
    assert_safe_input(media)?;
    let refs = header_refs(headers);
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
    ];
    args.extend(input_args(media, &refs));
    args.push(media.to_string());
    let run = || {
        capture(ffprobe, &args, Duration::from_secs(45)).map_err(|err| match err {
            CaptureError::Timeout => {
                EngineError::Message("Could not read the stream: timed out".into())
            }
            CaptureError::Io => EngineError::Message("Could not read the stream".into()),
        })
    };
    let mut captured = run()?;
    let mut untrusted = !captured.status.success() && certificate_refused(&captured.stderr);
    // A new PC may not have the site's root certificate yet. Once Windows has
    // fetched it, ffmpeg trusts the site too.
    if untrusted && crate::roots::fetch_root(media) {
        tracing::info!("Fetched the root certificate for the stream's site");
        captured = run()?;
        untrusted = !captured.status.success() && certificate_refused(&captured.stderr);
    }
    if untrusted {
        return Err(EngineError::Message(
            "Could not read the stream: its site's certificate could not be verified.".into(),
        ));
    }
    if !captured.status.success() {
        let detail = failure_line(&String::from_utf8_lossy(&captured.stderr));
        let message = if detail.is_empty() {
            "Could not read the stream".to_string()
        } else {
            format!("Could not read the stream: {detail}")
        };
        return Err(EngineError::Message(clip(&message, 500)));
    }
    let payload: Value = serde_json::from_slice(&captured.stdout).unwrap_or(Value::Null);
    Ok(parse_probe(&payload))
}

/// ffmpeg's TLS said the site's certificate is not trusted. Its last line
/// is only "I/O error", so every line is checked.
fn certificate_refused(stderr: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    text.lines().any(|line| {
        line.contains("certificate") && (line.contains("verif") || line.contains("untrusted"))
    })
}

fn header_refs(headers: &[(String, String)]) -> Vec<(&str, &str)> {
    headers
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

fn parse_probe(payload: &Value) -> Probe {
    let streams = payload.get("streams").and_then(Value::as_array);
    let mut video = Vec::new();
    let mut audio = Vec::new();
    let mut subs = Vec::new();
    if let Some(streams) = streams {
        for stream in streams {
            let kind = stream
                .get("codec_type")
                .and_then(Value::as_str)
                .unwrap_or("");
            let disposition = stream.get("disposition");
            if kind == "video"
                && disposition
                    .and_then(|value| value.get("attached_pic"))
                    .is_some_and(flag)
            {
                continue;
            }
            let info = StreamInfo {
                index: stream.get("index").and_then(Value::as_i64),
                codec: stream
                    .get("codec_name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                pix_fmt: stream
                    .get("pix_fmt")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                height: stream
                    .get("height")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .max(0),
                transfer: stream
                    .get("color_transfer")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                interlaced: stream
                    .get("field_order")
                    .and_then(Value::as_str)
                    .is_some_and(is_interlaced),
                is_default: disposition
                    .and_then(|value| value.get("default"))
                    .is_some_and(flag),
                forced: disposition
                    .and_then(|value| value.get("forced"))
                    .is_some_and(flag),
                lang: tag(stream, "language"),
                title: tag(stream, "title"),
                bitrate: stream_bitrate(stream),
            };
            match kind {
                "video" => video.push(info),
                "audio" => audio.push(info),
                "subtitle" => subs.push(info),
                _ => {}
            }
        }
    }
    let format = payload.get("format");
    let duration = format
        .and_then(|format| format.get("duration"))
        .and_then(number_value);
    let bitrate = format
        .and_then(|format| format.get("bit_rate"))
        .and_then(number_value)
        .and_then(whole_rate);
    Probe {
        video,
        audio,
        subs,
        duration,
        bitrate,
    }
}

/// A stream's own average rate: ffprobe's `bit_rate`, else the `BPS`
/// statistics tag mkvmerge writes (also as `BPS-eng`), which Matroska files
/// carry instead.
fn stream_bitrate(stream: &Value) -> Option<u64> {
    if let Some(rate) = stream.get("bit_rate").and_then(number_value) {
        return whole_rate(rate);
    }
    let tags = stream.get("tags")?.as_object()?;
    tags.iter()
        .filter(|(key, _)| {
            let key = key.to_ascii_uppercase();
            key == "BPS" || key.starts_with("BPS-")
        })
        .find_map(|(_, value)| value.as_str()?.trim().parse::<f64>().ok())
        .and_then(whole_rate)
}

fn whole_rate(rate: f64) -> Option<u64> {
    (rate.is_finite() && rate >= 1.0).then_some(rate as u64)
}

/// The picture's average rate: its own, else the whole file's less what the
/// audio streams say they take. Subtitles are too small to matter.
fn video_bitrate(probe: &Probe, video: &StreamInfo) -> Option<u64> {
    if video.bitrate.is_some() {
        return video.bitrate;
    }
    let audio: u64 = probe.audio.iter().filter_map(|stream| stream.bitrate).sum();
    probe
        .bitrate
        .map(|total| total.saturating_sub(audio))
        .filter(|rate| *rate > 0)
}

fn tag(stream: &Value, name: &str) -> Option<String> {
    let tags = stream.get("tags")?;
    let direct = tags.get(name).and_then(Value::as_str);
    let text = direct.or_else(|| {
        tags.as_object().and_then(|map| {
            map.iter().find_map(|(key, value)| {
                if key.eq_ignore_ascii_case(name) {
                    value.as_str()
                } else {
                    None
                }
            })
        })
    })?;
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_i64() == Some(1),
        Value::String(text) => text == "1" || text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn number_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

fn default_audio(streams: &[StreamInfo]) -> Option<&StreamInfo> {
    streams
        .iter()
        .find(|stream| stream.is_default)
        .or_else(|| streams.first())
}

/// The track to play. A preferred language wins when the file has it, and a
/// commentary or descriptive track loses to a normal track of that language.
/// Otherwise the file's default track, then the first.
fn pick_audio<'a>(streams: &'a [StreamInfo], preferred: Option<&str>) -> Option<&'a StreamInfo> {
    if let Some(key) = preferred.and_then(lang_key) {
        let matched: Vec<&StreamInfo> = streams
            .iter()
            .filter(|stream| stream_lang(stream).as_deref() == Some(key.as_str()))
            .collect();
        if let Some(chosen) = choose_audio(&matched) {
            return Some(chosen);
        }
    }
    default_audio(streams)
}

/// Among one language: the default main track, then any main track, then the first.
fn choose_audio<'a>(streams: &[&'a StreamInfo]) -> Option<&'a StreamInfo> {
    streams
        .iter()
        .copied()
        .find(|stream| stream.is_default && !is_commentary(stream))
        .or_else(|| {
            streams
                .iter()
                .copied()
                .find(|stream| !is_commentary(stream))
        })
        .or_else(|| streams.first().copied())
}

fn is_commentary(stream: &StreamInfo) -> bool {
    let title = stream.title.as_deref().unwrap_or("").to_ascii_lowercase();
    title.contains("commentary") || title.contains("descriptive")
}

fn stream_lang(stream: &StreamInfo) -> Option<String> {
    stream.lang.as_deref().and_then(lang_key)
}

/// "en", "eng", and "en-US" are one language. An unknown tag is kept so two
/// copies of it still match. Undetermined tags match nothing.
fn lang_key(raw: &str) -> Option<String> {
    let primary = raw
        .trim()
        .split(['-', '_', ' '])
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if primary.is_empty() {
        return None;
    }
    let key = match primary.as_str() {
        "en" | "eng" | "english" => "en",
        "ja" | "jpn" | "japanese" => "ja",
        "es" | "spa" | "esl" | "esp" | "spanish" => "es",
        "fr" | "fre" | "fra" | "french" => "fr",
        "de" | "ger" | "deu" | "german" => "de",
        "pt" | "por" | "portuguese" => "pt",
        "zh" | "chi" | "zho" | "chinese" => "zh",
        "ko" | "kor" | "korean" => "ko",
        "it" | "ita" | "italian" => "it",
        "ru" | "rus" | "russian" => "ru",
        "nl" | "dut" | "nld" | "dutch" => "nl",
        "sv" | "swe" | "swedish" => "sv",
        "no" | "nor" | "nob" | "nno" | "norwegian" => "no",
        "da" | "dan" | "danish" => "da",
        "fi" | "fin" | "finnish" => "fi",
        "pl" | "pol" | "polish" => "pl",
        "cs" | "cze" | "ces" | "czech" => "cs",
        "hu" | "hun" | "hungarian" => "hu",
        "tr" | "tur" | "turkish" => "tr",
        "ar" | "ara" | "arabic" => "ar",
        "he" | "heb" | "hebrew" => "he",
        "hi" | "hin" | "hindi" => "hi",
        "th" | "tha" | "thai" => "th",
        "vi" | "vie" | "vietnamese" => "vi",
        "uk" | "ukr" | "ukrainian" => "uk",
        "el" | "gre" | "ell" | "greek" => "el",
        "ro" | "rum" | "ron" | "romanian" => "ro",
        "bg" | "bul" | "bulgarian" => "bg",
        "hr" | "hrv" | "croatian" => "hr",
        "sk" | "slo" | "slk" | "slovak" => "sk",
        "sl" | "slv" | "slovenian" | "slovene" => "sl",
        "id" | "ind" | "indonesian" => "id",
        "ms" | "may" | "msa" | "malay" => "ms",
        "fa" | "per" | "fas" | "persian" | "farsi" => "fa",
        "ca" | "cat" | "catalan" => "ca",
        "und" | "undetermined" | "unknown" | "mul" | "mis" | "zxx" => return None,
        other
            if (2..=3).contains(&other.len())
                && other.chars().all(|ch| ch.is_ascii_alphabetic()) =>
        {
            other
        }
        _ => return None,
    };
    Some(key.to_string())
}

fn language_name(key: &str) -> Option<&'static str> {
    Some(match key {
        "en" => "English",
        "ja" => "Japanese",
        "es" => "Spanish",
        "fr" => "French",
        "de" => "German",
        "pt" => "Portuguese",
        "zh" => "Chinese",
        "ko" => "Korean",
        "it" => "Italian",
        "ru" => "Russian",
        "nl" => "Dutch",
        "sv" => "Swedish",
        "no" => "Norwegian",
        "da" => "Danish",
        "fi" => "Finnish",
        "pl" => "Polish",
        "cs" => "Czech",
        "hu" => "Hungarian",
        "tr" => "Turkish",
        "ar" => "Arabic",
        "he" => "Hebrew",
        "hi" => "Hindi",
        "th" => "Thai",
        "vi" => "Vietnamese",
        "uk" => "Ukrainian",
        "el" => "Greek",
        "ro" => "Romanian",
        "bg" => "Bulgarian",
        "hr" => "Croatian",
        "sk" => "Slovak",
        "sl" => "Slovenian",
        "id" => "Indonesian",
        "ms" => "Malay",
        "fa" => "Persian",
        "ca" => "Catalan",
        _ => return None,
    })
}

fn audio_base_label(stream: &StreamInfo) -> String {
    let key = stream_lang(stream);
    let named = key.as_deref().and_then(language_name);
    let title = stream
        .title
        .as_deref()
        .map(plain_label)
        .filter(|text| !text.is_empty());
    let mut label = if let Some(name) = named {
        name.to_string()
    } else if let Some(title) = title {
        title
    } else if let Some(key) = key.as_deref() {
        key.to_ascii_uppercase()
    } else {
        "Audio".to_string()
    };
    if label.chars().count() > 60 {
        let end = label
            .char_indices()
            .nth(57)
            .map(|(index, _)| index)
            .unwrap_or(label.len());
        label.truncate(end);
        label.push('…');
    }
    if is_commentary(stream) {
        let lower = label.to_ascii_lowercase();
        if !lower.contains("commentary") && !lower.contains("descriptive") {
            let kind = stream.title.as_deref().unwrap_or("").to_ascii_lowercase();
            let kind = if kind.contains("descriptive") {
                "descriptive"
            } else {
                "commentary"
            };
            label = format!("{label} ({kind})");
        }
    }
    label
}

/// Labels for the dock. A repeated name is numbered from the second one.
/// At most 32 tracks; a stream ffprobe did not number is left out.
fn audio_listing(
    streams: &[StreamInfo],
    chosen: Option<&StreamInfo>,
) -> (Vec<AudioTrack>, Option<u32>) {
    let mut rows = Vec::new();
    let mut bases = Vec::new();
    for stream in streams {
        let Some(index) = stream.index.and_then(|value| u32::try_from(value).ok()) else {
            continue;
        };
        if rows.len() >= 32 {
            break;
        }
        bases.push(audio_base_label(stream));
        rows.push(AudioTrack {
            index,
            label: String::new(),
            lang: stream_lang(stream),
        });
    }
    let mut totals: HashMap<String, usize> = HashMap::new();
    for base in &bases {
        *totals.entry(base.clone()).or_default() += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (track, base) in rows.iter_mut().zip(bases) {
        let count = totals.get(&base).copied().unwrap_or(1);
        let number = {
            let slot = seen.entry(base.clone()).or_default();
            *slot += 1;
            *slot
        };
        track.label = if count > 1 && number > 1 {
            format!("{base} {number}")
        } else {
            base
        };
    }
    let index = chosen
        .and_then(|stream| stream.index)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|index| rows.iter().any(|track| track.index == *index));
    (rows, index)
}

/// ffprobe names the field order of an interlaced picture by which field
/// comes first and which is shown first. Progressive and unknown are not.
fn is_interlaced(field_order: &str) -> bool {
    matches!(
        field_order.to_ascii_lowercase().as_str(),
        "tt" | "bb" | "tb" | "bt"
    )
}

fn is_hdr(transfer: &str) -> bool {
    let lower = transfer.to_ascii_lowercase();
    lower == "smpte2084" || lower == "arib-std-b67"
}

fn ffmpeg_has_zscale(ffmpeg: &Path) -> bool {
    let args = vec!["-hide_banner".into(), "-filters".into()];
    let Ok(captured) = capture(ffmpeg, &args, Duration::from_secs(20)) else {
        return false;
    };
    let text = String::from_utf8_lossy(&captured.stdout);
    contains_word(&text, "zscale")
}

fn contains_word(text: &str, word: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let needle = word.as_bytes();
    if needle.is_empty() {
        return false;
    }
    let mut index = 0;
    while index + needle.len() <= bytes.len() {
        if &bytes[index..index + needle.len()] == needle {
            let before = index == 0 || !bytes[index - 1].is_ascii_alphanumeric();
            let after = index + needle.len();
            let after_ok = after == bytes.len() || !bytes[after].is_ascii_alphanumeric();
            if before && after_ok {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn ffmpeg_args(
    source: &MediaSource,
    seek: u64,
    out: &Path,
    plan: Option<&VideoPlan>,
    zscale: bool,
) -> Result<Vec<String>, EngineError> {
    let input = source.input.as_str();
    assert_safe_input(input)?;
    let refs = header_refs(&source.headers);
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    args.extend(input_args(input, &refs));
    // The first stretch comes as fast as the source sends it, so the video
    // starts at once. After that ffmpeg reads a few times faster than it
    // plays. A copied picture would otherwise pull the whole file at line
    // speed while it plays, and that load makes the sound and picture stutter.
    args.extend([
        "-readrate".into(),
        READ_RATE.to_string(),
        "-readrate_initial_burst".into(),
        READ_BURST_SECONDS.to_string(),
    ]);
    let copy_seek = copied_from_keyframe(seek, plan);
    if seek > 0 {
        if copy_seek {
            // A copied picture can only start on a keyframe. Start the audio
            // there too, and keep the file's own clock so the real start can
            // be read back from the first segment.
            args.push("-noaccurate_seek".into());
        }
        args.push("-ss".into());
        args.push(seek.to_string());
    }
    args.push("-i".into());
    args.push(input.to_string());
    if copy_seek {
        args.push("-copyts".into());
    }
    if let (Some(video), Some(plan)) = (source.video.as_ref(), plan) {
        args.push("-map".into());
        args.push(stream_map(video, "v"));
        args.extend(video_args(plan, &video.transfer, video.interlaced, zscale));
    } else {
        args.push("-vn".into());
    }
    args.extend(audio_args(source.audio.as_ref()));
    args.extend([
        "-sn".into(),
        "-dn".into(),
        "-max_muxing_queue_size".into(),
        "2048".into(),
        "-f".into(),
        "hls".into(),
        "-hls_time".into(),
        SEGMENT_SECONDS.to_string(),
        "-hls_list_size".into(),
        "0".into(),
        "-hls_playlist_type".into(),
        "event".into(),
        "-hls_flags".into(),
        "independent_segments+temp_file".into(),
        "-hls_segment_filename".into(),
        // Every % in this name is a number pattern to ffmpeg; one in the
        // folder (a Windows user name can hold one) is written %%.
        format!("{}{}seg%05d.ts", out.display().to_string().replace('%', "%%"), std::path::MAIN_SEPARATOR),
        out.join("index.m3u8").display().to_string(),
    ]);
    Ok(args)
}

fn video_args(plan: &VideoPlan, transfer: &str, interlaced: bool, zscale: bool) -> Vec<String> {
    if plan.copy {
        return vec!["-c:v".into(), "copy".into()];
    }
    let mut filters = Vec::new();
    if interlaced {
        // Weave the fields back into whole frames before anything resizes
        // them, or scaling smears the two fields together. Frames the file
        // marks as progressive pass through untouched.
        filters.push("bwdif=mode=send_frame:parity=auto:deint=interlaced".into());
    }
    if let Some(scale) = plan.scale {
        filters.push(format!("scale=-2:{scale}"));
    } else {
        // x264 refuses an odd width or height in 4:2:0, as in a 1920x803 crop.
        // Dropping the last row or column costs nothing on an even picture.
        filters.push("crop=trunc(iw/2)*2:trunc(ih/2)*2".into());
    }
    if is_hdr(transfer) && zscale {
        // Many HDR files tag only the transfer. zscale then finds no path
        // between colorspaces and the whole transcode fails, so name the
        // BT.2020 primaries and matrix every HDR10 and HLG picture uses.
        filters.push(format!(
            "setparams=color_primaries=bt2020:color_trc={}:colorspace=bt2020nc",
            transfer.to_ascii_lowercase()
        ));
        filters.push("zscale=t=linear:npl=100".into());
        filters.push("format=gbrpf32le".into());
        filters.push("zscale=p=bt709".into());
        filters.push("tonemap=hable:desat=0".into());
        filters.push("zscale=t=bt709:m=bt709:r=tv".into());
    }
    filters.push("format=yuv420p".into());
    let mut args = vec![
        "-vf".into(),
        filters.join(","),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "21".into(),
    ];
    if let Some(rate) = plan.maxrate {
        // CRF keeps calm scenes small; the ceiling caps the busy ones, over
        // a two-second buffer.
        args.extend([
            "-maxrate".into(),
            rate.to_string(),
            "-bufsize".into(),
            (rate * 2).to_string(),
        ]);
    }
    args.extend([
        "-threads".into(),
        thread_count().to_string(),
        "-force_key_frames".into(),
        format!("expr:gte(t,n_forced*{SEGMENT_SECONDS})"),
    ]);
    args
}

/// How many times faster than playback ffmpeg reads once the burst is in.
const READ_RATE: u32 = 4;
/// Seconds of the source read at full speed before `READ_RATE` applies.
/// A copied picture is cut on the file's own keyframes, which can be ten
/// seconds apart, so this covers the first segment and the one after it.
const READ_BURST_SECONDS: u32 = 30;

/// Short segments let the first one land sooner, so the video starts sooner.
const SEGMENT_SECONDS: u32 = 2;

/// mpegts starts its clock this far in. With `-copyts` the first segment's
/// picture starts here plus the keyframe's time in the file.
const MPEGTS_START: f64 = 1.4;

/// A skip or resume whose picture is copied starts on the keyframe at or
/// before the asked time, not on the time itself.
fn copied_from_keyframe(seek: u64, plan: Option<&VideoPlan>) -> bool {
    seek > 0 && plan.is_some_and(|plan| plan.copy)
}

/// Where a copied picture really starts on the file's timeline, read from
/// the first segment. None when it cannot be read or makes no sense, and
/// the asked time is used.
fn copied_start(ffprobe: &Path, segment: &Path, asked: u64) -> Option<f64> {
    let args: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-select_streams".into(),
        "v:0".into(),
        "-show_entries".into(),
        "stream=start_time".into(),
        "-of".into(),
        "csv=p=0".into(),
        segment.display().to_string(),
    ];
    let captured = capture(ffprobe, &args, Duration::from_secs(5)).ok()?;
    if !captured.status.success() {
        return None;
    }
    let start = String::from_utf8_lossy(&captured.stdout)
        .trim()
        .parse()
        .ok()?;
    keyframe_start(start, asked)
}

/// The keyframe time behind a copied segment's first picture. A keyframe
/// lies at or before the asked time and within a few minutes of it.
fn keyframe_start(segment_start: f64, asked: u64) -> Option<f64> {
    if !segment_start.is_finite() {
        return None;
    }
    let asked = asked as f64;
    let at = (segment_start - MPEGTS_START).max(0.0);
    if at > asked + 0.5 || at < asked - 300.0 {
        return None;
    }
    Some(at.min(asked))
}

fn audio_args(audio: Option<&StreamInfo>) -> Vec<String> {
    let Some(audio) = audio else {
        return vec!["-an".into()];
    };
    let mut args = vec!["-map".into(), stream_map(audio, "a")];
    if copy_audio(&audio.codec) {
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        args.extend([
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
            "-ac".into(),
            "2".into(),
        ]);
    }
    args
}

fn stream_map(stream: &StreamInfo, kind: &str) -> String {
    match stream.index {
        Some(index) if index >= 0 => format!("0:{index}"),
        _ => format!("0:{kind}:0"),
    }
}

fn thread_count() -> usize {
    let cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(2);
    // Leave two cores for the player and the desktop. The encode uses the rest.
    cpus.saturating_sub(2).max(2)
}

fn spawn_ffmpeg(ffmpeg: &Path, args: &[String]) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    // Normal priority. A below-normal encode loses cores to the browser and
    // the picture falls behind the playhead. The window stays hidden.
    Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
}

fn drain_stderr(child: &mut Child) -> Arc<Mutex<String>> {
    let stderr = child.stderr.take();
    let bucket = Arc::new(Mutex::new(String::new()));
    let shared = bucket.clone();
    thread::spawn(move || {
        let Some(mut stderr) = stderr else {
            return;
        };
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    let mut text = shared.lock().unwrap_or_else(|poison| poison.into_inner());
                    text.push_str(&String::from_utf8_lossy(&buf[..size]));
                    if text.len() > 4000 {
                        let mut end = text.len() - 4000;
                        while end < text.len() && !text.is_char_boundary(end) {
                            end += 1;
                        }
                        text.drain(..end);
                    }
                }
            }
        }
    });
    bucket
}

async fn wait_for_segment(
    child: &Mutex<Child>,
    playlist: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(), EngineError> {
    wait_for_segment_within(child, playlist, cancel, FIRST_SEGMENT_LIMIT).await
}

const FIRST_SEGMENT_LIMIT: Duration = Duration::from_secs(60);
const SLOW_START: &str = "The video took too long to start.";

/// A failure that a second try usually gets past: something did not answer
/// in time. An unanswered question, a cancel or a missing stream is not.
fn worth_retry(err: &EngineError) -> bool {
    match err {
        EngineError::Rpc(crate::rpc::RpcFail::Timeout(_) | crate::rpc::RpcFail::ConnectTimeout) => {
            true
        }
        EngineError::Message(text) => {
            let lower = text.to_ascii_lowercase();
            text != NO_CHOICE
                && (text == SLOW_START || lower.contains("timed out") || lower.contains("time out"))
        }
        _ => false,
    }
}

async fn wait_for_segment_within(
    child: &Mutex<Child>,
    playlist: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
    limit: Duration,
) -> Result<(), EngineError> {
    let deadline = Instant::now() + limit;
    let mut first_segment: Option<Instant> = None;
    let exited = loop {
        if cancel() {
            return Err(EngineError::Message("Stopped".into()));
        }
        if let Some(depth) = playlist_depth(playlist) {
            if depth.segments > 0 && first_segment.is_none() {
                first_segment = Some(Instant::now());
            }
            let since_first = first_segment.map(|seen| seen.elapsed());
            if runway_ready(&depth, since_first) {
                return Ok(());
            }
        }
        if child_exited(child) {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    // A process that already wrote media is handled by the exit check, which
    // fails the job once the playlist has no end marker. Waiting out the
    // runway after ffmpeg has gone would only hide that.
    if playlist_depth(playlist).is_some_and(|depth| depth.segments > 0) {
        Ok(())
    } else if exited {
        Err(EngineError::Message("Transcoding failed to start".into()))
    } else {
        // ffmpeg is still reading: a slow server, or a picture too large to
        // encode in time. It did not fail.
        Err(EngineError::Message(SLOW_START.into()))
    }
}

fn child_exited(child: &Mutex<Child>) -> bool {
    matches!(lock_child(child).try_wait(), Ok(Some(_)))
}

fn lock_child(child: &Mutex<Child>) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Seconds of media to have published before the player is told to start.
/// Less than this, and the player sits on the newest segment: one slow
/// piece then looks like the stream died.
const START_RUNWAY: f64 = 12.0;
/// After the first segment, do not hold a slow source for the whole minute.
const START_GRACE: Duration = Duration::from_secs(8);

struct PlaylistDepth {
    seconds: f64,
    segments: u32,
    ended: bool,
}

fn playlist_depth(path: &Path) -> Option<PlaylistDepth> {
    let text = fs::read_to_string(path).ok()?;
    Some(depth_of(&text))
}

fn depth_of(text: &str) -> PlaylistDepth {
    let mut seconds = 0.0;
    let mut segments = 0;
    let mut ended = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let number = rest.split([',', ' ']).next().unwrap_or("");
            if let Ok(value) = number.parse::<f64>() {
                if value.is_finite() && value > 0.0 {
                    seconds += value;
                }
            }
        } else if line == "#EXT-X-ENDLIST" {
            ended = true;
        } else if !line.is_empty() && !line.starts_with('#') && line.ends_with(".ts") {
            segments += 1;
        }
    }
    PlaylistDepth {
        seconds,
        segments,
        ended,
    }
}

/// Ready once there is a cushion of media, the file has ended, or the first
/// segment has been sitting there through the grace period.
fn runway_ready(depth: &PlaylistDepth, since_first: Option<Duration>) -> bool {
    if depth.segments == 0 {
        return false;
    }
    if depth.ended || depth.seconds + 0.05 >= START_RUNWAY {
        return true;
    }
    since_first.is_some_and(|elapsed| elapsed >= START_GRACE)
}

/// The first eight subtitle streams that can become WebVTT, each with its
/// track number among all of the file's subtitle streams.
fn subtitle_streams(subs: &[StreamInfo]) -> impl Iterator<Item = (u32, &StreamInfo)> {
    subs.iter()
        .enumerate()
        .filter(|(_, stream)| text_subtitle_codec(&stream.codec))
        .take(8)
        .map(|(offset, stream)| ((offset as u32) + 1, stream))
}

/// A remote file of many gigabytes takes minutes to read in full, and this
/// runs in the background beside the transcode.
const SUBTITLE_LIMIT: Duration = Duration::from_secs(10 * 60);

/// One ffmpeg reads the input once and writes every wanted stream to its own
/// file in `scratch`, which is removed afterwards. Stops early once `cancel`
/// is true.
fn extract_subtitles(
    ffmpeg: &Path,
    input: &str,
    headers: &[(String, String)],
    subs: &[StreamInfo],
    scratch: &Path,
    cancel: &dyn Fn() -> bool,
) -> Vec<SubtitleText> {
    let wanted: Vec<(u32, &StreamInfo)> = subtitle_streams(subs).collect();
    if wanted.is_empty() || cancel() || fs::create_dir_all(scratch).is_err() {
        return Vec::new();
    }
    let started = Instant::now();
    let texts = if dump_subtitles(ffmpeg, input, headers, &wanted, scratch, cancel) {
        collect_subtitles(&wanted, scratch)
    } else if wanted.len() > 1 && !cancel() && started.elapsed() < ONE_BY_ONE_WINDOW {
        // One track ffmpeg cannot convert fails the whole run at startup;
        // then each goes on its own so the others still arrive. A run that
        // failed later was slow reading, which another pass would not fix.
        let mut texts = Vec::new();
        for one in &wanted {
            if cancel() {
                break;
            }
            let one = std::slice::from_ref(one);
            if dump_subtitles(ffmpeg, input, headers, one, scratch, cancel) {
                texts.extend(collect_subtitles(one, scratch));
            }
        }
        texts
    } else {
        Vec::new()
    };
    remove_dir(scratch);
    texts
}

/// A one-pass extraction that fails sooner than this is retried track by track.
const ONE_BY_ONE_WINDOW: Duration = Duration::from_secs(60);

fn subtitle_file(number: u32) -> String {
    format!("sub{number:05}.vtt")
}

/// Reads the WebVTT files one ffmpeg run left in `scratch`, skipping any that
/// are missing, empty or too large.
fn collect_subtitles(wanted: &[(u32, &StreamInfo)], scratch: &Path) -> Vec<SubtitleText> {
    let mut texts = Vec::new();
    for &(number, stream) in wanted {
        let path = scratch.join(subtitle_file(number));
        let too_big = fs::metadata(&path).map_or(true, |meta| meta.len() > 2 * 1024 * 1024);
        if too_big {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if text.trim().is_empty() || !text.to_ascii_uppercase().contains("WEBVTT") {
            continue;
        }
        let label = stream
            .title
            .clone()
            .or_else(|| stream.lang.clone())
            .unwrap_or_else(|| {
                if number == 1 {
                    "Subtitles".into()
                } else {
                    format!("Subtitles {number}")
                }
            });
        texts.push(SubtitleText {
            id: number,
            label: plain_label(&label),
            lang: stream.lang.clone(),
            forced: stream.forced,
            text,
        });
    }
    texts
}

/// Shift each kept subtitle to a session that starts at `seek` and write it
/// beside the playlist.
fn write_subtitles(
    texts: &[SubtitleText],
    dir: &Path,
    session: &str,
    seek: f64,
) -> Vec<SubtitleTrack> {
    let mut tracks = Vec::new();
    for sub in texts {
        let name = subtitle_file(sub.id);
        if fs::write(dir.join(&name), shift_vtt(&sub.text, -seek)).is_err() {
            continue;
        }
        tracks.push(SubtitleTrack {
            id: sub.id,
            label: sub.label.clone(),
            lang: sub.lang.clone(),
            forced: sub.forced,
            url: format!("/media/{session}/{name}"),
        });
    }
    tracks
}

fn text_subtitle_codec(codec: &str) -> bool {
    let lower = codec.trim().to_ascii_lowercase();
    if lower.is_empty() {
        return true;
    }
    // Image / bitmap subtitle streams cannot become WebVTT in the browser player.
    if matches!(
        lower.as_str(),
        "hdmv_pgs_subtitle"
            | "pgssub"
            | "dvd_subtitle"
            | "dvdsub"
            | "dvb_subtitle"
            | "xsub"
            | "vobsub"
    ) {
        return false;
    }
    true
}

/// True only when ffmpeg finished cleanly within the limit.
fn dump_subtitles(
    ffmpeg: &Path,
    input: &str,
    headers: &[(String, String)],
    wanted: &[(u32, &StreamInfo)],
    scratch: &Path,
    cancel: &dyn Fn() -> bool,
) -> bool {
    if assert_safe_input(input).is_err() {
        return false;
    }
    let args = subtitle_args(input, headers, wanted, scratch);
    capture_until(ffmpeg, &args, SUBTITLE_LIMIT, cancel)
        .is_ok_and(|captured| captured.status.success())
}

/// One input and one WebVTT output file per wanted stream.
fn subtitle_args(
    input: &str,
    headers: &[(String, String)],
    wanted: &[(u32, &StreamInfo)],
    scratch: &Path,
) -> Vec<String> {
    let refs = header_refs(headers);
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-y".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    args.extend(input_args(input, &refs));
    args.push("-i".into());
    args.push(input.to_string());
    for &(number, stream) in wanted {
        args.push("-map".into());
        args.push(match stream.index {
            Some(index) if index >= 0 => format!("0:{index}"),
            _ => format!("0:s:{}", number - 1),
        });
        // Force a text conversion so mov_text / ass / srt land as WebVTT.
        args.extend([
            "-c:s".into(),
            "webvtt".into(),
            "-f".into(),
            "webvtt".into(),
            scratch
                .join(subtitle_file(number))
                .to_string_lossy()
                .into_owned(),
        ]);
    }
    args
}

struct Captured {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

enum CaptureError {
    Timeout,
    Io,
}

fn capture(bin: &Path, args: &[String], limit: Duration) -> Result<Captured, CaptureError> {
    capture_until(bin, args, limit, &|| false)
}

/// Like `capture`, and also ends the process once `cancel` turns true, which
/// counts as a timeout.
fn capture_until(
    bin: &Path,
    args: &[String],
    limit: Duration,
    cancel: &dyn Fn() -> bool,
) -> Result<Captured, CaptureError> {
    use std::os::windows::process::CommandExt;
    let mut child = Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL)
        .spawn()
        .map_err(|_| CaptureError::Io)?;
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_handle = thread::spawn(move || read_all(stdout.take()));
    let err_handle = thread::spawn(move || read_all(stderr.take()));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > limit || cancel() => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                return Err(CaptureError::Timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(30)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = out_handle.join();
                let _ = err_handle.join();
                return Err(CaptureError::Io);
            }
        }
    };
    let stdout = out_handle.join().unwrap_or_default();
    let stderr = err_handle.join().unwrap_or_default();
    Ok(Captured {
        status,
        stdout,
        stderr,
    })
}

fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let Some(mut pipe) = pipe else {
        return Vec::new();
    };
    let mut buf = Vec::new();
    let _ = pipe.read_to_end(&mut buf);
    buf
}

/// The reap and a folder ffmpeg still holds can each block for a few hundred
/// milliseconds. Keep them off the async threads. Reaping first lets the
/// folder go on the first try.
async fn discard(child: Arc<Mutex<Child>>, dir: PathBuf) {
    let _ = tokio::task::spawn_blocking(move || {
        end_process(&child);
        remove_dir(&dir);
    })
    .await;
}

/// ffmpeg starts no children, so ending the process itself is enough.
fn end_process(child: &Mutex<Child>) {
    let mut child = lock_child(child);
    let _ = child.kill();
    let _ = child.wait();
}

fn remove_dir(path: &Path) {
    let _ = fs::remove_dir_all(path);
    if path.exists() {
        thread::sleep(Duration::from_millis(200));
        let _ = fs::remove_dir_all(path);
    }
}

/// How long a seek or quality change keeps the old session folder. A casting
/// TV, and the browser for a moment, still fetch the old playlist and
/// segments after the switch.
const RETIRE_GRACE: Duration = Duration::from_secs(30);

/// Removes a replaced session folder once `grace` has passed. Each folder
/// has its own timer, so quick restarts or a stop never leave one behind;
/// the job's current folder is still removed at once by `stop`, and the
/// whole HLS root is cleared when Portal shuts down.
fn retire_dir(dir: PathBuf, grace: Duration) {
    tokio::spawn(async move {
        tokio::time::sleep(grace).await;
        let _ = tokio::task::spawn_blocking(move || remove_dir(&dir)).await;
    });
}

fn failure_line(stderr: &str) -> String {
    let redacted = redact(stderr);
    last_line(&redacted)
}

fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

fn redact(text: &str) -> String {
    strip_urls_in_text(&strip_authorization(text))
}

/// Drops an Authorization value up to the end of its line, so a scheme word
/// such as `Basic` or `Bearer` does not leave the credential behind it.
fn strip_authorization(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let needle = "authorization:";
    let mut out = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        if lower[index..].starts_with(needle) {
            out.push_str(&text[index..index + needle.len()]);
            index += needle.len();
            while index < text.len() {
                let ch = text[index..].chars().next().unwrap();
                if ch == '\n' || ch == '\r' || !ch.is_whitespace() {
                    break;
                }
                out.push(ch);
                index += ch.len_utf8();
            }
            while index < text.len() {
                let ch = text[index..].chars().next().unwrap();
                if ch == '\n' || ch == '\r' {
                    break;
                }
                index += ch.len_utf8();
            }
            continue;
        }
        let ch = text[index..].chars().next().unwrap();
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

/// Cut every URL down to its scheme and host. Stream links can carry the
/// file's access in their path and query. Punctuation
/// that ends the URL, like the colon ffmpeg puts after it, is kept.
fn strip_urls_in_text(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(idx) = rest.find("://") {
        out.push_str(&rest[..idx + 3]);
        rest = &rest[idx + 3..];
        let end = rest
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '"' | '\'' | '<' | '>'))
            .unwrap_or(rest.len());
        let url = rest[..end].trim_end_matches([':', ',', '.', ';', ')', ']']);
        let tail = &rest[url.len()..end];
        let authority = &url[..url.find(['/', '?', '#']).unwrap_or(url.len())];
        let host = authority
            .rfind('@')
            .map_or(authority, |at| &authority[at + 1..]);
        out.push_str(host);
        out.push_str(tail);
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn rewrite_cue_line(line: &str, offset: f64) -> Option<String> {
    let (left, rest) = line.split_once("-->")?;
    let start = parse_timestamp(left.trim())?;
    let rest = rest.trim_start();
    let (end_token, suffix) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let end = parse_timestamp(end_token)?;
    let start = (start + offset).max(0.0);
    let end = (end + offset).max(start);
    let mut rewritten = format!("{} --> {}", format_timestamp(start), format_timestamp(end));
    if !suffix.is_empty() {
        rewritten.push(' ');
        rewritten.push_str(suffix.trim());
    }
    Some(rewritten)
}

fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (clock, fraction) = text.split_once(['.', ',']).unwrap_or((text, "0"));
    let parts: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [hours, minutes, seconds] => (*hours, *minutes, *seconds),
        [minutes, seconds] => ("0", *minutes, *seconds),
        _ => return None,
    };
    let hours: f64 = hours.parse().ok()?;
    let minutes: f64 = minutes.parse().ok()?;
    let seconds: f64 = seconds.parse().ok()?;
    if !(hours.is_finite() && minutes.is_finite() && seconds.is_finite()) {
        return None;
    }
    let mut millis = String::new();
    for ch in fraction.chars() {
        if ch.is_ascii_digit() {
            millis.push(ch);
        } else {
            break;
        }
    }
    while millis.len() < 3 {
        millis.push('0');
    }
    let millis: f64 = millis[..3].parse().unwrap_or(0.0);
    Some(hours * 3600.0 + minutes * 60.0 + seconds + millis / 1000.0)
}

fn format_timestamp(seconds: f64) -> String {
    let total = if !seconds.is_finite() || seconds <= 0.0 {
        0
    } else {
        (seconds * 1000.0).round() as u64
    };
    let millis = total % 1000;
    let whole = total / 1000;
    let secs = whole % 60;
    let minutes = (whole / 60) % 60;
    let hours = whole / 3600;
    format!("{hours:02}:{minutes:02}:{secs:02}.{millis:03}")
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// A play's id is also the path its video is served under, and a cast device
/// fetches that without signing in, so the id is random and cannot be guessed.
fn fresh_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::getrandom(&mut bytes).is_ok() {
        return bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let tick = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = nanos
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(tick.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        ^ (std::process::id() as u64).wrapping_shl(17);
    format!("{mixed:016x}")
}

#[cfg(test)]
#[path = "../../../tests-private/engine/src/playback.rs"]
mod tests;
