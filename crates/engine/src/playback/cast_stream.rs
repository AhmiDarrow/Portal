//! A whole-title stream for a TV.
//!
//! One playlist lists every piece of the title from its start, so the TV
//! shows the full length and can move anywhere in it. The pieces are made
//! when the TV asks for them: an encoder starts at the piece asked for and
//! runs on from there, and a jump to a part not yet made starts it again at
//! that part. A lone ask far from the encoder, such as the TV reading the
//! first piece while it resumes further on, gets a second encoder beside
//! the first, which stays: the TV takes up whichever one it reads on from.
//!
//! Every run keeps its own clock, starting at zero, so each run's pieces and
//! start file come out the same wherever it began. The title's own time is
//! written into a piece as it is served.

use super::*;

/// The playlist a TV is given.
pub const CAST_PLAYLIST: &str = "cast.m3u8";
/// The start file the playlist names, which every piece needs.
pub const CAST_INIT: &str = "castinit.mp4";
/// Short pieces: a TV that stalls on large ones plays these without a pause.
const PIECE_SECONDS: u32 = 3;
/// The most bits a second the picture may peak at, which keeps a piece small.
const CAST_MAXRATE: u64 = 8_000_000;
/// A piece this close ahead of the encoder is waited for. Further on, or
/// behind where it began, the encoder starts again at the piece.
const NEAR_PIECES: u32 = 5;
const PIECE_WAIT: Duration = Duration::from_secs(45);
const PIECE_POLL: Duration = Duration::from_millis(100);
/// A piece that takes this long to be ready is noted in the log.
const PIECE_SLOW: Duration = Duration::from_secs(1);
/// A last piece shorter than this is left out of the playlist.
const SLIVER: f64 = 0.5;
/// How much of ffmpeg's error output goes in the log, in characters.
const ERROR_TAIL: usize = 600;

pub(super) struct CastStream {
    session: String,
    dir: PathBuf,
    /// How many pieces the playlist lists.
    total: u32,
    /// The first piece of every encoder run so far.
    runs: Vec<u32>,
    /// The encoder the TV is reading from.
    run: Option<CastRun>,
    /// An encoder started for a piece far from `run`, kept apart until the
    /// TV reads on from it.
    side: Option<CastRun>,
    /// Each track's ticks a second, read from the start file.
    scales: Option<Vec<(u32, u32)>>,
}

struct CastRun {
    start: u32,
    /// Whether a graphics card encodes this run.
    hardware: bool,
    /// The first piece this run has not written yet.
    next: u32,
    child: Arc<Mutex<Child>>,
    /// The end of ffmpeg's error output, for the log when the run stops by itself.
    errors: Arc<Mutex<String>>,
}

impl CastRun {
    /// Moves `next` past the pieces now on disk.
    fn catch_up(&mut self, dir: &Path) {
        while piece_path(dir, self.start, self.next).is_file() {
            self.next += 1;
        }
    }

    /// Whether the TV reading `index` is reading from this run.
    fn covers(&self, index: u32) -> bool {
        self.start <= index && index < self.next + NEAR_PIECES
    }
}

impl CastStream {
    pub(super) fn session(&self) -> &str {
        &self.session
    }

    /// Ends the encoder and removes the files. Blocks, so call it off the
    /// async threads.
    pub(super) fn close(self) {
        for run in self.run.into_iter().chain(self.side) {
            end_process(&run.child);
        }
        remove_dir(&self.dir);
    }

    /// The run that made piece `index`, when the piece is on disk.
    fn made_by(&self, index: u32) -> Option<u32> {
        self.runs
            .iter()
            .rev()
            .copied()
            .filter(|start| *start <= index)
            .find(|start| piece_path(&self.dir, *start, index).is_file())
    }
}

enum Step {
    Have { path: PathBuf, start: u32 },
    Wait,
    /// A new encoder in place of the current one.
    Start,
    /// A new encoder beside the current one, which the TV still reads from.
    Side,
    Gone,
}

fn piece_path(dir: &Path, run: u32, index: u32) -> PathBuf {
    dir.join(format!("r{run}-{index:05}.m4s"))
}

fn init_path(dir: &Path, run: u32) -> PathBuf {
    dir.join(format!("init{run}.mp4"))
}

/// The piece number in a name like `c00042.m4s`.
pub(super) fn piece_index(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('c')?.strip_suffix(".m4s")?;
    (digits.len() == 5 && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Whether a media name belongs to a cast stream.
pub(super) fn cast_name(name: &str) -> bool {
    name == CAST_PLAYLIST || name == CAST_INIT || piece_index(name).is_some()
}

/// How many pieces a title of `duration` seconds has.
pub(super) fn piece_count(duration: f64) -> u32 {
    let piece = f64::from(PIECE_SECONDS);
    let whole = (duration / piece).floor();
    let rest = duration - whole * piece;
    let count = if rest >= SLIVER { whole + 1.0 } else { whole };
    count.clamp(1.0, 99_999.0) as u32
}

/// The piece that holds `seconds` of a title with `total` pieces.
pub(super) fn piece_at(seconds: f64, total: u32) -> u32 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return 0;
    }
    let index = (seconds / f64::from(PIECE_SECONDS)).floor();
    (index as u32).min(total.saturating_sub(1))
}

/// A finished playlist of every piece, made before any piece exists.
pub(super) fn cast_playlist(duration: f64) -> String {
    let total = piece_count(duration);
    let piece = f64::from(PIECE_SECONDS);
    let mut text = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-TARGETDURATION:{PIECE_SECONDS}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-INDEPENDENT-SEGMENTS\n#EXT-X-MAP:URI=\"{CAST_INIT}\"\n"
    );
    for index in 0..total {
        let left = duration - f64::from(index) * piece;
        let length = if index + 1 == total {
            left.clamp(SLIVER, piece)
        } else {
            piece
        };
        text.push_str(&format!("#EXTINF:{length:.6},\nc{index:05}.m4s\n"));
    }
    text.push_str("#EXT-X-ENDLIST\n");
    text
}

/// The picture for a TV: always encoded, so every piece starts on a
/// keyframe at its place in the playlist, and held to a rate a TV keeps up
/// with.
pub(super) fn cast_plan(stream: &StreamInfo, quality: Quality) -> VideoPlan {
    let plan = stream_plan(stream, quality);
    VideoPlan {
        copy: false,
        scale: plan.scale,
        maxrate: Some(plan.maxrate.map_or(CAST_MAXRATE, |rate| rate.min(CAST_MAXRATE))),
    }
}

/// An encode from piece `start` on. `until` ends it at a piece that is
/// already made.
pub(super) fn cast_args(
    source: &MediaSource,
    start: u32,
    until: Option<u32>,
    dir: &Path,
    zscale: bool,
    encoder: Encoder,
) -> Result<Vec<String>, EngineError> {
    let input = source.input.as_str();
    assert_safe_input(input)?;
    let Some(video) = source.video.as_ref() else {
        return Err(EngineError::Message("This title has no picture.".into()));
    };
    let refs = header_refs(&source.headers);
    let mut args = vec![
        "-hide_banner".into(),
        "-nostdin".into(),
        "-loglevel".into(),
        "error".into(),
    ];
    args.extend(input_args(input, &refs));
    args.extend([
        "-readrate".into(),
        READ_RATE.to_string(),
        "-readrate_initial_burst".into(),
        READ_BURST_SECONDS.to_string(),
    ]);
    args.extend(encoder::decode_args(encoder));
    if start > 0 {
        args.push("-ss".into());
        args.push((u64::from(start) * u64::from(PIECE_SECONDS)).to_string());
    }
    args.push("-i".into());
    args.push(input.to_string());
    args.push("-map".into());
    args.push(stream_map(video, "v"));
    let piece = f64::from(PIECE_SECONDS);
    // Where the run ends, in the source's time: at a piece already made, or
    // at the end of the title the playlist lists.
    let end = until
        .filter(|until| *until > start)
        .map(|until| f64::from(until) * piece)
        .into_iter()
        .chain(source.duration.filter(|length| length.is_finite()))
        .reduce(f64::min);
    let length = end
        .map(|end| end - f64::from(start) * piece)
        .filter(|length| *length > 0.0);
    let plan = cast_plan(video, source.quality);
    let mut picture = video_args_cut(
        &plan,
        &video.transfer,
        video.interlaced,
        zscale,
        PIECE_SECONDS,
        encoder,
    );
    // A file whose sound runs on past its picture would stop making pieces
    // where the picture ends, though the playlist lists them to the end.
    // Every later piece would then start ffmpeg again for one empty piece.
    // Holding the last frame and padding the sound, cut at the title's
    // length, makes every listed piece in the one run.
    let padded = length.is_some();
    if padded {
        if let Some(at) = picture.iter().position(|arg| arg == "-vf") {
            if let Some(filters) = picture.get_mut(at + 1) {
                filters.push_str(",tpad=stop_mode=clone:stop=-1");
            }
        }
    }
    args.extend(picture);
    // Without B-frames the picture starts with the sound, with no edit list
    // for the TV to honour or ignore.
    args.extend(["-bf".into(), "0".into()]);
    match source.audio.as_ref() {
        // Always stereo AAC: some TVs play other layouts as silence.
        Some(audio) => args.extend([
            "-map".into(),
            stream_map(audio, "a"),
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "192k".into(),
            "-ac".into(),
            "2".into(),
        ]),
        None => args.push("-an".into()),
    }
    if padded && source.audio.is_some() {
        args.extend(["-af".into(), "apad".into()]);
    }
    args.extend([
        "-sn".into(),
        "-dn".into(),
        "-max_muxing_queue_size".into(),
        "2048".into(),
    ]);
    if let Some(length) = length {
        args.push("-t".into());
        args.push(format!("{length:.3}"));
    }
    let folder = dir.display().to_string().replace('%', "%%");
    args.extend([
        "-f".into(),
        "hls".into(),
        "-hls_time".into(),
        PIECE_SECONDS.to_string(),
        "-hls_segment_type".into(),
        "fmp4".into(),
        "-hls_fmp4_init_filename".into(),
        format!("init{start}.mp4"),
        "-start_number".into(),
        start.to_string(),
        "-hls_list_size".into(),
        "0".into(),
        "-hls_playlist_type".into(),
        "vod".into(),
        "-hls_flags".into(),
        "independent_segments+temp_file".into(),
        "-hls_segment_filename".into(),
        format!("{folder}{}r{start}-%05d.m4s", std::path::MAIN_SEPARATOR),
        dir.join(format!("run{start}.m3u8")).display().to_string(),
    ]);
    Ok(args)
}

/// The boxes of an MP4 buffer at one level: (kind, body start, body end).
fn boxes(data: &[u8], from: usize, to: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut found = Vec::new();
    let mut at = from;
    while at + 8 <= to {
        let size = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize;
        let kind = [data[at + 4], data[at + 5], data[at + 6], data[at + 7]];
        let (head, size) = match size {
            0 => (8, to - at),
            1 if at + 16 <= to => {
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&data[at + 8..at + 16]);
                (16, u64::from_be_bytes(wide) as usize)
            }
            size => (8, size),
        };
        if size < head || at + size > to {
            break;
        }
        found.push((kind, at + head, at + size));
        at += size;
    }
    found
}

fn find_box(data: &[u8], from: usize, to: usize, kind: &[u8; 4]) -> Option<(usize, usize)> {
    boxes(data, from, to)
        .into_iter()
        .find(|(found, _, _)| found == kind)
        .map(|(_, start, end)| (start, end))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    let bytes = data.get(at..at + 4)?;
    Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Each track's id and ticks a second, from a start file.
pub(super) fn track_scales(init: &[u8]) -> Vec<(u32, u32)> {
    let mut scales = Vec::new();
    let Some((moov, moov_end)) = find_box(init, 0, init.len(), b"moov") else {
        return scales;
    };
    for (kind, start, end) in boxes(init, moov, moov_end) {
        if &kind != b"trak" {
            continue;
        }
        let id = find_box(init, start, end, b"tkhd").and_then(|(body, _)| {
            // Version 1 holds 64-bit times before the id.
            let skip = if init.get(body) == Some(&1) { 20 } else { 12 };
            be_u32(init, body + skip)
        });
        let scale = find_box(init, start, end, b"mdia")
            .and_then(|(mdia, mdia_end)| find_box(init, mdia, mdia_end, b"mdhd"))
            .and_then(|(body, _)| {
                let skip = if init.get(body) == Some(&1) { 20 } else { 12 };
                be_u32(init, body + skip)
            });
        if let (Some(id), Some(scale)) = (id, scale) {
            scales.push((id, scale));
        }
    }
    scales
}

/// Blanks every box of `kind` at this level, so a player skips it.
fn blank_boxes(data: &mut [u8], from: usize, to: usize, kind: &[u8; 4]) {
    for (found, start, _) in boxes(data, from, to) {
        if &found == kind {
            // The kind sits in the four bytes before the body of a plain box.
            if let Some(name) = data.get_mut(start - 4..start) {
                name.copy_from_slice(b"free");
            }
        }
    }
}

/// A start file with no edit lists. A run writes them from its own first
/// packets, and a TV that honours them would shift the picture or sound.
pub(super) fn plain_init(mut init: Vec<u8>) -> Vec<u8> {
    let len = init.len();
    if let Some((moov, moov_end)) = find_box(&init, 0, len, b"moov") {
        for (kind, start, end) in boxes(&init, moov, moov_end) {
            if &kind == b"trak" {
                blank_boxes(&mut init, start, end, b"edts");
            }
        }
    }
    init
}

/// Moves a piece from its run's clock to the title's: every track's start
/// time gains `seconds`. The piece's own index of itself is dropped, since
/// it names the old times.
pub(super) fn shift_piece(mut piece: Vec<u8>, scales: &[(u32, u32)], seconds: u64) -> Vec<u8> {
    let len = piece.len();
    blank_boxes(&mut piece, 0, len, b"sidx");
    if seconds == 0 {
        return piece;
    }
    for (kind, moof, moof_end) in boxes(&piece, 0, len) {
        if &kind != b"moof" {
            continue;
        }
        for (kind, traf, traf_end) in boxes(&piece, moof, moof_end) {
            if &kind != b"traf" {
                continue;
            }
            let track = find_box(&piece, traf, traf_end, b"tfhd")
                .and_then(|(body, _)| be_u32(&piece, body + 4));
            let scale = track.and_then(|track| {
                scales
                    .iter()
                    .find(|(id, _)| *id == track)
                    .map(|(_, scale)| *scale)
            });
            let (Some(scale), Some((body, end))) =
                (scale, find_box(&piece, traf, traf_end, b"tfdt"))
            else {
                continue;
            };
            let add = seconds * u64::from(scale);
            if piece.get(body) == Some(&1) && body + 12 <= end {
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&piece[body + 4..body + 12]);
                let moved = u64::from_be_bytes(wide).saturating_add(add);
                piece[body + 4..body + 12].copy_from_slice(&moved.to_be_bytes());
            } else if body + 8 <= end {
                let base = be_u32(&piece, body + 4).unwrap_or(0);
                let moved = u64::from(base).saturating_add(add).min(u64::from(u32::MAX)) as u32;
                piece[body + 4..body + 8].copy_from_slice(&moved.to_be_bytes());
            }
        }
    }
    piece
}

impl Player {
    /// The session of the job's whole-title stream, opened if it is not
    /// yet. None for a title with no picture or no known length, which has
    /// no such stream.
    pub fn cast_stream(&self, job_id: &str) -> Option<String> {
        let mut jobs = self.lock_jobs();
        let mut sessions = self.lock_sessions();
        let slot = jobs.get_mut(job_id)?;
        if let Some(cast) = &slot.cast {
            return Some(cast.session.clone());
        }
        let source = slot.source.as_ref()?;
        source.video.as_ref()?;
        let duration = source
            .duration
            .filter(|length| length.is_finite() && *length >= f64::from(PIECE_SECONDS))?;
        let session = fresh_id();
        let dir = self.inner.root.join(&session);
        fs::create_dir_all(&dir).ok()?;
        if fs::write(dir.join(CAST_PLAYLIST), cast_playlist(duration)).is_err() {
            remove_dir(&dir);
            return None;
        }
        sessions.insert(session.clone(), job_id.to_string());
        slot.cast = Some(CastStream {
            session: session.clone(),
            dir,
            total: piece_count(duration),
            runs: Vec::new(),
            run: None,
            side: None,
            scales: None,
        });
        Some(session)
    }

    /// The job's whole-title stream, when one is open.
    pub fn cast_session(&self, job_id: &str) -> Option<String> {
        self.lock_jobs()
            .get(job_id)
            .and_then(|slot| slot.cast.as_ref())
            .map(|cast| cast.session.clone())
    }

    /// Whether `session` is a whole-title stream.
    pub fn is_cast_session(&self, session: &str) -> bool {
        let job_id = self.lock_sessions().get(session).cloned();
        job_id.is_some_and(|job_id| {
            self.lock_jobs()
                .get(&job_id)
                .and_then(|slot| slot.cast.as_ref())
                .is_some_and(|cast| cast.session == session)
        })
    }

    /// Starts making pieces at `seconds`, before the TV asks for the first.
    pub fn cast_warm(&self, session: &str, seconds: f64) {
        let Some(job_id) = self.lock_sessions().get(session).cloned() else {
            return;
        };
        let total = self
            .lock_jobs()
            .get(&job_id)
            .and_then(|slot| slot.cast.as_ref())
            .map(|cast| cast.total);
        let Some(total) = total else {
            return;
        };
        let player = self.clone();
        let session = session.to_string();
        let index = piece_at(seconds, total);
        tokio::spawn(async move {
            let _ = player.cast_piece_on_disk(&session, index).await;
        });
    }

    /// Ends the encoder of the job's whole-title stream: no TV plays it
    /// now. The pieces stay, and the next request for one starts it again.
    pub fn cast_rest(&self, job_id: &str) {
        let runs: Vec<CastRun> = self
            .lock_jobs()
            .get_mut(job_id)
            .and_then(|slot| slot.cast.as_mut())
            .map(|cast| {
                cast.run
                    .take()
                    .into_iter()
                    .chain(cast.side.take())
                    .collect()
            })
            .unwrap_or_default();
        if !runs.is_empty() {
            tokio::task::spawn_blocking(move || {
                for run in runs {
                    end_process(&run.child);
                }
            });
        }
    }

    /// An embedded subtitle on the title's own clock, for a whole-title stream.
    pub fn cast_subtitle(&self, session: &str, id: u32) -> Option<String> {
        let job_id = self.lock_sessions().get(session).cloned()?;
        let jobs = self.lock_jobs();
        let slot = jobs.get(&job_id)?;
        slot.cast.as_ref().filter(|cast| cast.session == session)?;
        slot.subtitle_text
            .as_ref()?
            .iter()
            .find(|text| text.id == id)
            .map(|text| text.text.clone())
    }

    /// A file of a whole-title stream, ready for the TV: the start file, or
    /// a piece, which is made first if it does not exist.
    pub async fn cast_file(&self, session: &str, name: &str) -> Result<Vec<u8>, EngineError> {
        if !valid_session(session) {
            return Err(EngineError::NotFound);
        }
        if name == CAST_INIT {
            return self.cast_init(session).await;
        }
        let index = piece_index(name).ok_or(EngineError::NotFound)?;
        let asked = Instant::now();
        let (path, start) = self.cast_piece_on_disk(session, index).await?;
        let waited = asked.elapsed();
        if waited >= PIECE_SLOW {
            // The TV caught up with the encoder, or a jump started one.
            tracing::info!(
                session = %session,
                piece = index,
                seconds = format!("{:.1}", waited.as_secs_f64()),
                "The TV waited for a piece"
            );
        }
        let scales = self.cast_scales(session, start).await?;
        let seconds = u64::from(start) * u64::from(PIECE_SECONDS);
        let piece = tokio::fs::read(&path)
            .await
            .map_err(|_| EngineError::NotFound)?;
        self.touch_session(session);
        Ok(shift_piece(piece, &scales, seconds))
    }

    /// The TV fetched a piece: the job is being watched, and the browser's
    /// own stream of it can rest once no browser fetches that.
    fn touch_session(&self, session: &str) {
        let job_id = self.lock_sessions().get(session).cloned();
        if let Some(job_id) = job_id {
            if let Some(slot) = self.lock_jobs().get_mut(&job_id) {
                slot.last_touch = Instant::now();
                park(slot);
            }
        }
    }

    /// The start file of any run: they are all the same.
    async fn cast_init(&self, session: &str) -> Result<Vec<u8>, EngineError> {
        let deadline = Instant::now() + PIECE_WAIT;
        loop {
            let found = self.with_cast(session, |cast| {
                cast.runs
                    .iter()
                    .copied()
                    .find(|start| piece_path(&cast.dir, *start, *start).is_file())
                    .map(|start| init_path(&cast.dir, start))
            })?;
            if let Some(path) = found {
                let init = tokio::fs::read(&path)
                    .await
                    .map_err(|_| EngineError::NotFound)?;
                return Ok(plain_init(init));
            }
            // No run yet: the TV asked before Portal began one.
            let idle = self.with_cast(session, |cast| cast.runs.is_empty())?;
            if idle {
                self.cast_piece_on_disk(session, 0).await?;
                continue;
            }
            if Instant::now() >= deadline {
                return Err(EngineError::Message(SLOW_START.into()));
            }
            tokio::time::sleep(PIECE_POLL).await;
        }
    }

    async fn cast_scales(&self, session: &str, run: u32) -> Result<Vec<(u32, u32)>, EngineError> {
        let (known, path) =
            self.with_cast(session, |cast| (cast.scales.clone(), init_path(&cast.dir, run)))?;
        if let Some(scales) = known {
            return Ok(scales);
        }
        let init = tokio::fs::read(&path)
            .await
            .map_err(|_| EngineError::NotFound)?;
        let scales = track_scales(&init);
        if scales.is_empty() {
            return Err(EngineError::Message("The video for the TV could not be read.".into()));
        }
        let kept = scales.clone();
        self.with_cast(session, move |cast| cast.scales = Some(kept))?;
        Ok(scales)
    }

    fn with_cast<T>(
        &self,
        session: &str,
        read: impl FnOnce(&mut CastStream) -> T,
    ) -> Result<T, EngineError> {
        let job_id = self
            .lock_sessions()
            .get(session)
            .cloned()
            .ok_or(EngineError::NotFound)?;
        let mut jobs = self.lock_jobs();
        let cast = jobs
            .get_mut(&job_id)
            .and_then(|slot| slot.cast.as_mut())
            .filter(|cast| cast.session == session)
            .ok_or(EngineError::NotFound)?;
        Ok(read(cast))
    }

    /// What a request for piece `index` does, and the runs it no longer
    /// needs, which the caller ends.
    fn cast_step(&self, session: &str, index: u32) -> Result<(Step, Vec<CastRun>), EngineError> {
        self.with_cast(session, |cast| {
            if index >= cast.total {
                return (Step::Gone, Vec::new());
            }
            let dir = cast.dir.clone();
            for run in cast.run.iter_mut().chain(cast.side.iter_mut()) {
                run.catch_up(&dir);
            }
            let mut done = Vec::new();
            let side_followed = cast
                .side
                .as_ref()
                .is_some_and(|side| side.covers(index) && index > side.start);
            if side_followed {
                // The TV reads on from the side run: it is the one in use now.
                done.extend(cast.run.take());
                cast.run = cast.side.take();
            } else if cast.run.as_ref().is_some_and(|run| run.covers(index))
                && cast
                    .side
                    .as_ref()
                    .is_some_and(|side| side.next > side.start)
            {
                // The TV took its lone piece and went back to the main run.
                done.extend(cast.side.take());
            }
            if let Some(start) = cast.made_by(index) {
                let path = piece_path(&cast.dir, start, index);
                return (Step::Have { path, start }, done);
            }
            let waits = cast
                .run
                .iter()
                .chain(cast.side.iter())
                .any(|run| run.covers(index) && !child_exited(&run.child));
            if waits {
                return (Step::Wait, done);
            }
            // A main run still encoding stays for the TV, which is likely to
            // come back to it after this one piece.
            let busy = cast
                .run
                .as_ref()
                .is_some_and(|run| !child_exited(&run.child));
            (if busy { Step::Side } else { Step::Start }, done)
        })
    }

    /// Ends runs off the async threads, noting any that stopped by themselves.
    fn retire(&self, job_id: &str, runs: Vec<CastRun>) {
        if runs.is_empty() {
            return;
        }
        for run in &runs {
            self.note_run_end(job_id, run);
        }
        tokio::task::spawn_blocking(move || {
            for run in runs {
                end_process(&run.child);
            }
        });
    }

    /// A run that stopped before Portal ended it: logs why, and marks a
    /// graphics card that could not make even one piece.
    fn note_run_end(&self, job_id: &str, run: &CastRun) {
        let status = lock_child(&run.child).try_wait().ok().flatten();
        let Some(status) = status else {
            return;
        };
        if status.success() {
            tracing::info!(job = %job_id, from = run.start, to = run.next, "The TV encode ran to its end");
            return;
        }
        if run.hardware && run.next == run.start {
            self.hardware_failed();
        }
        let errors = run
            .errors
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone();
        let errors = errors.trim();
        let tail = errors
            .char_indices()
            .rev()
            .nth(ERROR_TAIL)
            .map_or(errors, |(at, _)| &errors[at..]);
        tracing::warn!(
            job = %job_id,
            from = run.start,
            to = run.next,
            code = status.code().unwrap_or(-1),
            errors = tail,
            "The TV encode stopped early"
        );
    }

    /// The piece's file and the run that made it, once it is on disk.
    async fn cast_piece_on_disk(
        &self,
        session: &str,
        index: u32,
    ) -> Result<(PathBuf, u32), EngineError> {
        let job_id = self
            .lock_sessions()
            .get(session)
            .cloned()
            .ok_or(EngineError::NotFound)?;
        let deadline = Instant::now() + PIECE_WAIT;
        let mut starts = 0;
        loop {
            let (step, done) = self.cast_step(session, index)?;
            self.retire(&job_id, done);
            match step {
                Step::Have { path, start } => return Ok((path, start)),
                Step::Gone => return Err(EngineError::NotFound),
                Step::Wait => {}
                Step::Start | Step::Side => {
                    // One request at a time decides, so two never start an
                    // encoder each.
                    let _gate = self.inner.cast_gate.lock().await;
                    let (step, done) = self.cast_step(session, index)?;
                    self.retire(&job_id, done);
                    let side = match step {
                        Step::Start => Some(false),
                        Step::Side => Some(true),
                        _ => None,
                    };
                    if let Some(side) = side {
                        if starts >= 2 {
                            return Err(EngineError::Message(
                                "Portal could not prepare that part of the video for the TV."
                                    .into(),
                            ));
                        }
                        starts += 1;
                        self.start_cast_run(session, index, side).await?;
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err(EngineError::Message(SLOW_START.into()));
            }
            tokio::time::sleep(PIECE_POLL).await;
        }
    }

    /// Starts an encoder at piece `start`: in place of the current runs, or,
    /// for a `side` run, beside the main one, which keeps going.
    async fn start_cast_run(
        &self,
        session: &str,
        start: u32,
        side: bool,
    ) -> Result<(), EngineError> {
        let job_id = self
            .lock_sessions()
            .get(session)
            .cloned()
            .ok_or(EngineError::NotFound)?;
        let (source, dir, total, runs, old) = {
            let mut jobs = self.lock_jobs();
            let slot = jobs.get_mut(&job_id).ok_or(EngineError::NotFound)?;
            let source = slot.source.clone().ok_or(EngineError::NotFound)?;
            // The TV has its own stream now, so the browser's stops here
            // until a browser asks for it again.
            park(slot);
            let cast = slot
                .cast
                .as_mut()
                .filter(|cast| cast.session == session)
                .ok_or(EngineError::NotFound)?;
            let mut old: Vec<CastRun> = cast.side.take().into_iter().collect();
            if !side {
                old.extend(cast.run.take());
            }
            for run in &mut old {
                run.catch_up(&cast.dir);
            }
            (source, cast.dir.clone(), cast.total, cast.runs.clone(), old)
        };
        for run in &old {
            self.note_run_end(&job_id, run);
        }
        room_to_encode(&dir)?;
        let encoder = self.encoder().await;
        let hdr = source
            .video
            .as_ref()
            .is_some_and(|stream| is_hdr(&stream.transfer));
        let zscale = hdr && self.zscale_available().await;
        let ffmpeg = self.inner.ffmpeg.clone();
        let spawn_dir = dir.clone();
        let (child, errors) = tokio::task::spawn_blocking(move || {
            for run in old {
                end_process(&run.child);
            }
            // The run stops where an earlier one already made the pieces.
            let until = (start + 1..total).find(|index| {
                runs.iter()
                    .any(|run| *run <= *index && piece_path(&spawn_dir, *run, *index).is_file())
            });
            let args = cast_args(&source, start, until, &spawn_dir, zscale, encoder)?;
            // ffmpeg writes the start file where it runs, not beside the pieces.
            let mut child = spawn_ffmpeg_in(&ffmpeg, &args, Some(&spawn_dir))
                .map_err(|_| EngineError::Message("Transcoding failed to start".into()))?;
            // Kept for the log; a full pipe would stop the encode.
            let errors = drain_stderr(&mut child);
            Ok::<_, EngineError>((child, errors))
        })
        .await
        .map_err(|_| EngineError::Message("Transcoding failed to start".into()))??;
        let child = Arc::new(Mutex::new(child));
        tracing::info!(job = %job_id, piece = start, side, encoder = encoder.name(), "Encoding for the TV");
        let kept = self.with_cast(session, |cast| {
            if !cast.runs.contains(&start) {
                cast.runs.push(start);
            }
            let run = CastRun {
                start,
                hardware: encoder.is_hardware(),
                next: start,
                child: child.clone(),
                errors,
            };
            if side {
                cast.side.replace(run)
            } else {
                cast.run.replace(run)
            }
        });
        match kept {
            // A run already in the slot is ended rather than lost.
            Ok(replaced) => {
                self.retire(&job_id, replaced.into_iter().collect());
                Ok(())
            }
            Err(err) => {
                // The stream closed while the encoder started.
                let _ = tokio::task::spawn_blocking(move || end_process(&child)).await;
                Err(err)
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests-private/engine/src/playback/cast_stream.rs"]
mod tests;
