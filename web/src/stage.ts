/**
 * Behaviour shared by the dock player and the TV cast page: hls.js with
 * recovery, the loaded-ahead band on the seek bar, controls that fade in
 * fullscreen, subtitles that make room for them, and focus that does not
 * stick to a control after a mouse click.
 */

/** A segment as hls.js reports it. `stats.loaded` grows as the bytes arrive. */
type HlsFragment = { stats?: { loaded?: number } };

export type HlsErrorData = {
  fatal?: boolean;
  type?: string;
  details?: string;
  /** Present on fragment events. */
  frag?: HlsFragment;
};

type HlsLevel = { details?: { requestScheduled?: number } | null };

export type HlsInstance = {
  loadSource(url: string): void;
  attachMedia(media: HTMLMediaElement): void;
  on(event: string, handler: (event: string, data: HlsErrorData) => void): void;
  startLoad(startPosition?: number): void;
  stopLoad(): void;
  recoverMediaError(): void;
  destroy(): void;
  levels: HlsLevel[] | null;
  loadLevel: number;
};

type RetryPolicy = {
  maxNumRetry: number;
  retryDelayMs: number;
  maxRetryDelayMs: number;
};

type LoadPolicy = {
  default: {
    maxTimeToFirstByteMs: number;
    maxLoadTimeMs: number;
    timeoutRetry: RetryPolicy;
    errorRetry: RetryPolicy;
  };
};

type HlsConfig = {
  startPosition?: number;
  liveDurationInfinity?: boolean;
  lowLatencyMode?: boolean;
  maxLiveSyncPlaybackRate?: number;
  maxMaxBufferLength?: number;
  backBufferLength?: number;
  maxBufferHole?: number;
  highBufferWatchdogPeriod?: number;
  nudgeMaxRetry?: number;
  appendTimeout?: number;
  fragLoadPolicy?: LoadPolicy;
  playlistLoadPolicy?: LoadPolicy;
};

type HlsStatic = {
  isSupported(): boolean;
  Events?: { ERROR?: string; FRAG_BUFFERED?: string; FRAG_LOADING?: string; FRAG_LOADED?: string };
  ErrorTypes?: { NETWORK_ERROR?: string; MEDIA_ERROR?: string };
  new (config?: HlsConfig): HlsInstance;
};

declare global {
  interface Window {
    Hls?: HlsStatic;
  }
}

declare const __PORTAL_VERSION__: string;

let hlsLoad: Promise<void> | null = null;
let hlsSettled = false;

/**
 * hls.js is large and only playback needs it, so it loads on the first play
 * (versioned, so the browser keeps it until Portal updates). If it fails,
 * attachHls falls back to the browser's own HLS as before.
 */
export function loadHls(): Promise<void> {
  hlsLoad ??= new Promise<void>((resolve) => {
    const script = document.createElement("script");
    script.src = `/vendor/hls.min.js?v=${encodeURIComponent(__PORTAL_VERSION__)}`;
    script.onload = script.onerror = () => {
      hlsSettled = true;
      resolve();
    };
    document.head.append(script);
  });
  return hlsLoad;
}

/** hls.js has loaded, or failed to, so attachHls can run now. */
export function hlsReady(): boolean {
  return hlsSettled;
}

const HLS_RECOVERIES = 6;
/** A pause this long may leave the playlist stale; playing again reloads it. */
const RESUME_IDLE_MS = 3000;
/** How long playback may sit with nothing ahead, and its download still, before the loader is restarted. */
const STALL_KICK_MS = 2500;
/** Leave a restart alone long enough for one local segment to arrive. */
const KICK_COOLDOWN_MS = 8000;

/** The native-HLS listeners of the stream last attached to each video, dropped on the next attach. */
const nativeWatch = new WeakMap<HTMLVideoElement, AbortController>();
/** Resume and stall listeners for the hls.js stream last attached to each video. */
const hlsWatch = new WeakMap<HTMLVideoElement, AbortController>();

/**
 * A dead socket has to fail while forward buffer remains. The stock fragment
 * timeout is two minutes, which is longer than that buffer, so the wait
 * lands at the cliff and lasts longer than opening a new title.
 */
function quickLoad(maxTimeToFirstByteMs: number, maxLoadTimeMs: number): LoadPolicy {
  return {
    default: {
      maxTimeToFirstByteMs,
      maxLoadTimeMs,
      timeoutRetry: { maxNumRetry: 4, retryDelayMs: 250, maxRetryDelayMs: 1000 },
      errorRetry: { maxNumRetry: 3, retryDelayMs: 250, maxRetryDelayMs: 1000 },
    },
  };
}

/**
 * Plays `playlist` in `media`. A dropped connection or a decode error is
 * retried a few times before `onFatal` reports it. Returns null when the
 * browser plays HLS itself.
 */
export function attachHls(
  media: HTMLVideoElement,
  playlist: string,
  onFatal: (detail: string) => void,
): HlsInstance | null {
  const Ctor = window.Hls;
  // An earlier stream's once-listeners never fired if it played cleanly; they
  // must not pile up across plays and answer for this one.
  nativeWatch.get(media)?.abort();
  nativeWatch.delete(media);
  hlsWatch.get(media)?.abort();
  hlsWatch.delete(media);
  // Portal's playlists grow while ffmpeg works (EVENT type). Players treat
  // that as live and join near the newest segment, which skips whatever
  // loaded before the player attached; every stream starts at 0 instead.
  if (!Ctor || typeof Ctor.isSupported !== "function" || !Ctor.isSupported()) {
    const watch = new AbortController();
    nativeWatch.set(media, watch);
    media.addEventListener(
      "loadedmetadata",
      () => {
        if (media.currentTime > 0) media.currentTime = 0;
      },
      { once: true, signal: watch.signal },
    );
    media.src = playlist;
    // The browser plays HLS itself (Safari): a failure is its media error, once.
    media.addEventListener(
      "error",
      () => {
        if (media.getAttribute("src") === playlist) onFatal("Playback failed.");
      },
      { once: true, signal: watch.signal },
    );
    return null;
  }
  const player = new Ctor({
    startPosition: 0,
    // The playlist grows while ffmpeg works. Low-latency mode treats that as
    // a live edge and asks for pieces that are not on disk yet. Stay at 1x
    // behind the edge so a cushion of media is not thrown away to catch up.
    lowLatencyMode: false,
    maxLiveSyncPlaybackRate: 1,
    // Until ffmpeg writes EXT-X-ENDLIST the playlist edge is not the end of
    // the film. Pinning MediaSource duration there makes the element stall
    // once playback reaches the buffer, and raising it afterwards is slow.
    liveDurationInfinity: true,
    // A bitrate guess would otherwise buffer for minutes. One minute is
    // enough runway for the next segment, which is already on disk.
    maxMaxBufferLength: 60,
    // Played media is evicted so a long sitting session cannot fill the
    // buffer quota and then stall on the next append.
    backBufferLength: 60,
    maxBufferHole: 0.5,
    highBufferWatchdogPeriod: 1,
    nudgeMaxRetry: 5,
    // A stuck append at the cliff gives up. With buffer ahead, hls.js waits
    // at least as long as that buffer, so a healthy append is left alone.
    appendTimeout: 8000,
    // A copied picture is cut on the file's own keyframes, so one segment can
    // be ten seconds of the original and tens of MB over Wi-Fi.
    // Local segments are usually instant. A late one, or one still being
    // renamed in, needs longer than a few seconds and a couple of instant retries.
    fragLoadPolicy: quickLoad(12000, 60000),
    playlistLoadPolicy: quickLoad(8000, 15000),
  });
  player.loadSource(playlist);
  player.attachMedia(media);
  watchResume(player, media, Ctor);
  const errorEvent = Ctor.Events?.ERROR;
  if (errorEvent) {
    let recoveries = 0;
    // Retries are per trouble spot: video loading again earns them back, so a
    // long film survives more than a few network blips.
    // Buffered, not merely downloaded: a fragment that downloads but never
    // decodes must not earn the retries back, or recovery would loop.
    const loaded = Ctor.Events?.FRAG_BUFFERED;
    if (loaded) player.on(loaded, () => {
      recoveries = 0;
    });
    player.on(errorEvent, (_event, data) => {
      if (!data?.fatal) return;
      if (recoveries < HLS_RECOVERIES) {
        if (data.type && data.type === Ctor.ErrorTypes?.NETWORK_ERROR) {
          recoveries += 1;
          player.startLoad();
          return;
        }
        if (data.type && data.type === Ctor.ErrorTypes?.MEDIA_ERROR) {
          recoveries += 1;
          player.recoverMediaError();
          return;
        }
      }
      onFatal(data.details || "Playback failed.");
    });
  }
  return player;
}

/**
 * After a pause, pull the playlist again. When playback then sits with nothing
 * ahead and its download has stopped moving, drop that fetch and load again.
 * `startLoad(-1)` keeps the playhead and the bytes already buffered, but it
 * starts the next segment's sound afresh, which can tick, so a download that
 * is still arriving is left alone. Seek and quality changes destroy this
 * player before the replacement stream plays.
 */
function watchResume(player: HlsInstance, media: HTMLVideoElement, Ctor: HlsStatic): void {
  const watch = new AbortController();
  hlsWatch.set(media, watch);
  let alive = true;
  let played = false;
  let pausedAt = 0;
  let lastKick = 0;
  let stallTimer = 0;
  // The segment being downloaded, and how many of its bytes had arrived when
  // last checked. hls.js gives the segment its loader's stats only after
  // FRAG_LOADING, so the count is read through the segment each time.
  let fetching: HlsFragment | null = null;
  let fetchSeen = 0;
  const fetchedBytes = (): number => fetching?.stats?.loaded ?? 0;
  const loading = Ctor.Events?.FRAG_LOADING;
  const loaded = Ctor.Events?.FRAG_LOADED;
  if (loading) player.on(loading, (_event, data) => {
    fetching = data?.frag ?? null;
    fetchSeen = 0;
  });
  if (loaded) player.on(loaded, (_event, data) => {
    if (data?.frag === fetching) fetching = null;
  });
  const fetchMoving = (): boolean => {
    const bytes = fetchedBytes();
    if (!fetching || bytes <= fetchSeen) return false;
    fetchSeen = bytes;
    return true;
  };
  const refreshPlaylist = (): void => {
    const level = player.levels?.[player.loadLevel];
    if (level?.details) level.details.requestScheduled = 0;
  };
  const kick = (): void => {
    if (!alive) return;
    const now = performance.now();
    if (now - lastKick < KICK_COOLDOWN_MS) return;
    lastKick = now;
    refreshPlaylist();
    fetching = null;
    player.startLoad(-1);
  };
  const watchStall = (): void => {
    window.clearTimeout(stallTimer);
    stallTimer = window.setTimeout(() => {
      if (!alive || media.paused || media.ended) return;
      if (media.readyState >= HTMLMediaElement.HAVE_FUTURE_DATA) return;
      // Still stuck after a restart (or inside its cooldown): look again.
      if (!fetchMoving()) kick();
      watchStall();
    }, STALL_KICK_MS);
  };
  const stop = player.destroy.bind(player);
  let stopped = false;
  const release = (): void => {
    alive = false;
    window.clearTimeout(stallTimer);
  };
  watch.signal.addEventListener("abort", release);
  player.destroy = () => {
    if (stopped) return;
    stopped = true;
    release();
    watch.abort();
    stop();
  };
  media.addEventListener(
    "pause",
    () => {
      pausedAt = performance.now();
      window.clearTimeout(stallTimer);
    },
    { signal: watch.signal },
  );
  media.addEventListener(
    "play",
    () => {
      const idle = pausedAt > 0 ? performance.now() - pausedAt : 0;
      pausedAt = 0;
      if (idle >= RESUME_IDLE_MS && alive) refreshPlaylist();
    },
    { signal: watch.signal },
  );
  media.addEventListener(
    "playing",
    () => {
      played = true;
      window.clearTimeout(stallTimer);
    },
    { signal: watch.signal },
  );
  media.addEventListener(
    "waiting",
    () => {
      window.clearTimeout(stallTimer);
      // The opening segment is still being made; restarting it only delays the start.
      if (!played || media.paused) return;
      fetchSeen = fetchedBytes();
      watchStall();
    },
    { signal: watch.signal },
  );
}

/**
 * Drops every subtitle track. Chrome can keep drawing a removed track's cue,
 * doubling the text after a seek, unless the track is disabled first.
 */
export function removeTracks(media: HTMLVideoElement): void {
  for (const track of Array.from(media.textTracks)) track.mode = "disabled";
  for (const node of Array.from(media.querySelectorAll("track"))) node.remove();
}

/**
 * How long the element can stand in for a duration. A growing transcode
 * reports Infinity until the playlist ends; the seekable edge is how far
 * ffmpeg has written.
 */
export function playedSpan(media: HTMLVideoElement): number {
  const duration = media.duration;
  if (Number.isFinite(duration) && duration > 0) return duration;
  const ranges = media.seekable;
  if (!ranges.length) return 0;
  const end = ranges.end(ranges.length - 1);
  return Number.isFinite(end) && end > 0 ? end : 0;
}

/** Writes the seek bar's spoken value only when it changes, since the clock repaints several times a second. */
export function paintSeekText(bar: HTMLInputElement, text: string): void {
  if (bar.getAttribute("aria-valuetext") !== text) bar.setAttribute("aria-valuetext", text);
}

/**
 * A drag that ends where it began fires no change event, which would leave
 * the page thinking the bar is still held and its clock frozen. `settle` runs
 * after the release once the change event, if any, has had its turn.
 */
export function onScrubRelease(stillDragging: () => boolean, settle: () => void, signal?: AbortSignal): void {
  const release = (): void => {
    if (!stillDragging()) return;
    window.setTimeout(() => {
      if (stillDragging()) settle();
    });
  };
  document.addEventListener("pointerup", release, { signal });
  document.addEventListener("pointercancel", release, { signal });
}

/** How far ahead the player has loaded, as a lighter band on the seek bar. */
export function paintBuffered(bar: HTMLInputElement, media: HTMLVideoElement, origin: number, max: number): void {
  let end = 0;
  for (let i = 0; i < media.buffered.length; i += 1) {
    if (media.buffered.start(i) <= media.currentTime + 1) end = Math.max(end, media.buffered.end(i));
  }
  const pct = max > 0 && end > 0 ? Math.min(100, Math.max(0, ((origin + end) / max) * 100)) : 0;
  bar.style.setProperty("--scrub-buffer", pct.toFixed(2) + "%");
}

const WAKE_MS = 2600;
const wakeTimers = new WeakMap<HTMLElement, number>();
const liftedCues = new WeakMap<HTMLElement, boolean>();

/**
 * Shows the controls. In fullscreen they fade again after a pause in
 * activity, unless `held` says something still needs them.
 */
export function wakeStage(stage: HTMLElement, media: HTMLVideoElement, held: () => boolean = () => false): void {
  stage.classList.add("is-awake");
  window.clearTimeout(wakeTimers.get(stage));
  const rest = (): void => {
    if (held() || stage.querySelector(".chrome:hover, .upnext:hover")) {
      wakeTimers.set(stage, window.setTimeout(rest, WAKE_MS));
      return;
    }
    stage.classList.remove("is-awake");
    placeCues(stage, media);
  };
  wakeTimers.set(stage, window.setTimeout(rest, WAKE_MS));
  placeCues(stage, media);
}

/** In fullscreen the controls float over the picture; subtitles move up while they show. */
export function placeCues(stage: HTMLElement, media: HTMLVideoElement, force = false): void {
  const shown = ["is-awake", "is-paused", "is-busy"].some((name) => stage.classList.contains(name));
  const lift = stage.classList.contains("is-full") && shown;
  if (lift === (liftedCues.get(stage) ?? false) && !force) return;
  liftedCues.set(stage, lift);
  for (const track of Array.from(media.textTracks)) {
    for (const cue of Array.from(track.cues ?? [])) {
      if (!(cue instanceof VTTCue)) continue;
      if (lift) {
        cue.snapToLines = false;
        cue.lineAlign = "end";
        cue.line = 80;
      } else {
        cue.snapToLines = true;
        cue.lineAlign = "start";
        cue.line = "auto";
      }
    }
  }
}

/**
 * A mouse click must not leave focus on a control: Space would press it
 * again instead of pausing, and arrow keys would nudge the seek bar. Keyboard
 * and remote activation (detail 0) keep focus where the user put it.
 */
/** Play waits out a second click, so a double-click only toggles fullscreen. */
const CLICK_HOLD_MS = 220;

export function onVideoClick(media: HTMLVideoElement, play: () => void, fullscreen: () => void): void {
  let timer = 0;
  media.addEventListener("click", () => {
    window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = 0;
      play();
    }, CLICK_HOLD_MS);
  });
  media.addEventListener("dblclick", (event) => {
    event.preventDefault();
    window.clearTimeout(timer);
    timer = 0;
    fullscreen();
  });
}

export function dropFocusAfterClick(stage: HTMLElement): void {
  stage.addEventListener("click", (event) => {
    if (event.detail === 0) return;
    window.setTimeout(() => {
      const active = document.activeElement;
      if (active instanceof HTMLElement && stage.contains(active) && active.matches(".icon-btn, .stage-play, .seek-bar, .volume-bar, button")) {
        active.blur();
      }
    });
  });
}
