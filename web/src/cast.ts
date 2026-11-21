import {
  ApiError,
  api,
  audioTracksOf,
  type AudioTrack,
  type CastSession,
  type NextItem,
  type PlayJob,
  type PlayKind,
  type MediaInfo,
  type Quality,
  type SubtitleCandidate,
  type SubtitleSettings,
  type SubtitleTrack,
} from "./api";
import { clampSkip, formatClock, seekValueText } from "./player-time";
import {
  type HlsInstance,
  attachHls,
  dropFocusAfterClick,
  hlsReady,
  loadHls,
  onScrubRelease,
  onVideoClick,
  paintBuffered,
  paintSeekText,
  placeCues as placeStageCues,
  playedSpan,
  removeTracks,
  wakeStage,
} from "./stage";
import { clampVolume, isQuality, onPrefs, prefs as sharedPrefs, setPrefs as setShared, syncPrefs } from "./prefs";
import { mountThemes } from "./themes";
import { cleanEpisodeTitle, episodeCode, splitTitle } from "./titles";
import {
  candidateTitle,
  cleanResults,
  onlineTrackUrl,
  preferredLanguages,
  preferredTrack,
  autoSubtitle,
  sameLanguage,
  searchFor,
  startupAudioLanguage,
  subtitleSettings,
  trackLanguage,
  trackName,
} from "./subtitles";

type Replay = {
  kind: PlayKind;
  id: string;
  file: string;
  title: string;
  season?: number;
  episode?: number;
  art?: string | null;
  showTitle?: string | null;
  showId?: string | null;
};

type Prefs = {
  quality: Quality;
  /** "off" or "auto" (on); a track picked here is this item's `subPick`. */
  subtitles: "off" | "auto";
  /** The user chose; before this, a saved "auto" was only the old default. */
  subtitlesSet: boolean;
  /** The language of the last subtitle picked, preferred next time. */
  subtitleLanguage: string | null;
  /** The language of the last audio track picked, preferred next time. */
  audioLanguage: string | null;
  autoplay: boolean;
  volume: number;
};

type Nodes = {
  root: HTMLElement;
  stage: HTMLElement;
  video: HTMLVideoElement;
  presence: HTMLElement;
  note: HTMLElement;
  title: HTMLElement;
  episode: HTMLElement;
  stageTitle: HTMLElement;
  stageStatus: HTMLElement;
  bigPlay: HTMLButtonElement;
  status: HTMLElement;
  play: HTMLButtonElement;
  back: HTMLButtonElement;
  forward: HTMLButtonElement;
  mute: HTMLButtonElement;
  elapsed: HTMLElement;
  total: HTMLElement;
  bar: HTMLInputElement;
  hover: HTMLElement;
  scrub: HTMLElement;
  chrome: HTMLElement;
  full: HTMLButtonElement;
  quality: HTMLSelectElement;
  qualityLabel: HTMLElement;
  audio: HTMLSelectElement;
  audioLabel: HTMLElement;
  subs: HTMLSelectElement;
  cc: HTMLButtonElement;
  links: HTMLButtonElement;
  next: HTMLButtonElement;
  auto: HTMLInputElement;
  volume: HTMLInputElement;
  close: HTMLButtonElement;
};

type Pin = { jobId: string; baseline: string };

type SessionView = {
  receiver_id: string;
  job_id: string;
  title: string;
  replay: Replay | null;
  /** A cast Portal ended because its TV went quiet, and where that TV last was. */
  lost: { receiver_id: string; position: number | null } | null;
};

const RECEIVER_KEY = "portal-cast-receiver";

const SHELL = `
  <p class="cast-banner">Open this page on your TV</p>
  <footer class="dock open cast-dock">
    <div class="stage player-stage is-idle" id="cast-stage">
      <video id="cast-video" playsinline></video>
      <div id="cast-empty" class="cast-empty">
        <p class="cast-wait">Waiting for Portal to cast…</p>
        <p id="cast-presence" class="cast-presence"></p>
        <p id="cast-note" class="cast-note" aria-hidden="true"></p>
      </div>
      <div class="stage-top" aria-hidden="true">
        <span id="cast-stage-title"></span>
      </div>
      <div class="stage-center">
        <div class="stage-spinner" aria-hidden="true"></div>
        <button type="button" class="stage-play" id="cast-big-play" aria-label="Play" tabindex="-1">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l10.5-6.5z" /></svg>
        </button>
        <p class="stage-status" id="cast-stage-status" aria-hidden="true"></p>
      </div>
      <div class="chrome player-chrome" id="cast-chrome">
        <div class="scrub" id="cast-scrub">
          <input id="cast-bar" class="seek-bar" type="range" min="0" max="0" step="1" value="0" aria-label="Time" disabled />
          <span id="cast-hover" class="scrub-tip" hidden></span>
        </div>
        <div class="chrome-row">
          <button type="button" id="cast-play" class="icon-btn" aria-label="Play" title="Play (k)" disabled>
            <svg class="when-paused" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l10.5-6.5z" /></svg>
            <svg class="when-playing" viewBox="0 0 24 24" aria-hidden="true"><path d="M7.5 5.5h3v13h-3zM13.5 5.5h3v13h-3z" /></svg>
          </button>
          <button type="button" id="cast-back" class="icon-btn" aria-label="Back 10 seconds" title="Back 10 seconds (j)" disabled>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M4.5 12a7.5 7.5 0 1 0 2.2-5.3M4.5 4.5v3.7h3.7" /><text x="12" y="15.6">10</text></svg>
          </button>
          <button type="button" id="cast-forward" class="icon-btn" aria-label="Forward 10 seconds" title="Forward 10 seconds (l)" disabled>
            <svg viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v3.7h-3.7" /><text x="12" y="15.6">10</text></svg>
          </button>
          <div class="volume">
            <button type="button" id="cast-mute" class="icon-btn" aria-label="Mute" title="Mute (m)">
              <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9.5h3.5L12 5.5v13l-4.5-4H4z" /><path class="line when-sound" d="M15.5 9a4 4 0 0 1 0 6M18 6.5a7.5 7.5 0 0 1 0 11" /><path class="line when-muted" d="M16 9.5l5 5M21 9.5l-5 5" /></svg>
            </button>
            <input id="cast-volume" class="volume-bar" type="range" min="0" max="1" step="0.05" value="1" aria-label="Volume, 100%" aria-valuetext="100%" />
          </div>
          <span class="clock"><span id="cast-elapsed" class="clock-now">0:00</span><span class="clock-sep" aria-hidden="true">/</span><span id="cast-total">0:00</span></span>
          <span class="chrome-gap"></span>
          <button type="button" id="cast-cc" class="icon-btn" aria-pressed="false" aria-label="Subtitles off" title="Subtitles off (c)" disabled>
            <svg viewBox="0 0 24 24" aria-hidden="true"><rect class="line" x="3" y="5.5" width="18" height="13" rx="2" /><path class="line" d="M10.5 10.3a2.2 2.2 0 1 0 0 3.4M17 10.3a2.2 2.2 0 1 0 0 3.4" /></svg>
          </button>
          <button type="button" id="cast-full" class="icon-btn" aria-label="Fullscreen" title="Fullscreen (f)" disabled>
            <svg class="when-windowed" viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M4 9V4h5M15 4h5v5M20 15v5h-5M9 20H4v-5" /></svg>
            <svg class="when-full" viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M9 4v5H4M20 9h-5V4M15 20v-5h5M4 15h5v5" /></svg>
          </button>
        </div>
      </div>
    </div>
    <div class="dock-side">
      <div class="dock-head">
        <div class="dock-titles">
          <h2 id="cast-title"></h2>
          <p id="cast-episode" hidden></p>
        </div>
      </div>
      <p id="cast-status" aria-live="polite"></p>
      <div class="dock-controls">
        <button type="button" id="cast-next" class="primary" disabled title="Next episode">Next episode</button>
        <button type="button" id="cast-links" disabled title="Links unavailable">Choose source</button>
        <button type="button" id="cast-close" title="Stop casting">Stop casting</button>
      </div>
      <div class="dock-controls dock-prefs">
        <label class="dock-field" id="cast-quality-label" title="Change quality from the controller">Quality
          <select id="cast-quality" disabled title="Change quality from the controller">
            <option value="auto">Auto</option>
            <option value="source">Source</option>
            <option value="2160">4K</option>
            <option value="1080">1080p</option>
            <option value="720">720p</option>
            <option value="480">480p</option>
          </select>
        </label>
        <label class="dock-field" id="cast-audio-label" title="Audio">Audio
          <select id="cast-audio" disabled title="Audio">
            <option value="">Audio</option>
          </select>
        </label>
        <label class="dock-field">Subtitles
          <select id="cast-subs" aria-label="Subtitles, no tracks" title="No subtitles" disabled>
            <option value="auto">Auto</option>
            <option value="off">Off</option>
          </select>
        </label>
        <label class="dock-field check"><input type="checkbox" id="cast-auto" checked /> Autoplay next</label>
      </div>
    </div>
  </footer>
`;

let prefs = loadPrefs();
let prefsOff: (() => void) | null = null;
let nodes: Nodes | null = null;
let active = false;
let presenceTimer = 0;
let sessionTimer = 0;
let presenceBusy = false;
let sessionBusy = false;
let holdSession = false;
let starting = false;
let closing = false;
let followToken = 0;
let skipGeneration = 0;
let watchingId: string | null = null;
let currentJob: string | null = null;
/** Where the stream stopped after it ran out of retries, and that stream's playlist; Play restarts there. */
let stalledAt: number | null = null;
let stalledOn = "";
let currentNext: NextItem | null = null;
let currentReplay: Replay | null = null;
let sessionReplay: Replay | null = null;
let pin: Pin | null = null;
let currentSubs: SubtitleTrack[] = [];
let hls: HlsInstance | null = null;
let loadedPlaylist = "";
/** The job is still resolving (opening, waiting for the controller, restarting). */
let waiting = false;
/** A stream is attached, and whether a frame has played since. */
let attached = false;
let started = false;
let buffering = false;
/** Re-applies the subtitle choice once the next stream has data. */
let pendingSubtitleApply: (() => void) | null = null;
/** The last job poll failed; the next good one clears its message. */
let pollFailed = false;
let origin = 0;
let jobDuration = 0;
let dragging = false;
let hoverX: number | null = null;
let skipping = false;
/** Progress and place posts, and the dashboard command already applied. */
let lastProgressPost = 0;
let lastPlacePost = 0;
let appliedCommand = 0;
let commandJob = "";
type RemoteCommand = { seq: number; action: string; seconds: number | null };
let queuedCommand: RemoteCommand | null = null;
let fillingSubs = false;
/** Tracks offered for the playing file, and the one ffmpeg is mapping. */
let currentAudio: AudioTrack[] = [];
let currentAudioIndex: number | null = null;
let audioListKey = "";
let fillingAudio = false;
/** A pick is waiting out the arrow-key settle, so a repaint must not put the old track back. */
let audioPending = false;
let audioTimer = 0;
const AUDIO_SETTLE = 600;
/** Where the server seek in flight lands, and where further ±10 presses meanwhile add up to. */
let skipTarget = 0;
let queuedSkip: number | null = null;
/** When the session poll last read the shared choices, so a dashboard change reaches this TV. */
let lastPrefsSync = 0;
const PREFS_SYNC_MS = 10_000;
/** A save from this page is under way; its echo is not a change from elsewhere. */
let savingPrefs = false;
let abort: AbortController | null = null;

const receiver = createReceiver();

function createReceiver(): { id: string; name: string } {
  const id = loadReceiverId();
  const short = id.replace(/-/g, "").slice(0, 4).toUpperCase();
  return { id, name: short ? "TV · " + short : "TV" };
}

function loadReceiverId(): string {
  try {
    const stored = sessionStorage.getItem(RECEIVER_KEY);
    if (stored && /^[A-Za-z0-9-]{8,64}$/.test(stored)) return stored;
  } catch {
    /* Private mode can reject sessionStorage. The id still lasts for this view. */
  }
  const created =
    typeof crypto.randomUUID === "function"
      ? crypto.randomUUID()
      : "tv-" + Math.random().toString(16).slice(2, 10) + Date.now().toString(16);
  try {
    sessionStorage.setItem(RECEIVER_KEY, created);
  } catch {
    /* Keep the in-memory id. */
  }
  return created;
}

/**
 * The choices every screen shares (quality, subtitles on or off and the
 * language last picked, the audio language last picked, Autoplay next) plus
 * this browser's volume. A subtitle track picked here is kept for this item
 * only; the next one goes by its language.
 */
function loadPrefs(): Prefs {
  const shared = sharedPrefs();
  return {
    quality: shared.quality,
    subtitles: shared.subtitles,
    subtitlesSet: shared.subtitlesSet,
    subtitleLanguage: shared.subtitleLanguage,
    audioLanguage: shared.audioLanguage,
    autoplay: shared.autoplay,
    volume: shared.volume,
  };
}

/** `language` is saved only when given: a pick names it, Auto clears it. `audio` likewise, and only for a tagged track. */
function savePrefs(language?: string | null, audio?: string | null): void {
  savingPrefs = true;
  try {
    setShared({
      quality: prefs.quality,
      subtitles: prefs.subtitles,
      subtitlesSet: prefs.subtitlesSet,
      ...(language !== undefined ? { subtitleLanguage: language } : {}),
      ...(audio !== undefined ? { audioLanguage: audio } : {}),
      autoplay: prefs.autoplay,
      volume: prefs.volume,
    });
  } finally {
    savingPrefs = false;
  }
  prefs.subtitleLanguage = sharedPrefs().subtitleLanguage;
  prefs.audioLanguage = sharedPrefs().audioLanguage;
}

/** Subtitles on or off and the language, as one value to compare. */
function subtitlePrefsKey(): string {
  return prefs.subtitles + "|" + (prefs.subtitleLanguage ?? "").toLowerCase();
}

function applyPrefs(): void {
  if (!nodes) return;
  nodes.quality.value = prefs.quality;
  nodes.auto.checked = prefs.autoplay;
  nodes.volume.value = String(prefs.volume);
  nodes.video.volume = prefs.volume;
  paintVolume();
}

function must<T extends HTMLElement>(root: ParentNode, id: string): T {
  const node = root.querySelector("#" + id);
  if (!(node instanceof HTMLElement)) throw new Error("Missing " + id);
  return node as T;
}

function message(err: unknown): string {
  if (err instanceof TypeError) return "Portal is offline.";
  if (err instanceof Error && err.message.trim()) return err.message;
  return "Portal is offline.";
}

function isMissing(err: unknown): boolean {
  return err instanceof ApiError && err.status === 404;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

function setStatus(text: string): void {
  if (!nodes) return;
  nodes.status.textContent = text;
  nodes.stageStatus.textContent = text;
  nodes.note.textContent = text;
  paintStage();
}

/** The job is resolving: the stage spins and shows `text`. */
function setWaiting(text: string): void {
  waiting = true;
  setStatus(text);
}

/** A failure or a choice for the controller: the message alone, no spinner. */
function setMessage(text: string): void {
  waiting = false;
  setStatus(text);
}

function placeCues(force = false): void {
  if (nodes) placeStageCues(nodes.stage, nodes.video, force);
}

/** Shows the controls; in fullscreen they fade again after a pause in activity. */
function wake(): void {
  if (nodes) wakeStage(nodes.stage, nodes.video, () => dragging);
}

/** Spinner while resolving, loading or buffering; a play button while paused. */
function paintStage(): void {
  if (!nodes) return;
  const { stage, video: media } = nodes;
  const following = !!currentJob;
  const busy = following && (skipping || (waiting && !loadedPlaylist) || (attached && !started) || buffering);
  const paused = following && started && !busy && (media.paused || media.ended);
  const text = nodes.stageStatus.textContent?.trim() ?? "";
  stage.classList.toggle("is-busy", busy);
  stage.classList.toggle("is-paused", paused);
  stage.classList.toggle("has-message", following && !started && !!text);
  stage.classList.toggle("has-note", following && started && !!subNote && text === subNote);
  stage.classList.toggle("is-idle", !following);
  placeCues();
}

function setTitle(title: string): void {
  if (!nodes) return;
  const season = currentReplay?.season;
  const episode = currentReplay?.episode;
  const split = splitTitle(title, season, episode, currentReplay?.showTitle);
  const heading = split.heading;
  nodes.title.textContent = heading;
  nodes.episode.textContent = split.detail;
  nodes.episode.hidden = !split.detail;
  const full = split.detail ? heading + " · " + split.detail : heading;
  nodes.stageTitle.textContent = full;
  if (full) nodes.title.title = full;
  else nodes.title.removeAttribute("title");
}

function isKind(value: unknown): value is PlayKind {
  return value === "movie" || value === "episode" || value === "song" || value === "file";
}

function num(value: unknown): number | undefined {
  return typeof value === "number" && Number.isFinite(value) ? value : undefined;
}

/** A catalog play target. PlayJob.id is the job id, so a payload with `state` is not replayable. */
function replayFromRecord(row: Record<string, unknown>): Replay | null {
  const file = typeof row.file === "string" ? row.file.trim() : "";
  const kind = row.kind;
  if (!file || !isKind(kind)) return null;
  const mediaId = typeof row.media_id === "string" ? row.media_id.trim() : "";
  const titleId = typeof row.title_id === "string" ? row.title_id.trim() : "";
  let id = mediaId || titleId;
  if (!id && typeof row.id === "string" && typeof row.state !== "string") id = row.id.trim();
  if (!id) return null;
  const showId = typeof row.show_id === "string" ? row.show_id.trim() : "";
  const title = typeof row.title === "string" && row.title.trim() ? row.title.trim() : "Playback";
  const episode = kind === "episode";
  const catalogId = episode && showId ? showId : id;
  return {
    kind,
    id: catalogId,
    file,
    title,
    season: num(row.season),
    episode: num(row.episode),
    art: typeof row.art === "string" ? row.art : null,
    showTitle: typeof row.show_title === "string" ? row.show_title : null,
    showId: episode ? showId || catalogId : undefined,
  };
}

function replayFromJob(job: PlayJob): Replay | null {
  return replayFromRecord(job as PlayJob & Record<string, unknown>);
}

function replayFromNext(next: NextItem): Replay {
  const episode = next.kind === "episode";
  const showId = next.show_id || next.id;
  return {
    kind: next.kind,
    id: episode ? showId : next.id,
    file: next.file,
    title: next.title,
    season: next.season ?? undefined,
    episode: next.episode ?? undefined,
    art: next.art ?? null,
    showTitle: next.show_title ?? null,
    showId: episode ? showId : undefined,
  };
}

function replayBody(target: Replay, manualLinks: boolean, resume: number): Record<string, unknown> {
  const body: Record<string, unknown> = {
    kind: target.kind,
    id: target.id,
    file: target.file,
    title: target.title,
    quality: prefs.quality,
  };
  if (target.season != null) body.season = target.season;
  if (target.episode) body.episode = target.episode;
  if (target.art) body.art = target.art;
  if (target.showTitle) body.show_title = target.showTitle;
  if (target.showId) body.show_id = target.showId;
  if (resume > 60) body.resume_seconds = Math.floor(resume);
  if (manualLinks) body.manual_links = true;
  const audioLanguage = startupAudioLanguage(prefs.audioLanguage);
  if (audioLanguage) body.audio_language = audioLanguage;
  return body;
}

function readSession(raw: CastSession): SessionView {
  const bag = raw as CastSession & Record<string, unknown>;
  const nested = bag.session && typeof bag.session === "object" ? bag.session : null;
  const body = (nested ?? bag) as CastSession & Record<string, unknown>;
  const receiverId = typeof body.receiver_id === "string" ? body.receiver_id.trim() : "";
  const jobId = typeof body.job_id === "string" ? body.job_id.trim() : "";
  const title = typeof body.title === "string" ? body.title.trim() : "";
  const nestedReplay = nested ? replayFromRecord(nested as Record<string, unknown>) : null;
  const lost = bag.lost && typeof bag.lost === "object" ? bag.lost : null;
  const lostId = typeof lost?.receiver_id === "string" ? lost.receiver_id.trim() : "";
  const lostAt = lost?.place?.position;
  return {
    receiver_id: receiverId,
    job_id: jobId,
    title,
    replay: nestedReplay ?? replayFromRecord(bag),
    lost: lostId
      ? { receiver_id: lostId, position: typeof lostAt === "number" && Number.isFinite(lostAt) && lostAt >= 0 ? lostAt : null }
      : null,
  };
}

/** The stream gave up and is waiting for Play. */
function stalled(): boolean {
  return !loadedPlaylist && stalledAt != null;
}

function sourcePosition(): number {
  // Nothing plays while the stream waits for Play: the place is where it stopped.
  if (stalled() && stalledAt != null) return stalledAt;
  const current = nodes?.video.currentTime ?? 0;
  const base = Number.isFinite(current) ? current : 0;
  return origin + base;
}

function progressDuration(): number {
  if (jobDuration > 0 && Number.isFinite(jobDuration)) return jobDuration;
  const span = nodes ? playedSpan(nodes.video) : 0;
  if (span > 0) return span + origin;
  return 0;
}

function progressBody(): Record<string, unknown> | null {
  const target = currentReplay;
  if (!target || (target.kind !== "movie" && target.kind !== "episode")) return null;
  // Nothing loaded and no place known (opening, or between streams): saving now would reset it.
  if (!loadedPlaylist && !stalled()) return null;
  const position = sourcePosition();
  const duration = progressDuration();
  if (!Number.isFinite(duration) || duration <= 0) return null;
  if (!Number.isFinite(position) || position < 0) return null;
  const episode = target.kind === "episode";
  const showId = target.showId || target.id;
  const name = (episode ? target.showTitle || target.title : target.title).trim();
  if (!name || name.startsWith("{")) return null;
  const body: Record<string, unknown> = {
    id: episode ? showId : target.id,
    kind: episode ? "tv" : "movie",
    name,
    file: target.file,
    position,
    duration,
  };
  if (target.art) body.art = target.art;
  if (episode) {
    if (target.season != null) body.season = target.season;
    if (target.episode) body.episode = target.episode;
    body.episode_name = target.title;
  }
  return body;
}

/** Saves where this TV is. A browser cast's dashboard does not write progress. */
async function postProgress(force: boolean, keepalive = false): Promise<void> {
  if (skipping) return;
  const now = Date.now();
  if (!force && now - lastProgressPost < 5000) return;
  const body = progressBody();
  if (!body) return;
  // Pause, stop, and leaving the page write the library now. The ticker does not.
  if (force) body.flush = true;
  lastProgressPost = now;
  // A missed save is made up by the next one; it must not end as an unhandled rejection.
  await api.progress(body, keepalive).catch(() => undefined);
}

/** Tells the dashboard where playback is, so its clock and play button follow. */
function postPlace(force: boolean): void {
  if (!currentJob || skipping || !loadedPlaylist) return;
  const now = Date.now();
  if (!force && now - lastPlacePost < 1000) return;
  const position = sourcePosition();
  const duration = progressDuration();
  if (!Number.isFinite(position) || position < 0) return;
  const media = nodes?.video;
  const paused = !media || media.paused || media.ended;
  lastPlacePost = now;
  void api
    .castPlace({
      position,
      duration: Number.isFinite(duration) && duration > 0 ? duration : 0,
      paused,
    })
    .catch(() => undefined);
}

function readCommand(raw: CastSession): RemoteCommand | null {
  const nested = raw.session && typeof raw.session === "object" ? raw.session : raw;
  const command = nested.command;
  if (!command || !Number.isFinite(command.seq) || command.seq <= 0) return null;
  const action = command.action;
  if (action !== "play" && action !== "pause" && action !== "seek") return null;
  const seconds = typeof command.seconds === "number" && Number.isFinite(command.seconds) ? command.seconds : null;
  return { seq: command.seq, action, seconds };
}

function noteCommandJob(jobId: string): void {
  if (jobId === commandJob) return;
  commandJob = jobId;
  appliedCommand = 0;
  queuedCommand = null;
}

function acceptCommand(raw: CastSession, jobId: string): void {
  noteCommandJob(jobId);
  if (!jobId) return;
  const command = readCommand(raw);
  if (!command || command.seq <= appliedCommand) return;
  appliedCommand = command.seq;
  if (skipping || starting) {
    queuedCommand = command;
    return;
  }
  void performCommand(command);
}

function flushQueuedCommand(): void {
  const queued = queuedCommand;
  queuedCommand = null;
  if (queued) void performCommand(queued);
}

async function performCommand(command: RemoteCommand): Promise<void> {
  if (command.action === "pause") {
    nodes?.video.pause();
    postPlace(true);
    void postProgress(true);
    return;
  }
  if (command.action === "play") {
    // Play from the dashboard restarts a stream that gave up, as Play here does.
    if (stalled()) {
      togglePlay();
      return;
    }
    const media = nodes?.video;
    if (!media || media.ended) return;
    void media.play().catch(() => undefined);
    postPlace(true);
    return;
  }
  if (command.action === "seek" && command.seconds != null) await skipTo(command.seconds);
}

function originFrom(job: PlayJob): number {
  if (job.position != null && Number.isFinite(job.position) && job.position >= 0) return job.position;
  return 0;
}

/** Hide a track before dropping it; Chrome can otherwise leave its last cue drawn on the picture. */
function destroyHls(): void {
  pendingAttach = null;
  if (hls) hls.destroy();
  hls = null;
  attached = false;
  started = false;
  buffering = false;
  const media = nodes?.video;
  if (!media) return;
  if (pendingSubtitleApply) media.removeEventListener("loadeddata", pendingSubtitleApply);
  pendingSubtitleApply = null;
  media.pause();
  removeTracks(media);
  media.removeAttribute("src");
  media.load();
  clearSubtitles();
}

function dropStream(): void {
  loadedPlaylist = "";
  destroyHls();
  syncPlayButton();
}

/*
 * Subtitles on the TV page: the job's embedded tracks, plus online results
 * searched once per job (as the dock player does). `prefs.subtitles` is on
 * or off for every screen. A track picked here is this item's `subPick`;
 * its language is remembered, so the next item starts in that language.
 */
let onlineKey = "";
/** The item (its file) the online results were found for. */
let onlineFor = "";
let onlineItems: SubtitleCandidate[] = [];
let onlineToken = 0;
/** This item's explicit choice: "e:<embedded id>" or "o:<online id>". */
let subPick: string | null = null;
/** The attached online track, "<id>@<origin>", and its blob URL. */
let onlineActive = "";
let onlineBlob = "";
let onlineLoading = "";
const onlineFailed = new Set<string>();
let castSubSettings: SubtitleSettings | null = null;
/** A subtitle failure shown on the picture for a moment. */
let subNote = "";
let subNoteTimer = 0;
const SUB_NOTE_MS = 4000;

/** Forgets the item's online results and its pick; a late answer for it is ignored. */
function forgetOnline(): void {
  onlineToken += 1;
  onlineKey = "";
  onlineFor = "";
  onlineItems = [];
  subPick = null;
  onlineFailed.clear();
  dropOnlineTrack();
  clearSubNote();
}

/** Hide the track before dropping it; Chrome can otherwise leave its last cue drawn on the picture. */
function dropOnlineTrack(): void {
  const node = nodes?.video.querySelector<HTMLTrackElement>("track[data-online]");
  if (node) {
    node.track.mode = "disabled";
    node.remove();
  }
  if (onlineBlob) URL.revokeObjectURL(onlineBlob);
  onlineBlob = "";
  onlineActive = "";
  onlineLoading = "";
}

/** Says on the picture why subtitles did not load; it goes after a few seconds or at the next choice. */
function noteSubtitle(text: string): void {
  window.clearTimeout(subNoteTimer);
  subNote = text;
  setStatus(text);
  subNoteTimer = window.setTimeout(() => clearSubNote(), SUB_NOTE_MS);
}

function clearSubNote(): void {
  window.clearTimeout(subNoteTimer);
  subNoteTimer = 0;
  if (!subNote) return;
  const shown = nodes?.status.textContent === subNote;
  subNote = "";
  if (shown) setStatus("");
  else paintStage();
}

function clearSubtitles(): void {
  currentSubs = [];
  dropOnlineTrack();
  const select = nodes?.subs;
  if (!select) return;
  fillingSubs = true;
  select.replaceChildren();
  select.add(new Option("Auto", "auto"));
  select.add(new Option("Off", "off"));
  select.value = "off";
  select.disabled = true;
  fillingSubs = false;
  paintSubtitleState();
}

/** This item's pick, while the item still has it. */
function validPick(): string | null {
  const pick = subPick;
  if (pick?.startsWith("e:") && currentSubs.some((track) => "e:" + track.id === pick)) return pick;
  if (pick?.startsWith("o:") && onlineItems.some((item) => "o:" + item.id === pick)) return pick;
  return null;
}

/** "e:<id>", "o:<id>", or null, by the same rules as the dock player. */
function chosenSubtitle(): string | null {
  if (prefs.subtitles === "off") return null;
  const pick = validPick();
  if (pick) return pick;
  const usable = onlineItems.filter((item) => !onlineFailed.has(item.id));
  // The language of the last pick comes first, in the video, then online.
  const language = prefs.subtitleLanguage;
  if (language) {
    const track = preferredTrack(currentSubs, [language]);
    if (track) return "e:" + track.id;
    const item = usable.find((candidate) => sameLanguage(candidate.language, language));
    if (item) return "o:" + item.id;
  }
  return autoSubtitle(currentSubs, usable, preferredLanguages(castSubSettings));
}

function chosenName(choice: string | null): string {
  if (!choice) return "";
  const id = choice.slice(2);
  if (choice.startsWith("e:")) {
    const track = currentSubs.find((item) => String(item.id) === id);
    return track ? trackName(track) : "";
  }
  const item = onlineItems.find((candidate) => candidate.id === id);
  return item ? candidateTitle(item) : "";
}

function applySubtitleMode(): void {
  const media = nodes?.video;
  if (!media) return;
  const want = attached || loadedPlaylist ? chosenSubtitle() : null;
  const embedded = Array.from(media.querySelectorAll<HTMLTrackElement>("track:not([data-online])"));
  embedded.forEach((node, index) => {
    const track = currentSubs[index];
    node.track.mode = track && want === "e:" + track.id ? "showing" : "disabled";
  });
  if (!want?.startsWith("o:")) {
    if (onlineActive || onlineLoading) dropOnlineTrack();
    return;
  }
  const id = want.slice(2);
  const key = id + "@" + origin;
  if (key === onlineActive) {
    const node = media.querySelector<HTMLTrackElement>("track[data-online]");
    if (node) node.track.mode = "showing";
    return;
  }
  if (key === onlineLoading) return;
  dropOnlineTrack();
  onlineLoading = key;
  const token = onlineToken;
  const item = onlineItems.find((candidate) => candidate.id === id);
  void onlineTrackUrl(id, origin).then(
    (url) => {
      if (token !== onlineToken || onlineLoading !== key || !nodes) {
        URL.revokeObjectURL(url);
        return;
      }
      onlineLoading = "";
      const node = document.createElement("track");
      node.kind = "subtitles";
      node.label = item ? candidateTitle(item) : "Subtitles";
      const srclang = trackLanguage(item?.language);
      if (srclang) node.srclang = srclang;
      node.src = url;
      node.default = true;
      node.dataset.online = "1";
      node.addEventListener("load", () => placeCues(true), { once: true });
      nodes.video.appendChild(node);
      node.track.mode = "showing";
      onlineActive = key;
      onlineBlob = url;
      // An online pick's language is remembered once it has loaded.
      if (subPick === want && item?.language) savePrefs(item.language);
    },
    (err: unknown) => {
      if (token !== onlineToken || onlineLoading !== key) return;
      onlineLoading = "";
      onlineFailed.add(id);
      if (subPick === want) subPick = null;
      noteSubtitle(err instanceof Error && err.message ? err.message : "Those subtitles could not be loaded.");
      paintSubtitleChoices();
      applySubtitleMode();
    },
  );
}

/** The select's value: Off, this item's pick, or Auto. */
function subtitleSelectValue(): string {
  if (prefs.subtitles === "off") return "off";
  return validPick() ?? "auto";
}

function paintSubtitleState(): void {
  if (!nodes) return;
  const { subs: select, cc } = nodes;
  const chosen = select.selectedOptions[0];
  const name = chosen?.textContent?.trim() || "Off";
  const empty = select.disabled || (currentSubs.length === 0 && onlineItems.length === 0);
  const on = !empty && select.value !== "off";
  select.classList.toggle("is-on", on);
  select.title = empty ? "No subtitles" : "Subtitles: " + name;
  select.setAttribute("aria-label", empty ? "Subtitles, no tracks" : "Subtitles, " + name);
  // CC turns subtitles on or off; it works before any track is found.
  const ccOn = prefs.subtitles !== "off";
  const shown = ccOn ? chosenName(chosenSubtitle()) : "";
  const state = !ccOn ? "off" : shown ? "on, " + shown : "on";
  cc.disabled = !currentJob;
  cc.classList.toggle("is-on", ccOn);
  cc.setAttribute("aria-pressed", ccOn ? "true" : "false");
  cc.title = "Subtitles " + state + " (c)";
  cc.setAttribute("aria-label", "Subtitles " + state);
}

/** The select's choices: Auto, Off, embedded tracks, then online results. */
function paintSubtitleChoices(): void {
  const select = nodes?.subs;
  if (!select) return;
  fillingSubs = true;
  select.replaceChildren();
  select.add(new Option("Auto", "auto"));
  select.add(new Option("Off", "off"));
  for (const track of currentSubs) select.add(new Option(trackName(track), "e:" + track.id));
  if (onlineItems.length) {
    const group = document.createElement("optgroup");
    group.label = "Online";
    for (const item of onlineItems.slice(0, 20)) {
      group.append(new Option(candidateTitle(item) + " (" + item.source + ")", "o:" + item.id));
    }
    select.append(group);
  }
  select.value = subtitleSelectValue();
  select.disabled = currentSubs.length === 0 && onlineItems.length === 0;
  fillingSubs = false;
  paintSubtitleState();
}

/**
 * Off, Auto (Portal picks), or a track for this item. A track's language is
 * remembered, so the next item starts in it; an online pick's once it loads.
 */
function pickSubtitle(value: string): void {
  clearSubNote();
  let language: string | null | undefined;
  if (value === "off") {
    subPick = null;
    prefs.subtitles = "off";
  } else if (value === "auto") {
    subPick = null;
    prefs.subtitles = "auto";
    language = null;
  } else {
    subPick = value;
    prefs.subtitles = "auto";
    if (value.startsWith("e:")) {
      const track = currentSubs.find((item) => "e:" + item.id === value);
      const lang = track?.lang?.trim() ?? "";
      if (lang) language = lang;
    } else {
      onlineFailed.delete(value.slice(2));
    }
  }
  prefs.subtitlesSet = true;
  savePrefs(language);
  applySubtitleMode();
  paintSubtitleState();
}

/** CC: subtitles on or off for every screen. On again comes back to this item's pick. */
function toggleSubtitles(): void {
  if (!currentJob) return;
  clearSubNote();
  prefs.subtitles = prefs.subtitles === "off" ? "auto" : "off";
  prefs.subtitlesSet = true;
  savePrefs();
  paintSubtitleChoices();
  applySubtitleMode();
}

function fillSubtitles(tracks: SubtitleTrack[]): void {
  const media = nodes?.video;
  if (!nodes?.subs || !media) return;
  currentSubs = tracks;
  paintSubtitleChoices();
  removeTracks(media);
  dropOnlineTrack();
  tracks.forEach((track) => {
    const node = document.createElement("track");
    node.kind = "subtitles";
    node.label = trackName(track);
    const srclang = trackLanguage(track.lang);
    if (srclang) node.srclang = srclang;
    node.src = track.url;
    node.addEventListener("load", () => placeCues(true), { once: true });
    media.appendChild(node);
  });
  const apply = () => applySubtitleMode();
  if (pendingSubtitleApply) media.removeEventListener("loadeddata", pendingSubtitleApply);
  pendingSubtitleApply = apply;
  media.addEventListener("loadeddata", apply, { once: true });
  window.setTimeout(apply, 300);
}

/** What the TV page knows about the video, for the subtitle search. */
function castSeed(): MediaInfo | null {
  const replay = currentReplay;
  if (!replay) return null;
  if (replay.kind === "episode" && replay.season != null && replay.episode) {
    const show = replay.showTitle?.trim() || "";
    return {
      kind: "episode",
      title: cleanEpisodeTitle(replay.title, replay.season, replay.episode, show) || null,
      show_title: show || null,
      season: replay.season,
      episode: replay.episode,
    };
  }
  if (replay.kind === "movie") return { kind: "movie", title: replay.title.trim() };
  return null;
}

/** Online results, once per job. A search skipped or failed keeps no results from another item. */
function searchOnline(job: PlayJob): void {
  if (onlineKey === job.id) return;
  onlineKey = job.id;
  const token = onlineToken;
  const item = currentReplay?.file ?? "";
  const none = (): void => {
    if (token !== onlineToken || !onlineItems.length || (item && onlineFor === item)) return;
    onlineItems = [];
    onlineFor = "";
    paintSubtitleChoices();
    applySubtitleMode();
  };
  const seed = castSeed();
  if (!seed) {
    none();
    return;
  }
  void (async () => {
    const settings = await subtitleSettings();
    if (token !== onlineToken) return;
    castSubSettings = settings;
    const body = searchFor(seed, job.release, settings);
    if (!body) {
      none();
      return;
    }
    try {
      const results = await api.searchSubtitles(body);
      if (token !== onlineToken) return;
      onlineItems = cleanResults(results).items;
      onlineFor = item;
    } catch {
      none();
      return;
    }
    paintSubtitleChoices();
    applySubtitleMode();
  })();
}

function paintScrubFill(bar: HTMLInputElement): void {
  const max = Number(bar.max);
  const at = Number(bar.value);
  const fill = Number.isFinite(max) && max > 0 && Number.isFinite(at) ? Math.min(100, Math.max(0, (at / max) * 100)) : 0;
  bar.style.setProperty("--scrub-fill", fill.toFixed(2) + "%");
}

function paintHours(duration: number): void {
  if (!nodes) return;
  const hours = duration >= 3600;
  nodes.elapsed.classList.toggle("has-hours", hours);
  nodes.total.classList.toggle("has-hours", hours);
}

function scrubRatio(bar: HTMLInputElement, clientX: number): number | null {
  const rect = bar.getBoundingClientRect();
  if (rect.width <= 0) return null;
  const thumb = Number.parseFloat(getComputedStyle(bar).getPropertyValue("--scrub-thumb"));
  const thumbPx = Number.isFinite(thumb) && thumb > 0 && thumb < rect.width ? thumb : 0;
  const span = rect.width - thumbPx;
  if (span <= 0) return 0;
  const x = clientX - rect.left - thumbPx / 2;
  if (x <= 0) return 0;
  if (x >= span) return 1;
  return x / span;
}

function concealHover(): void {
  const tip = nodes?.hover;
  if (!tip) return;
  tip.hidden = true;
  tip.textContent = "";
}

function hideHover(): void {
  hoverX = null;
  concealHover();
}

function placeHover(clientX: number): void {
  const bar = nodes?.bar;
  const tip = nodes?.hover;
  if (!bar || !tip) return;
  const max = Number(bar.max);
  const ratio = scrubRatio(bar, clientX);
  if (bar.disabled || skipping || ratio == null || !Number.isFinite(max) || max <= 0) {
    concealHover();
    return;
  }
  const at = clampSkip(ratio * max, max);
  tip.textContent = formatClock(at);
  tip.hidden = false;
  const host = bar.parentElement;
  if (!host) return;
  const hostRect = host.getBoundingClientRect();
  const half = tip.offsetWidth / 2;
  const edge = 4;
  const x = clientX - hostRect.left;
  const clamped = Math.min(Math.max(x, half + edge), Math.max(half + edge, hostRect.width - half - edge));
  tip.style.left = clamped + "px";
}

function paintClock(): void {
  if (!nodes || dragging || skipping) return;
  const position = sourcePosition();
  const duration = progressDuration();
  const elapsed = formatClock(position);
  const total = formatClock(duration);
  if (nodes.elapsed.textContent !== elapsed) nodes.elapsed.textContent = elapsed;
  if (nodes.total.textContent !== total) nodes.total.textContent = total;
  paintHours(duration);
  const max = Number.isFinite(duration) && duration > 0 ? Math.floor(duration) : 0;
  nodes.bar.max = String(max);
  const at = max > 0 ? Math.min(Math.floor(position), max) : 0;
  nodes.bar.value = String(at);
  nodes.bar.disabled = max <= 0 || !currentJob;
  paintSeekText(nodes.bar, seekValueText(at, max));
  paintScrubFill(nodes.bar);
  paintBuffered(nodes.bar, nodes.video, origin, max);
  if (hoverX == null) concealHover();
  else placeHover(hoverX);
}

function resetClock(): void {
  if (!nodes) return;
  dragging = false;
  nodes.chrome.classList.remove("is-scrubbing");
  nodes.bar.disabled = true;
  nodes.bar.value = "0";
  nodes.bar.max = "0";
  paintSeekText(nodes.bar, seekValueText(0, 0));
  paintScrubFill(nodes.bar);
  nodes.bar.style.setProperty("--scrub-buffer", "0%");
  hideHover();
  nodes.elapsed.textContent = formatClock(0);
  nodes.total.textContent = formatClock(0);
  paintHours(0);
}

function nextDetail(next: NextItem): string {
  const parts: string[] = [];
  const code = episodeCode(next.season, next.episode);
  if (code) parts.push(code);
  const title = cleanEpisodeTitle(next.title, next.season, next.episode, next.show_title);
  if (title) parts.push(title);
  return parts.join(" · ");
}

function nextIsEpisode(next: NextItem): boolean {
  return next.kind === "episode" || !!(next.season && next.episode);
}

function paintNext(): void {
  const button = nodes?.next;
  if (!button) return;
  const next = currentNext?.file ? currentNext : null;
  button.disabled = !next;
  if (!next) {
    button.textContent = "Next episode";
    button.title = "Next episode";
    button.setAttribute("aria-label", "Next episode");
    return;
  }
  const detail = nextDetail(next);
  const episode = nextIsEpisode(next);
  const lead = episode ? "Next episode" : "Next";
  const full = detail ? lead + " · " + detail : lead;
  const compact = episode && episodeCode(next.season, next.episode)
    ? "Next · " + episodeCode(next.season, next.episode)
    : lead;
  button.textContent = full.length > 36 ? compact : full;
  button.title = full;
  button.setAttribute("aria-label", full);
}

function paintVolume(): void {
  if (!nodes) return;
  const { volume, mute } = nodes;
  const pct = Math.round(prefs.volume * 100);
  const muted = prefs.volume <= 0 || nodes.video.muted;
  volume.setAttribute("aria-label", muted ? "Volume, muted" : "Volume, " + pct + "%");
  volume.setAttribute("aria-valuetext", muted ? "Muted" : pct + "%");
  volume.style.setProperty("--volume-fill", (muted ? 0 : pct) + "%");
  mute.classList.toggle("is-muted", muted);
  mute.setAttribute("aria-label", muted ? "Unmute" : "Mute");
  mute.title = (muted ? "Unmute" : "Mute") + " (m)";
}

function paintLinks(): void {
  const button = nodes?.links;
  if (!button) return;
  const ready = !!(currentReplay?.file && currentJob);
  button.disabled = !ready;
  const label = ready
    ? "Choose a stream link"
    : currentJob
      ? "Choose a source from the controller"
      : "Links unavailable";
  button.textContent = label;
  button.title = label;
  button.setAttribute("aria-label", label);
}

function paintQuality(): void {
  const select = nodes?.quality;
  if (!select || !nodes) return;
  const ready = !!(currentReplay?.file && currentJob);
  select.disabled = !ready;
  const title = ready ? "Quality" : "Change quality from the controller";
  select.title = title;
  nodes.qualityLabel.title = title;
}

function audioKey(tracks: AudioTrack[]): string {
  return tracks.map((track) => track.index + "\n" + track.label + "\n" + (track.lang ?? "")).join("\n");
}

/** Shows the file's audio tracks. One track stays visible and cannot be changed. */
function paintAudio(job: PlayJob | null): void {
  const select = nodes?.audio;
  if (!select || !nodes) return;
  const tracks = audioTracksOf(job);
  const listKey = audioKey(tracks);
  if (listKey !== audioListKey) {
    window.clearTimeout(audioTimer);
    audioTimer = 0;
    audioPending = false;
  }
  if (audioPending && listKey === audioListKey) return;
  currentAudio = tracks;
  audioListKey = listKey;
  const chosen = tracks.some((track) => track.index === job?.audio_index) ? (job?.audio_index ?? null) : (tracks[0]?.index ?? null);
  currentAudioIndex = chosen;
  fillingAudio = true;
  select.replaceChildren();
  if (!tracks.length) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "Audio";
    select.append(option);
    select.value = "";
    select.disabled = true;
    select.title = "Audio";
    nodes.audioLabel.title = "Audio";
    fillingAudio = false;
    return;
  }
  for (const track of tracks) {
    const option = document.createElement("option");
    option.value = String(track.index);
    option.textContent = track.label;
    select.append(option);
  }
  select.value = chosen == null ? "" : String(chosen);
  select.disabled = tracks.length < 2;
  const title = tracks.length < 2 ? tracks[0].label : "Audio track";
  select.title = title;
  nodes.audioLabel.title = title;
  fillingAudio = false;
}

/**
 * Switches this TV onto the picked track without asking the add-on again.
 * The poll stays quiet while ffmpeg restarts, so it does not drop the picture
 * and start the old stream over.
 */
async function applyAudio(): Promise<void> {
  audioPending = false;
  const select = nodes?.audio;
  if (!select || !nodes || fillingAudio) return;
  const raw = select.value;
  if (!/^\d+$/.test(raw)) return;
  const index = Number(raw);
  if (index === currentAudioIndex) return;
  const chosen = currentAudio.find((track) => track.index === index);
  if (!chosen) {
    paintAudio(null);
    return;
  }
  if (chosen.lang) savePrefs(undefined, chosen.lang);
  const id = currentJob;
  if (!id || !loadedPlaylist) {
    paintAudio(null);
    return;
  }
  const at = sourcePosition();
  const token = followToken;
  const mine = ++skipGeneration;
  skipping = true;
  nodes.video.pause();
  hls?.stopLoad();
  setWaiting("Switching audio…");
  syncPlayButton();
  try {
    const job = await api.setAudio(id, index, at);
    if (!active || token !== followToken || mine !== skipGeneration) return;
    skipping = false;
    if (job.playlist) attachPlayback(job);
    else {
      setMessage(job.error || "The audio track did not start.");
      paintAudio(job);
    }
  } catch (err) {
    if (!active || token !== followToken || mine !== skipGeneration) return;
    skipping = false;
    setMessage(message(err));
    fillingAudio = true;
    if (currentAudioIndex != null) select.value = String(currentAudioIndex);
    fillingAudio = false;
    if (loadedPlaylist) {
      hls?.startLoad();
      void nodes.video.play().catch(() => undefined);
    }
  } finally {
    if (mine === skipGeneration) {
      skipping = false;
      syncPlayButton();
    }
  }
}

function paintJob(job: PlayJob): void {
  const found = replayFromJob(job);
  if (found) currentReplay = found;
  currentNext = job.next ?? null;
  if (job.title) setTitle(job.title);
  paintNext();
  if (job.duration != null && Number.isFinite(job.duration) && job.duration >= 0) jobDuration = job.duration;
  paintLinks();
  paintQuality();
  paintAudio(job);
}

function syncPlayButton(): void {
  const button = nodes?.play;
  const media = nodes?.video;
  if (!button || !media || !nodes) return;
  const playing = !!loadedPlaylist && !media.paused && !media.ended && !skipping;
  const label = playing ? "Pause" : "Play";
  button.classList.toggle("is-playing", playing);
  button.setAttribute("aria-label", label);
  button.title = label + " (k)";
  // A stream that gave up keeps Play, which restarts it.
  button.disabled = (!loadedPlaylist && !stalled()) || skipping;
  // ±10 stays pressable while a skip is under way: the press adds to it, and a remote keeps its focus.
  const seekable = !!loadedPlaylist && !!currentJob;
  nodes.back.disabled = !seekable;
  nodes.forward.disabled = !seekable;
  nodes.full.disabled = !loadedPlaylist && document.fullscreenElement !== nodes.stage;
  paintStage();
}

function syncFullscreen(): void {
  if (!nodes) return;
  const button = nodes.full;
  const on = document.fullscreenElement === nodes.stage;
  const label = on ? "Exit fullscreen" : "Fullscreen";
  nodes.stage.classList.toggle("is-full", on);
  button.classList.toggle("is-on", on);
  button.setAttribute("aria-label", label);
  button.title = label + " (f)";
  button.disabled = !loadedPlaylist && !on;
  wake();
}

function skipBy(delta: number): void {
  if (!nodes) return;
  if (skipping) {
    // Keep the press: it runs from where the seek in flight lands.
    if (currentJob) queuedSkip = (queuedSkip ?? skipTarget) + delta;
    return;
  }
  if (nodes.bar.disabled) return;
  void skipTo(sourcePosition() + delta);
}

/** Moves the video to `local` seconds if the browser can seek there without a new stream. */
function seekInPlace(local: number): boolean {
  const media = nodes?.video;
  if (!media) return false;
  const ranges = media.seekable;
  if (!Number.isFinite(local) || local < 0 || !ranges.length) return false;
  const start = ranges.start(0);
  const end = ranges.end(ranges.length - 1);
  // Keep clear of the live edge, where the next segment may not exist yet.
  if (local < start || local > end - 2) return false;
  media.currentTime = local;
  return true;
}

function toggleMute(): void {
  if (!nodes) return;
  const media = nodes.video;
  if (media.muted || prefs.volume <= 0) {
    media.muted = false;
    if (prefs.volume <= 0) {
      prefs.volume = 0.5;
      nodes.volume.value = String(prefs.volume);
      media.volume = prefs.volume;
      savePrefs();
    }
  } else {
    media.muted = true;
  }
  paintVolume();
}

function nudgeVolume(delta: number): void {
  if (!nodes) return;
  const media = nodes.video;
  prefs.volume = clampVolume(Math.round((prefs.volume + delta) * 20) / 20);
  media.volume = prefs.volume;
  media.muted = false;
  nodes.volume.value = String(prefs.volume);
  savePrefs();
  paintVolume();
  wake();
}

function togglePlay(): void {
  const media = nodes?.video;
  if (!media || skipping) return;
  if (stalled() && stalledAt != null && currentJob) {
    // The stall is kept until the new stream attaches, so a failed restart can be tried again.
    void skipTo(stalledAt);
    return;
  }
  if (!loadedPlaylist) return;
  if (media.paused || media.ended) void media.play().catch(() => undefined);
  else media.pause();
}

function toggleFullscreen(): void {
  const stage = nodes?.stage;
  if (!stage) return;
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => undefined);
  else if (loadedPlaylist) void stage.requestFullscreen().catch(() => undefined);
}

function positionDrift(job: PlayJob): boolean {
  if (job.position == null || !Number.isFinite(job.position)) return false;
  return Math.abs(job.position - origin) > 1.5;
}

/** The file's own subtitles are read after playback starts; they join the menu when ready. */
function awaitSubtitles(job: PlayJob): void {
  if (!job.subtitles_pending) return;
  const current = (): boolean => active && attached && currentJob === job.id && loadedPlaylist === job.playlist;
  void (async () => {
    for (let tries = 0; tries < 100; tries += 1) {
      await sleep(3000);
      if (!current()) return;
      let now: PlayJob;
      try {
        now = await api.job(job.id);
      } catch {
        continue;
      }
      if (!current() || now.session !== job.session) return;
      if (now.subtitles_pending) continue;
      if (now.subtitles?.length) fillSubtitles(now.subtitles);
      return;
    }
  })();
}

/** A job waiting for hls.js to load before it attaches. */
let pendingAttach: PlayJob | null = null;

function attachPlayback(job: PlayJob): void {
  if (!hlsReady()) {
    // hls.js loads on the first play; attach once it is here, unless a newer
    // attach or a stop came first.
    pendingAttach = job;
    void loadHls().then(() => {
      if (pendingAttach === job) attachPlayback(job);
    });
    return;
  }
  pendingAttach = null;
  const playlist = job.playlist;
  const media = nodes?.video;
  if (!playlist || !media || !nodes) {
    setMessage("The stream did not arrive.");
    return;
  }
  origin = originFrom(job);
  if (job.duration != null && Number.isFinite(job.duration) && job.duration >= 0) jobDuration = job.duration;
  loadedPlaylist = playlist;
  stalledAt = null;
  destroyHls();
  fillSubtitles(job.subtitles ?? []);
  paintAudio(job);
  searchOnline(job);
  attached = true;
  waiting = false;
  awaitSubtitles(job);
  media.volume = prefs.volume;
  hls = attachHls(media, playlist, (detail) => {
    // Out of retries: say why on the stage, and Play picks up from here.
    stalledAt = sourcePosition();
    stalledOn = playlist;
    attached = false;
    started = false;
    buffering = false;
    dropStream();
    setMessage(
      /load|timeout/i.test(detail)
        ? "The stream stopped loading. Press Play to try again."
        : "This video could not be played here. Press Play to try again.",
    );
  });
  void media.play().catch(() => undefined);
  setStatus("");
  paintClock();
  syncPlayButton();
  postPlace(true);
}

async function skipTo(seconds: number): Promise<void> {
  if (skipping || !nodes) return;
  const id = currentJob;
  const token = followToken;
  if (!id) return;
  const at = clampSkip(seconds, progressDuration());
  // Inside what this page already has, move in place: instant, no new stream,
  // and quick presses of ±10 add up.
  if (attached && loadedPlaylist && seekInPlace(at - origin)) {
    paintClock();
    postPlace(true);
    return;
  }
  skipping = true;
  skipTarget = at;
  queuedSkip = null;
  const mine = ++skipGeneration;
  nodes.video.pause();
  setStatus("Skipping to " + formatClock(at));
  nodes.bar.disabled = true;
  syncPlayButton();
  let restart: Replay | null = null;
  try {
    const job = await api.seek(id, at);
    if (!active || token !== followToken || mine !== skipGeneration) return;
    if (!job.playlist) {
      const live = await api.job(id).catch(() => null);
      if (!active || token !== followToken || mine !== skipGeneration) return;
      if (live?.state === "ready" && live.playlist) {
        paintJob(live);
        attachPlayback(live);
        return;
      }
      if (live && live.state !== "failed" && live.state !== "ready") {
        setMessage("The stream did not arrive.");
        paintClock();
        return;
      }
      restart = currentReplay;
      stalledAt = at;
      dropStream();
      setMessage("The stream did not arrive.");
      paintClock();
      return;
    }
    paintJob(job);
    attachPlayback(job);
  } catch (err) {
    if (!active || token !== followToken || mine !== skipGeneration) return;
    const failure = message(err);
    const live = await api.job(id).catch(() => null);
    if (!active || token !== followToken || mine !== skipGeneration) return;
    if (live?.state === "ready" && live.playlist) {
      // The same stream means the seek was refused before it was touched.
      if (attached && loadedPlaylist === live.playlist) {
        void nodes.video.play().catch(() => undefined);
        setMessage(failure);
      } else {
        paintJob(live);
        attachPlayback(live);
      }
      paintClock();
      return;
    }
    // Still resolving: say why, and leave that job alone.
    if (live && live.state !== "failed" && live.state !== "ready") {
      setMessage(failure);
      paintClock();
      return;
    }
    // The old stream is gone. Play starts this item again from here.
    restart = currentReplay;
    stalledAt = at;
    dropStream();
    setMessage(failure);
    paintClock();
  } finally {
    if (mine === skipGeneration) {
      skipping = false;
      syncPlayButton();
      // A dashboard command waiting wins over ±10 presses made meanwhile.
      const queued = queuedCommand ? null : queuedSkip;
      queuedSkip = null;
      flushQueuedCommand();
      if (queued !== null && active && token === followToken) void skipTo(queued);
      else if (restart?.file && active && token === followToken) void startReplay(restart, false, at);
    }
  }
}

function stopPlayback(): void {
  forgetOnline();
  stalledAt = null;
  currentJob = null;
  currentNext = null;
  currentReplay = null;
  skipping = false;
  waiting = false;
  origin = 0;
  jobDuration = 0;
  dropStream();
  resetClock();
  paintNext();
  paintLinks();
  paintQuality();
  window.clearTimeout(audioTimer);
  audioTimer = 0;
  audioPending = false;
  paintAudio(null);
  setTitle("");
  nodes?.root.classList.remove("is-playing");
  syncPlayButton();
}

function stopLocal(): void {
  followToken += 1;
  watchingId = null;
  pin = null;
  stopPlayback();
  setStatus("");
}

async function run(jobId: string, token: number): Promise<void> {
  while (active && token === followToken) {
    if (skipping) {
      await sleep(200);
      continue;
    }
    let job: PlayJob;
    try {
      job = await api.job(jobId);
    } catch (err) {
      if (!active || token !== followToken) return;
      pollFailed = true;
      setMessage(message(err));
      await sleep(1000);
      continue;
    }
    if (!active || token !== followToken) return;
    if (skipping) {
      await sleep(200);
      continue;
    }
    if (pollFailed) {
      pollFailed = false;
      setStatus("");
    }
    paintJob(job);
    if (job.state === "failed") {
      // Remember the place before the picture is dropped, so Play can start
      // this item again. Seeking the failed job cannot.
      if (!stalled()) stalledAt = sourcePosition();
      setMessage(job.error || "Playback failed.");
      if (loadedPlaylist) dropStream();
      syncPlayButton();
      await sleep(1000);
      continue;
    }
    if (job.state === "ready" && job.playlist) {
      // The stream that gave up waits for Play, which restarts it where it stopped.
      // Attaching it again here would start it over from its beginning.
      const gaveUp = stalled() && job.playlist === stalledOn;
      if (!gaveUp && (loadedPlaylist !== job.playlist || (loadedPlaylist && positionDrift(job)))) attachPlayback(job);
      if (loadedPlaylist) {
        postPlace(false);
        void postProgress(false);
      }
      await sleep(1000);
      continue;
    }
    if (job.state === "browse") {
      setMessage("Choose a stream link from the controller.");
      await sleep(1000);
      continue;
    }
    if (loadedPlaylist && (job.state === "working" || job.state === "prompt")) {
      dropStream();
      setWaiting(job.state === "prompt" ? "Waiting for the controller…" : "Skipping…");
    } else if (job.state === "prompt") {
      setWaiting("Waiting for the controller…");
    } else if (!loadedPlaylist) {
      if (job.state === "ready") setMessage("The stream did not arrive.");
      else setWaiting("Opening…");
    }
    await sleep(500);
  }
}

async function follow(jobId: string): Promise<void> {
  if (!nodes || !active) return;
  const token = ++followToken;
  watchingId = jobId;
  // Another job for the same item (a quality or link change) keeps its subtitle pick.
  if (currentJob !== jobId && !sameItem(sessionReplay, currentReplay)) forgetOnline();
  currentJob = jobId;
  currentReplay = sessionReplay;
  stalledAt = null;
  origin = 0;
  jobDuration = 0;
  nodes.root.classList.add("is-playing");
  pollFailed = false;
  dropStream();
  resetClock();
  setWaiting("Opening…");
  paintLinks();
  paintQuality();
  paintAudio(null);
  paintNext();
  await run(jobId, token);
}

/** The same file, so its subtitles and the pick still fit. */
function sameItem(a: Replay | null, b: Replay | null): boolean {
  return !!a?.file && !!b?.file && a.file === b.file;
}

/** `resume` is where the source should start: the current place for a quality or link change, 0 for a new item. */
async function startReplay(target: Replay, manualLinks: boolean, resume = 0): Promise<void> {
  if (starting || !nodes || !target.file) return;
  const previous = currentJob;
  starting = true;
  holdSession = true;
  const token = ++followToken;
  const baseline = watchingId ?? "";
  let acceptedId = "";
  let resumeBaseline = "";
  // A new item starts without the last one's subtitles; a quality or link change keeps them.
  if (!sameItem(target, currentReplay)) forgetOnline();
  try {
    nodes.root.classList.add("is-playing");
    stalledAt = null;
    dropStream();
    resetClock();
    setWaiting(manualLinks ? "Opening links…" : "Opening…");
    const accepted = await api.play(replayBody(target, manualLinks, resume));
    acceptedId = accepted.id;
    if (!active || token !== followToken) {
      await api.stop(accepted.id).catch(() => undefined);
      return;
    }
    // The new play replaces this screen's stream. Leaving the old job
    // transcoding keeps a second ffmpeg running until Portal quits.
    if (previous && previous !== accepted.id) await api.stop(previous).catch(() => undefined);
    currentReplay = target;
    watchingId = accepted.id;
    currentJob = accepted.id;
    pin = { jobId: accepted.id, baseline };
    setTitle(target.title);
    paintLinks();
    paintQuality();
    paintAudio(null);
    try {
      await api.setCastSession(receiver.id, accepted.id, target.title, {
        kind: target.kind,
        id: target.id,
        file: target.file,
        title: target.title,
        season: target.season,
        episode: target.episode,
        art: target.art,
        showTitle: target.showTitle,
        showId: target.showId,
      });
    } catch (err) {
      if (token === followToken) setStatus(message(err));
    }
    if (!active || token !== followToken) return;
  } catch (err) {
    if (active && token === followToken) {
      setMessage(message(err));
      resumeBaseline = baseline;
    }
    acceptedId = "";
  } finally {
    starting = false;
    holdSession = false;
  }
  if (resumeBaseline && active) {
    await follow(resumeBaseline);
    return;
  }
  if (!acceptedId || !active || token !== followToken) {
    if (!acceptedId && !resumeBaseline) nodes?.root.classList.remove("is-playing");
    return;
  }
  await run(acceptedId, token);
}

async function closeCast(): Promise<void> {
  if (closing) return;
  closing = true;
  const id = currentJob;
  await postProgress(true).catch(() => undefined);
  holdSession = true;
  pin = null;
  followToken += 1;
  watchingId = null;
  stopPlayback();
  setStatus("");
  try {
    await api.clearCastSession();
  } catch (err) {
    if (active) setStatus(message(err));
  }
  if (id) await api.stop(id).catch(() => undefined);
  holdSession = false;
  closing = false;
}

/** Portal ended this TV's cast after it went quiet (asleep, or the tab closed). */
function lostText(position: number | null): string {
  const at = position != null && position >= 1 ? " at " + formatClock(position) : "";
  return "This TV lost the cast" + at + ". Cast again from Portal.";
}

function applySession(session: SessionView): void {
  if (!active || holdSession) return;
  sessionReplay = session.replay;
  if (pin) {
    if (session.job_id === pin.jobId) pin = null;
    else if (session.job_id && session.job_id !== pin.baseline) pin = null;
    else return;
  }
  const targeted = !session.receiver_id || session.receiver_id === receiver.id;
  if (!targeted) {
    if (watchingId || currentJob) stopLocal();
    setStatus("Casting to another screen.");
    return;
  }
  if (session.replay && session.job_id && session.job_id === currentJob) {
    currentReplay = session.replay;
    paintLinks();
    paintQuality();
  }
  if (session.job_id === watchingId) {
    if (session.title && !nodes?.title.textContent) setTitle(session.title);
    return;
  }
  if (!session.job_id) {
    if (watchingId || currentJob) stopLocal();
    // The waiting screen says why this TV's cast ended, and keeps no older message.
    const lost = session.lost?.receiver_id === receiver.id ? session.lost : null;
    const text = lost ? lostText(lost.position) : "";
    if (nodes?.status.textContent !== text) setStatus(text);
    return;
  }
  void follow(session.job_id);
  if (session.title) setTitle(session.title);
}

async function pollSession(): Promise<void> {
  if (!active || sessionBusy || holdSession || !nodes) return;
  sessionBusy = true;
  try {
    let raw: CastSession;
    try {
      raw = await api.castSession();
    } catch (err) {
      if (!active) return;
      if (isMissing(err)) {
        applySession({ receiver_id: "", job_id: "", title: "", replay: null, lost: null });
        return;
      }
      if (!currentJob) setStatus(message(err));
      return;
    }
    if (!active) return;
    const view = readSession(raw);
    applySession(view);
    acceptCommand(raw, view.job_id);
    // A subtitle choice made on the dashboard reaches this TV while it plays.
    if (currentJob && Date.now() - lastPrefsSync >= PREFS_SYNC_MS) {
      lastPrefsSync = Date.now();
      void syncPrefs();
    }
  } finally {
    sessionBusy = false;
  }
}

async function pulsePresence(): Promise<void> {
  if (!active || presenceBusy) return;
  presenceBusy = true;
  try {
    await api.castPresence(receiver.id, receiver.name);
  } catch {
    /* Stay on the waiting screen. The next pulse retries. */
  } finally {
    presenceBusy = false;
  }
}

/** Keys a focused slider moves itself by. */
const SLIDER_KEYS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"]);

/**
 * Keys meant for a focused field. A focused button only keeps Space and
 * Enter, and a focused slider only the keys that move it.
 */
function typingTarget(target: EventTarget | null, key: string): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (target instanceof HTMLInputElement && target.type === "range") return SLIDER_KEYS.has(key);
  if (tag === "INPUT" || tag === "SELECT" || tag === "TEXTAREA") return true;
  if (tag === "BUTTON") return key === " " || key === "Spacebar" || key === "Enter";
  return target.isContentEditable;
}

const ARROWS = new Set(["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"]);

/** A button, menu or checkbox in the stage controls or the side panel, which a remote's arrows move between. */
function remoteControl(target: EventTarget | null): HTMLElement | null {
  if (!nodes || !(target instanceof HTMLElement) || !nodes.root.contains(target)) return null;
  if (!target.closest(".player-chrome, .dock-side")) return null;
  if (target instanceof HTMLButtonElement || target instanceof HTMLSelectElement) return target;
  if (target instanceof HTMLInputElement && target.type === "checkbox") return target;
  return null;
}

/** The controls a remote can reach. Sliders keep their own arrows, so they are left out. */
function remoteTargets(): HTMLElement[] {
  if (!nodes) return [];
  const full = document.fullscreenElement;
  const all = nodes.root.querySelectorAll<HTMLElement>(
    ".player-chrome button, .dock-side button, .dock-side select, .dock-side input[type=checkbox]",
  );
  return Array.from(all).filter(
    (node) => !node.matches(":disabled") && node.getClientRects().length > 0 && (!full || full.contains(node)),
  );
}

/** Moves focus to the nearest control in the arrow's direction; straight ahead beats nearer but off to the side. */
function moveFocus(from: HTMLElement, key: string): void {
  const here = from.getBoundingClientRect();
  const x = here.left + here.width / 2;
  const y = here.top + here.height / 2;
  const sideways = key === "ArrowLeft" || key === "ArrowRight";
  let best: HTMLElement | null = null;
  let bestScore = Infinity;
  for (const node of remoteTargets()) {
    if (node === from) continue;
    const box = node.getBoundingClientRect();
    const dx = box.left + box.width / 2 - x;
    const dy = box.top + box.height / 2 - y;
    const ahead = key === "ArrowLeft" ? -dx : key === "ArrowRight" ? dx : key === "ArrowUp" ? -dy : dy;
    if (ahead <= 1) continue;
    const score = ahead + Math.abs(sideways ? dy : dx) * 2;
    if (score < bestScore) {
      best = node;
      bestScore = score;
    }
  }
  best?.focus();
}

/** Nothing in particular has focus, so OK means play or pause. */
function nothingFocused(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return true;
  if (target === document.body || target === document.documentElement) return true;
  return !!nodes && (target === nodes.stage || target === nodes.video);
}

/** A remote's media keys, wherever focus is. Answers whether `key` was one. */
function mediaKey(key: string): boolean {
  const media = nodes?.video;
  if (key === "MediaPlayPause") togglePlay();
  else if (key === "MediaPlay") {
    if (!media || media.paused || media.ended || stalled()) togglePlay();
  } else if (key === "MediaPause") {
    if (media && !media.paused) media.pause();
  } else if (key === "MediaRewind") skipBy(-10);
  else if (key === "MediaFastForward") skipBy(10);
  else return false;
  return true;
}

function onKey(event: KeyboardEvent): void {
  if (!active || !nodes) return;
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  const key = event.key;
  if (mediaKey(key)) {
    event.preventDefault();
    wake();
    return;
  }
  // With a control focused, a remote's arrows move between controls instead of seeking.
  const control = remoteControl(event.target);
  if (control && ARROWS.has(key)) {
    event.preventDefault();
    wake();
    moveFocus(control, key);
    return;
  }
  if (control instanceof HTMLInputElement && key === "Enter") {
    // OK ticks a checkbox, as Space does.
    event.preventDefault();
    control.click();
    return;
  }
  if (control && key === "Escape" && document.fullscreenElement !== nodes.stage) {
    // Back to the picture: arrows seek and change the volume again.
    event.preventDefault();
    control.blur();
    return;
  }
  if (typingTarget(event.target, key)) return;
  wake();
  if (key === " " || key === "Spacebar" || key === "k" || key === "K" || (key === "Enter" && nothingFocused(event.target))) {
    event.preventDefault();
    togglePlay();
    return;
  }
  if (key === "c" || key === "C") {
    event.preventDefault();
    toggleSubtitles();
    return;
  }
  if (key === "m" || key === "M") {
    event.preventDefault();
    toggleMute();
    return;
  }
  if (key === "j" || key === "J" || key === "l" || key === "L") {
    event.preventDefault();
    skipBy(key.toLowerCase() === "j" ? -10 : 10);
    return;
  }
  if (key === "f" || key === "F") {
    event.preventDefault();
    toggleFullscreen();
    return;
  }
  if ((key === "n" || key === "N") && currentNext?.file) {
    event.preventDefault();
    const next = replayFromNext(currentNext);
    void postProgress(true).finally(() => {
      void startReplay(next, false);
    });
    return;
  }
  if (key === "Escape" && document.fullscreenElement === nodes.stage) {
    event.preventDefault();
    void document.exitFullscreen().catch(() => undefined);
    return;
  }
  // Arrows stay with the remote's focus navigation until there is a stream to control.
  if ((key === "ArrowUp" || key === "ArrowDown") && loadedPlaylist) {
    event.preventDefault();
    nudgeVolume(key === "ArrowUp" ? 0.05 : -0.05);
    return;
  }
  if (key === "ArrowLeft" || key === "ArrowRight") {
    // During a skip the press is kept and adds to it.
    if (!skipping && nodes.bar.disabled) return;
    event.preventDefault();
    skipBy(key === "ArrowLeft" ? -10 : 10);
  }
}

function listen(): void {
  abort?.abort();
  abort = new AbortController();
  const opts = { signal: abort.signal };
  document.addEventListener("fullscreenchange", () => syncFullscreen(), opts);
  document.addEventListener("keydown", onKey, opts);
  onScrubRelease(
    () => dragging,
    () => {
      dragging = false;
      nodes?.chrome.classList.remove("is-scrubbing");
      paintClock();
    },
    abort.signal,
  );
  // A killed tab can drop the last progress post. keepalive lets this one finish.
  window.addEventListener(
    "pagehide",
    () => {
      void postProgress(true, true);
    },
    opts,
  );
}

function bind(ui: Nodes): void {
  // Choices made on the dashboard or another TV apply here too.
  prefsOff?.();
  prefsOff = onPrefs(() => {
    const before = subtitlePrefsKey();
    prefs = loadPrefs();
    applyPrefs();
    if (savingPrefs || subtitlePrefsKey() === before) return;
    // Subtitles turned on or off, or a language picked, on another screen: that wins over this item's pick.
    subPick = null;
    clearSubNote();
    paintSubtitleChoices();
    applySubtitleMode();
  });
  void syncPrefs();
  ui.quality.value = prefs.quality;
  ui.auto.checked = prefs.autoplay;
  ui.volume.value = String(prefs.volume);
  ui.video.volume = prefs.volume;
  ui.quality.addEventListener("change", () => {
    const value = ui.quality.value;
    prefs.quality = isQuality(value) ? value : "auto";
    savePrefs();
    if (!currentReplay?.file || !currentJob) return;
    const again = currentReplay;
    const at = sourcePosition();
    void postProgress(true).finally(() => {
      void startReplay(again, false, at);
    });
  });
  ui.audio.addEventListener("change", () => {
    if (fillingAudio) return;
    audioPending = true;
    window.clearTimeout(audioTimer);
    audioTimer = window.setTimeout(() => void applyAudio(), AUDIO_SETTLE);
  });
  ui.subs.addEventListener("change", () => {
    if (fillingSubs) return;
    pickSubtitle(ui.subs.value);
  });
  ui.cc.addEventListener("click", () => toggleSubtitles());
  ui.auto.addEventListener("change", () => {
    prefs.autoplay = ui.auto.checked;
    savePrefs();
  });
  ui.volume.addEventListener("input", () => {
    prefs.volume = clampVolume(Number(ui.volume.value));
    ui.video.volume = prefs.volume;
    ui.video.muted = false;
    savePrefs();
    paintVolume();
  });
  ui.mute.addEventListener("click", () => toggleMute());
  ui.back.addEventListener("click", () => skipBy(-10));
  ui.forward.addEventListener("click", () => skipBy(10));
  ui.bigPlay.addEventListener("click", () => togglePlay());
  dropFocusAfterClick(ui.stage);
  ui.stage.addEventListener("pointermove", () => wake());
  ui.stage.addEventListener("pointerdown", () => wake());
  ui.stage.addEventListener("focusin", () => wake());
  ui.bar.addEventListener("input", () => {
    dragging = true;
    ui.chrome.classList.add("is-scrubbing");
    const at = clampSkip(Number(ui.bar.value), progressDuration());
    ui.elapsed.textContent = formatClock(at);
    paintSeekText(ui.bar, seekValueText(at, Number(ui.bar.max)));
    paintScrubFill(ui.bar);
  });
  ui.bar.addEventListener("change", () => {
    dragging = false;
    ui.chrome.classList.remove("is-scrubbing");
    void skipTo(Number(ui.bar.value));
  });
  ui.scrub.addEventListener("pointermove", (event) => {
    hoverX = event.clientX;
    placeHover(event.clientX);
  });
  ui.scrub.addEventListener("pointerleave", () => hideHover());
  ui.scrub.addEventListener("pointercancel", () => hideHover());
  ui.play.addEventListener("click", () => togglePlay());
  ui.full.addEventListener("click", () => toggleFullscreen());
  onVideoClick(ui.video, () => togglePlay(), () => toggleFullscreen());
  ui.video.addEventListener("play", () => {
    syncPlayButton();
    postPlace(true);
  });
  ui.video.addEventListener("playing", () => {
    started = true;
    buffering = false;
    syncPlayButton();
  });
  ui.video.addEventListener("waiting", () => {
    if (!attached) return;
    buffering = true;
    paintStage();
  });
  ui.video.addEventListener("canplay", () => {
    buffering = false;
    paintStage();
  });
  ui.video.addEventListener("pause", () => {
    syncPlayButton();
    if (skipping) return;
    postPlace(true);
    void postProgress(true);
  });
  ui.video.addEventListener("volumechange", () => paintVolume());
  ui.video.addEventListener("progress", () => paintClock());
  ui.video.addEventListener("timeupdate", () => paintClock());
  ui.video.addEventListener("loadedmetadata", () => paintClock());
  ui.video.addEventListener("durationchange", () => paintClock());
  ui.video.addEventListener("ended", () => {
    syncPlayButton();
    const next = currentNext;
    // Mark this episode before the next one starts, including when nothing follows.
    void (async () => {
      postPlace(true);
      // A save that fails here must not keep the next episode from starting.
      await postProgress(true).catch(() => undefined);
      if (!next?.file) return;
      await syncPrefs();
      if (prefs.autoplay && currentNext === next && active) void startReplay(replayFromNext(next), false);
    })();
  });
  ui.links.addEventListener("click", () => {
    if (!currentReplay?.file) return;
    const again = currentReplay;
    const at = sourcePosition();
    void postProgress(true).finally(() => {
      void startReplay(again, true, at);
    });
  });
  ui.next.addEventListener("click", () => {
    if (!currentNext?.file) return;
    const next = replayFromNext(currentNext);
    void postProgress(true).finally(() => {
      void startReplay(next, false);
    });
  });
  ui.close.addEventListener("click", () => {
    void closeCast();
  });
  paintSubtitleState();
  paintLinks();
  paintQuality();
  paintAudio(null);
  paintVolume();
  syncPlayButton();
  syncFullscreen();
}

function ensureRoot(): void {
  if (nodes && document.body.contains(nodes.root)) return;
  const root = document.createElement("div");
  root.id = "cast-root";
  root.className = "cast-page";
  root.innerHTML = SHELL;
  document.body.appendChild(root);
  const ui: Nodes = {
    root,
    stage: must(root, "cast-stage"),
    video: must(root, "cast-video"),
    presence: must(root, "cast-presence"),
    note: must(root, "cast-note"),
    title: must(root, "cast-title"),
    episode: must(root, "cast-episode"),
    stageTitle: must(root, "cast-stage-title"),
    stageStatus: must(root, "cast-stage-status"),
    bigPlay: must(root, "cast-big-play"),
    status: must(root, "cast-status"),
    play: must(root, "cast-play"),
    back: must(root, "cast-back"),
    forward: must(root, "cast-forward"),
    mute: must(root, "cast-mute"),
    elapsed: must(root, "cast-elapsed"),
    total: must(root, "cast-total"),
    bar: must(root, "cast-bar"),
    hover: must(root, "cast-hover"),
    scrub: must(root, "cast-scrub"),
    chrome: must(root, "cast-chrome"),
    full: must(root, "cast-full"),
    quality: must(root, "cast-quality"),
    qualityLabel: must(root, "cast-quality-label"),
    audio: must(root, "cast-audio"),
    audioLabel: must(root, "cast-audio-label"),
    subs: must(root, "cast-subs"),
    cc: must(root, "cast-cc"),
    links: must(root, "cast-links"),
    next: must(root, "cast-next"),
    auto: must(root, "cast-auto"),
    volume: must(root, "cast-volume"),
    close: must(root, "cast-close"),
  };
  ui.presence.textContent = receiver.name;
  nodes = ui;
  bind(ui);
}

function ensureThemes(): void {
  const menu = document.querySelector("#theme-menu");
  if (!(menu instanceof HTMLElement) || menu.childElementCount > 0) return;
  mountThemes();
}

/** Hide the library dock so its keyboard shortcuts do not fight the TV. */
function tuckControllerDock(hide: boolean): void {
  const dock = document.getElementById("dock");
  const media = document.getElementById("player-video");
  if (!(dock instanceof HTMLElement)) return;
  if (hide) {
    if (dock.classList.contains("open")) dock.dataset.castHid = "1";
    else delete dock.dataset.castHid;
    dock.classList.remove("open");
    if (media instanceof HTMLVideoElement && !media.paused) media.pause();
    return;
  }
  if (dock.dataset.castHid === "1") {
    delete dock.dataset.castHid;
    dock.classList.add("open");
  }
}

function clearTimer(id: number): number {
  if (id) window.clearInterval(id);
  return 0;
}

export function syncCast(on: boolean): void {
  if (on) {
    if (active) return;
    prefs = loadPrefs();
    ensureThemes();
    ensureRoot();
    applyPrefs();
    active = true;
    document.body.classList.add("cast-mode");
    tuckControllerDock(true);
    listen();
    void pulsePresence();
    void pollSession();
    presenceTimer = window.setInterval(() => void pulsePresence(), 5000);
    sessionTimer = window.setInterval(() => void pollSession(), 1000);
    return;
  }
  if (!active && !document.getElementById("cast-root")) return;
  void postProgress(true, true);
  active = false;
  presenceTimer = clearTimer(presenceTimer);
  sessionTimer = clearTimer(sessionTimer);
  abort?.abort();
  abort = null;
  holdSession = false;
  stopLocal();
  nodes?.root.remove();
  nodes = null;
  tuckControllerDock(false);
  document.body.classList.remove("cast-mode");
}
