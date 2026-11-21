import {
  ApiError,
  api,
  artUrl,
  audioTracksOf,
  type AudioTrack,
  type CastDevice,
  type CastReceiver,
  type CastReplay,
  type CastSession,
  type CastSubtitle,
  type DeviceCast,
  seasonFoldersFor,
  settingsHref,
  type BrowseItem,
  type BrowsePage,
  type Episode,
  type MediaInfo,
  type NextItem,
  type PlayJob,
  type PlayKind,
  type Prompt,
  type Quality,
  type SubtitleCandidate,
  type TitleDetail,
  type SubtitleSettings,
  type SubtitleTrack,
} from "./api";
import { activityView, beginActivity, describePlay, endActivity, setNotice } from "./activity";
import { clampSkip, formatClock, seekValueText } from "./player-time";
import {
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
  type HlsInstance,
} from "./stage";
import { cleanEpisodeTitle, displayMediaTitle, episodeCode, splitTitle } from "./titles";
import { closeInfo, infoOpen, openInfo } from "./info";
import { bindSavedButton, loadSaved, wireSavedButton, type SaveTitle } from "./saved";
import {
  DELAY_STEP,
  candidateDetail,
  candidateTitle,
  clampDelay,
  cleanResults,
  delayLabel,
  onlineTrackUrl,
  preferredLanguages,
  autoSubtitle,
  preferredTrack,
  sameLanguage,
  searchFor,
  shiftCues,
  startupAudioLanguage,
  subtitleSettings,
  trackLanguage,
  trackName,
  languageName,
} from "./subtitles";
import { QUALITIES, clampVolume, onPrefs, prefs, setPrefs, syncPrefs } from "./prefs";

export type PlayTarget = {
  kind: PlayKind;
  id: string;
  file: string;
  title: string;
  season?: number;
  episode?: number;
  art?: string | null;
  showTitle?: string | null;
  showId?: string | null;
  /** What browse knew about the title, so Info opens at once. */
  info?: MediaInfo | null;
  resumeSeconds?: number | null;
  /** When true, show the add-on source list instead of auto-resolving. */
  manualLinks?: boolean;
  progressKind: "movie" | "tv";
  progressId: string;
  progressName: string;
  trackProgress: boolean;
};

/** Opens a folder page; `picking` is set when the folder is an add-on's source list for that item. */
let onBrowse: (path: string, picking?: PlayTarget) => void = () => {};
let mounted = false;
let generation = 0;
let skipGeneration = 0;
let currentJob: string | null = null;
let castReceiverId: string | null = null;
let castReceiverName = "";
let castMode: "browser" | "device" | null = null;
/** Tracks offered for the playing file, and the one ffmpeg is mapping. */
let currentAudio: AudioTrack[] = [];
let currentAudioIndex: number | null = null;
let audioListKey = "";
/** True while the menu is being filled, so that write does not start a new stream. */
let fillingAudio = false;
/** A pick is waiting out the arrow-key settle, so a repaint must not put the old track back. */
let audioPending = false;
let audioTimer = 0;
/** Where the next ready job goes when a new item starts mid-cast (Next, quality, links). */
let pendingRecast: CastChoice | null = null;
/** The current item's start failed (no stream, or Portal did not answer); Play tries again. */
let startFailed = false;
let devicePaused = false;
let castBusy = false;
let castWatch = 0;
/** Bumped to cancel an in-flight Cast menu open (e.g. Subtitles opened meanwhile). */
let castMenuGen = 0;
let currentTarget: PlayTarget | null = null;
let currentNext: NextItem | null = null;
let currentSubs: SubtitleTrack[] = [];
/** The job as it last became ready, for the subtitle search. */
let lastReadyJob: PlayJob | null = null;
let hls: HlsInstance | null = null;
let origin = 0;
let jobDuration = 0;
let lastPost = 0;
let ticker = 0;
let shownPrompt = "";
let dragging = false;
let hoverX: number | null = null;
let skipping = false;
/** How long the Quality menu must sit still before the stream restarts. */
const QUALITY_SETTLE = 600;
/** Where the server seek in flight lands, and where further ±10 presses meanwhile add up to. */
let skipTarget = 0;
let queuedSkip: number | null = null;
/** The same for a Cast device: a seek on the TV in flight, its target, and presses meanwhile. */
let deviceSeeking = false;
let deviceSeekTarget = 0;
let deviceQueued: number | null = null;
/** When the TV was asked to reload at a new place; 0 once it is there. */
let deviceLandSince = 0;
/** How long a TV may take to show a seek before the next press goes anyway. */
const DEVICE_LAND_MS = 8000;
let sawProgress = false;
let canceling = false;
let cancelPromptId = "";
let playTicket = 0;
/** hls/src attached for the current job, and whether a frame has played since. */
let attached = false;
let started = false;
let buffering = false;
/** Next-episode file whose up-next card was dismissed. */
let upNextDismissed = "";
/** Cancel for the resolve in progress, mirrored onto the stage. */
let stageCancel: (() => void) | null = null;
/** Re-applies the subtitle choice once the next stream has data. */
let pendingSubtitleApply: (() => void) | null = null;
/** The device cast as Portal last reported it, and when that answer came. */
let deviceView: DeviceCast | null = null;
let deviceSeenAt = 0;
/** When Portal's view of the TV last changed, on this page's clock. */
let deviceFreshAt = 0;
/** A TV that has not reported for this long is not assumed to play on. */
const DEVICE_STALE_MS = 15_000;
/** A Play or Pause sent to the TV, which its next reports may not show yet. */
let deviceWant: { paused: boolean; since: number } | null = null;
const DEVICE_WANT_MS = 8000;
/** The line the cast last wrote, so an error or a subtitle note stays until the TV's state changes. */
let deviceLine = "";
/** The TV's volume as set from here, until the TV reports it back. */
let tvVolume: { level: number; muted: boolean; since: number } | null = null;
let tvVolumeSend: { level?: number; muted?: boolean } = {};
let tvVolumeTimer = 0;
/** The Cast menu's device search; TV controls still work meanwhile. */
let castScanning = false;
let deviceWatch = 0;
let deviceClock = 0;
let receiverWatch = 0;
/** Where a cast was when it ended, so Play here carries on from there. */
let deviceLeftAt = 0;
/** Where the browser TV is, and when that report was made. The clock moves on from it. */
type BrowserPlace = { position: number; duration: number; paused: boolean; updatedMs: number };
let browserPlace: BrowserPlace | null = null;
let browserSeenAt = 0;
let browserClock = 0;
let browserSeeking = false;
let browserSeekTarget = 0;
let browserSeekSince = 0;
let browserQueued: number | null = null;

function el<T extends HTMLElement>(id: string): T {
  const node = document.getElementById(id);
  if (!node) throw new Error("Missing " + id);
  return node as T;
}

function video(): HTMLVideoElement {
  return el<HTMLVideoElement>("player-video");
}

/** Skip the DOM write when timeupdate repaints the same clock text. */
function setText(node: HTMLElement, text: string): void {
  if (node.textContent !== text) node.textContent = text;
}

/**
 * The player's messages, errors included, live in the dock (and on the stage
 * before video). They are not also put in the page notice, which would show
 * the same words twice and outlast the player once it closes.
 */
function setStatus(text: string): void {
  // Unchanged text is not written again, so screen readers do not repeat it.
  setText(el("dock-status"), text);
  setText(el("dock-stage-status"), text);
  paintStage();
}

/**
 * A long wait for video can mean Portal's transcode stopped (the network
 * dropped mid-play); the job then says why. Asked once per stall.
 */
function checkStall(): void {
  const token = generation;
  const jobId = currentJob;
  if (!jobId || !attached) return;
  window.setTimeout(() => {
    if (token !== generation || currentJob !== jobId || !attached || !buffering) return;
    void api
      .job(jobId)
      .then((job) => {
        if (token !== generation || currentJob !== jobId || !buffering || job.state !== "failed") return;
        deviceLeftAt = sourcePosition();
        destroyHls();
        attached = false;
        started = false;
        buffering = false;
        const text = (job.error || "The stream stopped.") + " Press Play to try again.";
        setStatus(text);
        syncPlayButton();
      })
      .catch(() => undefined);
  }, 8000);
}

function failStart(text: string): void {
  startFailed = true;
  const where = pendingRecast ? " on " + pendingRecast.name : "";
  const shown = /try again/i.test(text) ? text : text.replace(/\s*$/, "") + " Press Play to try again" + where + ".";
  setStatus(shown);
  syncPlayButton();
}

/** hls.js gives codes like "fragLoadError"; people get a sentence. */
function streamFailure(detail: string): string {
  if (/load|timeout/i.test(detail)) return "The stream stopped loading. Press Play to try again.";
  return "This video could not be played here. Press Play to try again.";
}

function placeCues(force = false): void {
  placeStageCues(el("dock-stage"), video(), force);
}

/** Shows the controls; in fullscreen they fade again after a pause in activity. */
function wake(): void {
  wakeStage(el("dock-stage"), video(), () => dragging || subtitleMenuOpen());
}

/** Spinner while resolving, loading or buffering; a play button while paused. */
function paintStage(): void {
  const stage = document.getElementById("dock-stage");
  if (!stage) return;
  const media = video();
  const active = !!currentTarget && castMode !== "device";
  const busy = active && (playTicket !== 0 || (attached && !started) || buffering || skipping);
  const paused = active && started && !busy && (media.paused || media.ended);
  const text = el("dock-stage-status").textContent?.trim() ?? "";
  const away = castingAway();
  stage.classList.toggle("is-busy", busy);
  stage.classList.toggle("is-paused", paused);
  stage.classList.toggle("has-message", !!currentTarget && !started && !!text && !away);
  stage.classList.toggle("is-idle", !currentTarget);
  stage.classList.toggle("is-casting", away);
  paintCastStage(away);
  placeCues();
}

/** While a TV plays, the stage says where, over the title's art, instead of showing an empty picture. */
function paintCastStage(away: boolean): void {
  const panel = el("dock-stage-cast");
  const paused = castMode === "device" ? devicePaused : (browserPlace?.paused ?? false);
  setText(el("dock-stage-cast-lead"), paused ? "Paused on" : "Playing on");
  setText(el("dock-stage-cast-name"), away ? castReceiverName || "the TV" : "");
  const art = away ? artUrl(currentTarget?.art) : null;
  const image = art ? 'url("' + art.replace(/["\\]/g, "\\$&") + '")' : "none";
  if (panel.style.getPropertyValue("--cast-art") !== image) panel.style.setProperty("--cast-art", image);
  // Fullscreen and picture-in-picture would only enlarge that panel.
  el<HTMLButtonElement>("dock-full").disabled = away && document.fullscreenElement !== el("dock-stage");
  el<HTMLButtonElement>("dock-pip").disabled = away;
}

function playLinks(): boolean {
  return !!currentTarget?.manualLinks;
}

function showPlay(
  input: Parameters<typeof describePlay>[0],
  onCancel?: (() => void) | null,
  cancelDisabled = false,
): void {
  // Idle dock / canceled generation must not keep resolve/play loading copy.
  if (!currentTarget) {
    clearPlayActivity();
    return;
  }
  const described = describePlay({ ...input, links: input.links ?? playLinks() });
  playTicket = beginActivity("play", described, { onCancel, cancelDisabled });
  stageCancel = onCancel ?? null;
  const cancel = el<HTMLButtonElement>("dock-stage-cancel");
  cancel.hidden = !stageCancel;
  cancel.disabled = cancelDisabled;
  setStatus(activityView(described).status);
}

function clearPlayActivity(): void {
  const ticket = playTicket;
  if (!ticket) return;
  playTicket = 0;
  endActivity(ticket);
  stageCancel = null;
  el("dock-stage-cancel").hidden = true;
  paintStage();
  // The episode list waits out the resolve. A release clears episodeWait first, so teardown does not browse.
  if (episodeWait && !holdEpisodes) {
    episodeWait = false;
    requestEpisodes();
  }
}

function setDockTitle(title: string): void {
  const node = el("dock-title");
  const line = el("dock-episode");
  const split = splitTitle(title, currentTarget?.season, currentTarget?.episode, currentTarget?.showTitle);
  const episode = currentTarget?.season != null && !!currentTarget?.episode;
  const heading = episode ? split.heading : humanTitle(title) || title.trim();
  node.textContent = heading;
  line.textContent = split.detail;
  line.hidden = !split.detail;
  const full = split.detail ? heading + " · " + split.detail : heading;
  el("dock-stage-title").textContent = full;
  if (full) node.title = full;
  else node.removeAttribute("title");
}

/** What the side panel's Watchlist and Bookmark save: the movie, or an episode's show. */
function saveTarget(target: PlayTarget | null): SaveTitle | null {
  if (!target) return null;
  if (target.kind === "episode") {
    const id = target.showId || target.id;
    const name = humanTitle(target.showTitle) || humanTitle(target.progressName) || "";
    return id && name ? { id, kind: "tv", name, art: target.art ?? null } : null;
  }
  if (target.kind === "movie") {
    const name = humanTitle(target.title) || target.title.trim();
    const info = target.info;
    return target.id && name
      ? {
          id: target.id,
          kind: "movie",
          name,
          art: target.art ?? null,
          year: info?.year ?? null,
          imdb_id: info?.imdb_id ?? null,
        }
      : null;
  }
  return null;
}

/** What Info shows for the target before the lookup answers. */
function infoSeed(target: PlayTarget): MediaInfo {
  const episode = target.kind === "episode" && target.season != null && !!target.episode;
  const show = humanTitle(target.showTitle);
  const base: MediaInfo = episode
    ? {
        kind: "episode",
        title: cleanEpisodeTitle(target.title, target.season, target.episode, show) || target.title,
        show_title: show || null,
        season: target.season,
        episode: target.episode,
      }
    : { kind: target.kind === "movie" ? "movie" : null, title: humanTitle(target.title) || target.title };
  return { ...base, ...(target.info ?? {}) };
}

/** Points the side panel's Watchlist, Bookmark, and Info at what is playing. */
function paintSaveButtons(): void {
  const target = currentTarget;
  const save = saveTarget(target);
  bindSavedButton(el<HTMLButtonElement>("dock-watchlist"), save);
  bindSavedButton(el<HTMLButtonElement>("dock-bookmark"), save);
  const info = el<HTMLButtonElement>("dock-info");
  info.disabled = !target;
  const name = target ? humanTitle(target.title) || target.title : "";
  info.title = name ? `Info about “${name}”` : "Info";
  if (save) void loadSaved([save.id], () => currentTarget === target);
}

function openTargetInfo(): void {
  const target = currentTarget;
  if (!target) return;
  openInfo({ seed: infoSeed(target), art: target.art, save: saveTarget(target), opener: el("dock-info") });
}

function showDock(title: string): void {
  el("dock").classList.add("open");
  setDockTitle(title);
  document.body.classList.add("playing");
  applyTheater();
}

function hideDock(): void {
  // A fullscreen stage or a picture-in-picture window would otherwise stay up, empty.
  if (document.fullscreenElement === el("dock-stage")) void document.exitFullscreen().catch(() => undefined);
  if (document.pictureInPictureElement === video()) void document.exitPictureInPicture().catch(() => undefined);
  el("dock").classList.remove("open");
  setMini(false);
  document.body.classList.remove("playing");
  // theaterWanted stays, so the next item in this dock session opens wide again.
  applyTheater();
  setStatus("");
  setDockTitle("");
}

/** Keeps the page clear of the fixed player. The dock's height changes with the title, status, and phone wrapping. */
function fitPlayingPadding(): void {
  placeTheaterHead();
  const dock = document.getElementById("dock");
  // Theater already covers the page under the header, so the body must not grow a second time.
  if (document.body.classList.contains("theater")) {
    document.body.style.paddingBottom = "";
    return;
  }
  if (!dock || !document.body.classList.contains("playing") || !dock.classList.contains("open")) {
    document.body.style.paddingBottom = "";
    return;
  }
  const cover = Math.max(0, window.innerHeight - dock.getBoundingClientRect().top);
  document.body.style.paddingBottom = `${Math.ceil(cover) + 12}px`;
}

/** Theater stays for this dock session only. Closing the player drops the wide layout and keeps the wish. */
let theaterWanted = false;
/** The episode list starts open for a show. A movie has no list. */
let episodesOpen = true;
/** A resolve is in flight, so the list waits until Kodi is free. */
let episodeWait = false;
/** True from the start of a play until the dock is showing that item. Browse stays shut. */
let holdEpisodes = false;
/** Bumped so a newer play owns the hold and an older start does not clear it. */
let episodeHold = 0;
let episodeTicket = 0;
/** Show whose rows are on screen, the season those rows list, and the playing season last followed. */
let listedShow = "";
let listedSeason: number | null = null;
let followedSeason: number | null = null;
/** A season chip clicked while a resolve owns Kodi. Applied when the list is allowed to load. */
let queuedSeason: number | null = null;
/** Opening theater refetches once Kodi is free, without wiping a list that is already on screen. */
let reloadEpisodes = false;
let scrolledKey = "";
const episodeTitles = new Map<string, TitleDetail | null>();
const episodePages = new Map<string, BrowsePage>();

type SeasonChoice = { season: number; path: string };

type ListedEpisode = {
  showId: string;
  showName: string;
  file: string;
  season: number | null;
  episode: number;
  name: string;
  art?: string | null;
  info?: MediaInfo | null;
  position: number;
  duration: number;
  finished: boolean;
  resumable: boolean;
};

/** The show id an episode belongs to. Movies and loose files have none, so theater stays one column. */
function showIdOf(target: PlayTarget | null): string {
  if (!target) return "";
  if (target.kind !== "episode" && target.progressKind !== "tv") return "";
  return target.showId || target.progressId || "";
}

/** Header height, including a wrapped activity line, so theater starts under it. */
function placeTheaterHead(): void {
  const head = document.querySelector(".top");
  if (!(head instanceof HTMLElement)) return;
  const height = Math.ceil(head.getBoundingClientRect().height);
  if (height > 0) document.documentElement.style.setProperty("--head-h", height + "px");
}

function setEpisodesOpen(open: boolean): void {
  const aside = el("dock-episodes");
  const wasOpen = aside.classList.contains("is-open");
  const inside = aside.contains(document.activeElement);
  episodesOpen = open;
  aside.classList.toggle("is-open", open);
  // A closed list is zero-width. Keep its rows out of tab order until it opens.
  // inert moves focus before we can see that it was inside, so remember that first.
  aside.inert = !open;
  aside.setAttribute("aria-hidden", open ? "false" : "true");
  const tab = el<HTMLButtonElement>("dock-episodes-tab");
  tab.setAttribute("aria-expanded", open ? "true" : "false");
  tab.title = open ? "Hide episodes" : "Show episodes";
  if (!open) {
    if (inside) tab.focus();
    return;
  }
  if (!wasOpen) schedulePlayingScroll();
}

/** Centers the playing episode after the panel has finished opening. */
function schedulePlayingScroll(): void {
  scrolledKey = "";
  const aside = el("dock-episodes");
  let settled = false;
  const finish = (): void => {
    if (settled) return;
    settled = true;
    aside.removeEventListener("transitionend", onEnd);
    if (aside.classList.contains("is-open")) markPlayingRow();
  };
  const onEnd = (event: TransitionEvent): void => {
    if (event.target === aside && event.propertyName === "width") finish();
  };
  aside.addEventListener("transitionend", onEnd);
  window.setTimeout(finish, 260);
}

function applyTheater(): void {
  const on = theaterWanted && el("dock").classList.contains("open");
  document.body.classList.toggle("theater", on);
  const button = el<HTMLButtonElement>("dock-theater");
  button.classList.toggle("is-on", on);
  button.setAttribute("aria-pressed", on ? "true" : "false");
  const label = on ? "Exit theater mode" : "Theater mode";
  button.setAttribute("aria-label", label);
  button.title = label + " (t)";
  const show = showIdOf(currentTarget);
  const side = el("dock-theater-side");
  const showSide = on && !!show;
  // Hiding the side (display:none) drops focus to the page. The dock is still open, so land on Theater.
  if (!showSide && el("dock").classList.contains("open") && side.contains(document.activeElement)) {
    el<HTMLButtonElement>("dock-theater").focus({ preventScroll: true });
  }
  side.hidden = !showSide;
  if (showSide) {
    setEpisodesOpen(episodesOpen);
    requestEpisodes();
  }
  fitPlayingPadding();
}

/** Theater was on when the player was minimized, so Expand brings it back. */
let theaterBeforeMini = false;

function isMini(): boolean {
  return el("dock").classList.contains("mini");
}

/** Minimized, the dock shrinks to a corner player and playback carries on. Only the layout changes. */
function setMini(on: boolean): void {
  const dock = el("dock");
  if (isMini() === on) return;
  if (on) {
    if (document.fullscreenElement === el("dock-stage")) void document.exitFullscreen().catch(() => undefined);
    if (castMenuOpen()) {
      castMenuGen += 1;
      hideCastMenu();
      paintCast();
    }
    if (subtitleMenuOpen()) {
      hideSubtitleMenu();
      paintSubtitleState();
    }
    theaterBeforeMini = theaterWanted;
    theaterWanted = false;
    // Minimizing pauses what plays here; a TV keeps playing, since this page is only its remote.
    if (!castingAway() && !video().paused) video().pause();
  }
  dock.classList.toggle("mini", on);
  const button = el<HTMLButtonElement>("dock-mini");
  const label = on ? "Expand player" : "Minimize player";
  button.setAttribute("aria-label", label);
  button.title = label;
  button.setAttribute("aria-expanded", on ? "false" : "true");
  const wide = !on && theaterBeforeMini;
  if (!on) theaterBeforeMini = false;
  // toggleTheater refetches the episode list, which may have moved on while minimized.
  if (wide && dock.classList.contains("open")) toggleTheater();
  else {
    if (wide) theaterWanted = true;
    applyTheater();
  }
}

function toggleTheater(): void {
  // T while minimized opens the player straight into theater.
  if (isMini()) {
    theaterBeforeMini = false;
    setMini(false);
    theaterWanted = false;
  }
  theaterWanted = !theaterWanted;
  if (theaterWanted) {
    // Opening wide loads the season again. A quality change does not, so its list stays put.
    // Keep listedShow so a resolve that is already running does not replace the same show's rows with a waiting note.
    episodeTitles.clear();
    episodePages.clear();
    scrolledKey = "";
    reloadEpisodes = true;
  }
  applyTheater();
}

function paintEpisodeNote(text: string): void {
  const list = el("dock-episodes-list");
  const previous = document.activeElement;
  const inside = previous instanceof Node && list.contains(previous);
  list.replaceChildren();
  const note = document.createElement("p");
  note.className = "theater-note";
  note.textContent = text;
  list.append(note);
  // The note is not a tab stop. A row that was focused would otherwise fall to the document.
  if (inside) el<HTMLButtonElement>("dock-episodes-tab").focus({ preventScroll: true });
}

function seasonLabel(season: number): string {
  return season === 0 ? "Specials" : "Season " + season;
}

/** Library folders win, then folders this tab already opened. Specials go last. */
function seasonChoices(title: TitleDetail | null, showId: string): SeasonChoice[] {
  const bySeason = new Map<number, string>();
  for (const folder of title?.season_folders ?? []) {
    if (folder.path) bySeason.set(folder.season, folder.path);
  }
  for (const folder of seasonFoldersFor(showId)) {
    if (folder.path && !bySeason.has(folder.season)) bySeason.set(folder.season, folder.path);
  }
  if (!bySeason.size && title) {
    for (const season of title.seasons) bySeason.set(season.number, "");
  }
  return [...bySeason.entries()]
    .map(([season, path]) => ({ season, path }))
    .sort((a, b) => {
      if (a.season === 0) return 1;
      if (b.season === 0) return -1;
      return a.season - b.season;
    });
}

function episodeNumbers(item: BrowseItem, folderSeason: number | null): { season: number | null; episode: number } | null {
  if (item.kind === "run" || !item.path) return null;
  const info = item.info ?? null;
  let season = item.season ?? info?.season ?? folderSeason;
  let episode = item.episode ?? info?.episode ?? null;
  if (!episode) {
    const label = item.label + " " + (info?.title ?? "");
    const coded = /S(\d{1,2})\s*E(\d{1,3})/i.exec(label);
    const crossed = coded ? null : /(\d{1,2})\s*[x×]\s*(\d{1,3})/i.exec(label);
    const match = coded ?? crossed;
    if (!match) return null;
    if (item.season == null && info?.season == null) season = Number(match[1]);
    episode = Number(match[2]);
  }
  if (!episode || episode < 1) return null;
  return { season, episode };
}

function progressFor(title: TitleDetail | null, path: string, season: number | null, episode: number): Episode | undefined {
  if (!title) return undefined;
  let byNumber: Episode | undefined;
  for (const group of title.seasons) {
    for (const item of group.episodes) {
      if (path && item.path === path) return item;
      if (!byNumber && item.season === season && item.episode === episode) byNumber = item;
    }
  }
  return byNumber;
}

function listedFromLibrary(title: TitleDetail, showName: string, season: number | null): ListedEpisode[] {
  const groups = season == null ? title.seasons : title.seasons.filter((group) => group.number === season);
  const rows: ListedEpisode[] = [];
  for (const group of groups) {
    for (const episode of group.episodes) {
      const name = cleanEpisodeTitle(episode.name, episode.season, episode.episode, showName) || episode.name;
      rows.push({
        showId: title.id,
        showName,
        file: episode.path,
        season: episode.season,
        episode: episode.episode,
        name,
        art: episode.art ?? title.art,
        position: episode.position,
        duration: episode.duration,
        finished: episode.finished,
        resumable: episode.resumable,
      });
    }
  }
  rows.sort((a, b) => a.episode - b.episode);
  return rows;
}

function listedFromBrowse(
  page: BrowsePage,
  folderSeason: number | null,
  title: TitleDetail | null,
  showId: string,
  showName: string,
): ListedEpisode[] {
  const rows: ListedEpisode[] = [];
  for (const item of page.items) {
    const numbers = episodeNumbers(item, folderSeason);
    if (!numbers) continue;
    const known = progressFor(title, item.path, numbers.season, numbers.episode);
    const raw = item.info?.title || item.label || "";
    const name = cleanEpisodeTitle(raw, numbers.season, numbers.episode, showName) || item.label || episodeCode(numbers.season, numbers.episode);
    rows.push({
      showId,
      showName,
      file: item.path,
      season: numbers.season,
      episode: numbers.episode,
      name,
      art: item.art ?? known?.art ?? title?.art,
      info: item.info,
      position: known?.position ?? 0,
      duration: known?.duration ?? 0,
      finished: !!known?.finished,
      resumable: !!known?.resumable,
    });
  }
  rows.sort((a, b) => a.episode - b.episode || a.name.localeCompare(b.name));
  return rows;
}

function playListed(row: ListedEpisode): void {
  const current = currentTarget;
  if (current && row.file && current.file === row.file) return;
  const showName = humanTitle(row.showName) || humanTitle(current?.showTitle) || humanTitle(current?.progressName) || row.showName;
  const resume = row.resumable && row.position > 60 ? row.position : undefined;
  play({
    kind: "episode",
    id: row.showId,
    showId: row.showId,
    file: row.file,
    title: displayMediaTitle(row.name, row.season, row.episode, showName),
    showTitle: showName,
    season: row.season ?? undefined,
    episode: row.episode,
    art: row.art ?? current?.art,
    info: row.info,
    resumeSeconds: resume,
    progressKind: "tv",
    progressId: row.showId,
    progressName: showName,
    trackProgress: true,
  });
}

function markPlayingRow(): void {
  const list = el("dock-episodes-list");
  const file = currentTarget?.file ?? "";
  const season = currentTarget?.season;
  const episode = currentTarget?.episode;
  const buttons = [...list.querySelectorAll<HTMLButtonElement>(".theater-ep")];
  const byFile = file ? buttons.find((button) => button.dataset.file === file) : undefined;
  const byNumber =
    season != null && episode != null
      ? buttons.find((button) => button.dataset.season === String(season) && button.dataset.episode === String(episode))
      : undefined;
  const match = byFile ?? byNumber ?? null;
  let current: HTMLElement | null = null;
  for (const button of buttons) {
    if (button === match) {
      button.setAttribute("aria-current", "true");
      current = button;
    } else button.removeAttribute("aria-current");
  }
  // Width is still 0 while the drawer is closed or mid-animation. Measuring then
  // would lock in a bad scroll position for when the list is actually visible.
  const panel = el("dock-episodes");
  if (!current || !panel.classList.contains("is-open") || list.clientWidth < 8) return;
  const key = showIdOf(currentTarget) + "\n" + file + "\n" + String(season ?? "") + "\n" + String(episode ?? "");
  if (key === scrolledKey) return;
  scrolledKey = key;
  const top = current.getBoundingClientRect().top - list.getBoundingClientRect().top;
  list.scrollTop += top - list.clientHeight / 2 + current.offsetHeight / 2;
}

function paintEpisodeRows(rows: ListedEpisode[], note: string): void {
  const list = el("dock-episodes-list");
  const previous = document.activeElement;
  const inside = previous instanceof Node && list.contains(previous);
  const previousFile = previous instanceof HTMLButtonElement && inside ? (previous.dataset.file ?? "") : "";
  if (!rows.length) {
    paintEpisodeNote(note || "No episodes listed for this season.");
    return;
  }
  list.replaceChildren();
  if (note) {
    const line = document.createElement("p");
    line.className = "theater-note";
    line.textContent = note;
    list.append(line);
  }
  for (const row of rows) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "theater-ep" + (row.finished ? " is-watched" : "");
    button.dataset.file = row.file;
    button.dataset.season = row.season == null ? "" : String(row.season);
    button.dataset.episode = String(row.episode);
    const code = episodeCode(row.season, row.episode);
    const resume = row.resumable && row.position > 60;
    const action = resume ? "Resume" : row.finished ? "Play again" : "Play";
    button.setAttribute("aria-label", action + " " + [code, row.name].filter(Boolean).join(" · "));
    const still = document.createElement("span");
    still.className = "theater-still";
    const url = artUrl(row.art);
    if (url) {
      const img = document.createElement("img");
      img.alt = "";
      img.src = url;
      img.decoding = "async";
      still.append(img);
    }
    if (!row.finished && row.duration > 0 && row.position > 0) {
      const bar = document.createElement("span");
      bar.className = "bar";
      bar.setAttribute("aria-hidden", "true");
      const fill = document.createElement("span");
      const width = Math.max(0, Math.min(100, (row.position / row.duration) * 100));
      fill.style.width = width + "%";
      bar.append(fill);
      still.append(bar);
    }
    const text = document.createElement("span");
    const codeLine = document.createElement("p");
    codeLine.className = "theater-ep-code";
    codeLine.textContent = code;
    const nameLine = document.createElement("p");
    nameLine.className = "theater-ep-name";
    nameLine.textContent = row.name || code || "Episode";
    text.append(codeLine, nameLine);
    const metaText = resume ? "Resume " + formatClock(row.position) : row.finished ? "Watched" : "";
    if (metaText) {
      const meta = document.createElement("p");
      meta.className = "theater-ep-meta";
      meta.textContent = metaText;
      text.append(meta);
    }
    button.append(still, text);
    button.addEventListener("click", () => playListed(row));
    list.append(button);
  }
  markPlayingRow();
  if (!inside) return;
  const again = previousFile
    ? [...list.querySelectorAll<HTMLButtonElement>(".theater-ep")].find((button) => button.dataset.file === previousFile)
    : undefined;
  const keep = again ?? list.querySelector<HTMLButtonElement>('.theater-ep[aria-current="true"]');
  (keep ?? el<HTMLButtonElement>("dock-episodes-tab")).focus({ preventScroll: true });
}

function paintSeasonChips(showId: string, choices: SeasonChoice[], selected: number | null): void {
  const box = el("dock-episodes-seasons");
  const restore = document.activeElement instanceof Node && box.contains(document.activeElement);
  box.replaceChildren();
  if (choices.length < 2) return;
  for (const choice of choices) {
    const button = document.createElement("button");
    button.type = "button";
    button.setAttribute("role", "tab");
    button.setAttribute("aria-selected", choice.season === selected ? "true" : "false");
    button.tabIndex = choice.season === selected ? 0 : -1;
    button.textContent = seasonLabel(choice.season);
    button.addEventListener("click", () => {
      if (showIdOf(currentTarget) !== showId) return;
      queuedSeason = choice.season;
      if (playTicket !== 0 || holdEpisodes) {
        episodeWait = true;
        return;
      }
      // listedSeason changes only after this season paints. Setting it here makes the cache treat the old rows as the new season.
      void fillEpisodes(showId, choice.season, false);
    });
    box.append(button);
  }
  const current = box.querySelector<HTMLButtonElement>('[aria-selected="true"]');
  if (!current) return;
  const left = current.getBoundingClientRect().left - box.getBoundingClientRect().left;
  box.scrollLeft += left - box.clientWidth / 2 + current.offsetWidth / 2;
  // Replacing the chips drops focus. Put it back on the season that is showing.
  if (restore) current.focus({ preventScroll: true });
}

/** Up and down move through episodes. Left and right change season. Other arrows stay put. */
function moveTheaterFocus(target: Node, key: string): boolean {
  if (!(target instanceof HTMLButtonElement)) return false;
  if (target.classList.contains("theater-ep") && (key === "ArrowUp" || key === "ArrowDown")) {
    const episodes = [...el("dock-episodes-list").querySelectorAll<HTMLButtonElement>(".theater-ep")];
    const index = episodes.indexOf(target);
    const next = index < 0 ? undefined : episodes[index + (key === "ArrowDown" ? 1 : -1)];
    if (!next) return false;
    next.focus({ preventScroll: true });
    const list = el("dock-episodes-list");
    const top = next.getBoundingClientRect().top - list.getBoundingClientRect().top;
    if (top < 0 || top + next.offsetHeight > list.clientHeight) {
      list.scrollTop += top - list.clientHeight / 2 + next.offsetHeight / 2;
    }
    return true;
  }
  if (target.getAttribute("role") === "tab" && (key === "ArrowLeft" || key === "ArrowRight")) {
    const chips = [...el("dock-episodes-seasons").querySelectorAll<HTMLButtonElement>('[role="tab"]')];
    const index = chips.indexOf(target);
    const next = index < 0 ? undefined : chips[index + (key === "ArrowRight" ? 1 : -1)];
    if (!next) return false;
    next.click();
    return true;
  }
  return false;
}

function showHeading(title: TitleDetail | null): string {
  return humanTitle(currentTarget?.showTitle) || humanTitle(title?.name) || humanTitle(currentTarget?.progressName) || "";
}

/** This load lost the race to a resolve. Retry when the play ticket clears. Do not browse from here. */
function deferEpisodeLoad(ticket: number, showId: string, refetch: boolean): void {
  if (ticket !== episodeTicket) return;
  if (refetch) reloadEpisodes = true;
  if (playTicket === 0 || !document.body.classList.contains("theater") || showIdOf(currentTarget) !== showId) return;
  episodeWait = true;
}

/**
 * Loads one season into the side panel.
 * `follow` is set when playback moved to another show or season, so a season the viewer opened stays put.
 */
async function fillEpisodes(showId: string, season: number | null, follow: boolean): Promise<void> {
  const ticket = ++episodeTicket;
  const mine = (): boolean =>
    ticket === episodeTicket &&
    !holdEpisodes &&
    playTicket === 0 &&
    document.body.classList.contains("theater") &&
    showIdOf(currentTarget) === showId;
  let title = episodeTitles.get(showId) ?? null;
  if (!episodeTitles.has(showId)) {
    try {
      title = await api.title(showId);
    } catch (err) {
      if (!mine()) {
        deferEpisodeLoad(ticket, showId, false);
        return;
      }
      if (err instanceof ApiError && err.status === 404) title = null;
      else {
        episodeTitles.delete(showId);
        setStatus(message(err));
        paintEpisodeNote("Episodes could not be loaded.");
        return;
      }
    }
    if (!mine()) {
      deferEpisodeLoad(ticket, showId, false);
      return;
    }
    episodeTitles.set(showId, title);
  }
  if (!mine()) {
    deferEpisodeLoad(ticket, showId, false);
    return;
  }
  const choices = seasonChoices(title, showId);
  const selected =
    season != null && (choices.length === 0 || choices.some((choice) => choice.season === season))
      ? season
      : (choices[0]?.season ?? season);
  const list = el("dock-episodes-list");
  const refetch = reloadEpisodes;
  reloadEpisodes = false;
  if (!refetch && listedShow === showId && listedSeason === selected && list.querySelector(".theater-ep")) {
    if (follow) followedSeason = currentTarget?.season ?? null;
    queuedSeason = null;
    paintSeasonChips(showId, choices, selected);
    markPlayingRow();
    return;
  }
  const showName = showHeading(title);
  el("dock-episodes-show").textContent = showName;
  el("dock-episodes-heading").textContent = selected == null ? "Episodes" : seasonLabel(selected);
  paintSeasonChips(showId, choices, selected);
  const choice = choices.find((item) => item.season === selected);
  let rows: ListedEpisode[] = [];
  let note = "";
  if (choice?.path) {
    if (!mine()) {
      deferEpisodeLoad(ticket, showId, refetch);
      return;
    }
    let page = episodePages.get(choice.path) ?? null;
    if (!page) {
      try {
        page = await api.browse(choice.path);
        episodePages.set(choice.path, page);
      } catch (err) {
        episodePages.delete(choice.path);
        if (!mine()) {
          deferEpisodeLoad(ticket, showId, refetch);
          return;
        }
        setStatus(message(err));
        paintEpisodeNote("Episodes could not be loaded.");
        return;
      }
    }
    if (!mine()) {
      deferEpisodeLoad(ticket, showId, refetch);
      return;
    }
    if (!page) {
      deferEpisodeLoad(ticket, showId, refetch);
      return;
    }
    rows = listedFromBrowse(page, choice.season, title, showId, showName);
    if (!rows.length && title) {
      rows = listedFromLibrary(title, showName, selected);
      if (rows.length) note = "Only episodes Portal has already opened are listed.";
    }
  } else if (title) {
    rows = listedFromLibrary(title, showName, selected);
    if (rows.length) note = "Only episodes Portal has already opened are listed.";
  }
  if (!mine()) {
    deferEpisodeLoad(ticket, showId, refetch);
    return;
  }
  listedShow = showId;
  listedSeason = selected;
  if (follow) followedSeason = currentTarget?.season ?? null;
  if (queuedSeason === selected) queuedSeason = null;
  paintEpisodeRows(rows, note);
}

/** Asks for the playing season, unless the viewer is browsing another one. Never calls Kodi while a play is resolving. */
function requestEpisodes(): void {
  if (holdEpisodes || !document.body.classList.contains("theater")) return;
  const showId = showIdOf(currentTarget);
  if (!showId) return;
  const playing = currentTarget?.season ?? null;
  const list = el("dock-episodes-list");
  if (playTicket !== 0) {
    episodeWait = true;
    if (showId !== listedShow || !list.querySelector(".theater-ep")) {
      paintEpisodeNote("Episodes will list when this starts playing.");
    }
    return;
  }
  episodeWait = false;
  const follow = showId !== listedShow || playing !== followedSeason;
  let season = follow ? playing : (listedSeason ?? playing);
  if (!follow && queuedSeason != null) season = queuedSeason;
  if (follow) queuedSeason = null;
  void fillEpisodes(showId, season, follow);
}

function appliedSeek(resume: number | null | undefined): number {
  if (resume == null || !Number.isFinite(resume) || resume <= 60) return 0;
  return Math.floor(resume);
}

function originFrom(job: PlayJob): number {
  if (job.position != null && Number.isFinite(job.position) && job.position >= 0) return job.position;
  return appliedSeek(currentTarget?.resumeSeconds);
}

/** Where the TV is, from Portal's last look, moved on by the time since while it plays. */
function devicePosition(): number | null {
  const view = deviceView;
  if (castMode !== "device" || !view || view.position == null || !Number.isFinite(view.position)) return null;
  const end = progressDuration();
  // A seek in flight shows where it goes, not the report from before it.
  if (deviceSeeking) return end > 0 ? Math.min(deviceSeekTarget, end) : deviceSeekTarget;
  const moving = view.state === "playing" && !devicePaused;
  const now = Math.min(Date.now(), deviceFreshAt + DEVICE_STALE_MS);
  const at = view.position + (moving ? Math.max(0, now - deviceSeenAt) / 1000 : 0);
  return end > 0 ? Math.min(at, end) : at;
}

/** Something plays on a TV and not on this page. */
function castingAway(): boolean {
  return (castMode === "device" && !!castReceiverId) || browserCasting();
}

/** This page is following a browser TV and is not playing the video itself. */
function browserCasting(): boolean {
  return castMode === "browser" && !attached && !!castReceiverId;
}

function browserPosition(): number | null {
  const place = browserPlace;
  if (!browserCasting() || !place || !Number.isFinite(place.position)) return null;
  if (browserSeeking) return browserSeekTarget;
  const moving = !place.paused;
  const at = place.position + (moving ? Math.max(0, Date.now() - browserSeenAt) / 1000 : 0);
  const end = place.duration > 0 ? place.duration : progressDuration();
  return end > 0 ? Math.min(at, end) : at;
}

function readPlace(source: { place?: CastSession["place"] } | null | undefined): BrowserPlace | null {
  const place = source?.place;
  if (!place || !Number.isFinite(place.position) || place.position < 0) return null;
  const duration = Number.isFinite(place.duration) && place.duration > 0 ? place.duration : 0;
  const updated = Number.isFinite(place.updated_ms) ? place.updated_ms : Date.now();
  return { position: place.position, duration, paused: place.paused === true, updatedMs: updated };
}

function sourcePosition(): number {
  const device = devicePosition();
  if (device != null) return device;
  const browser = browserPosition();
  if (browser != null) return browser;
  // Nothing plays here (a cast ended, or the stream stopped): the place is where it left off.
  if (!attached && castMode !== "device" && deviceLeftAt > 0) return deviceLeftAt;
  const current = video().currentTime;
  const base = Number.isFinite(current) ? current : 0;
  return origin + base;
}

function progressDuration(): number {
  if (jobDuration > 0 && Number.isFinite(jobDuration)) return jobDuration;
  const device = castMode === "device" ? deviceView?.duration : null;
  if (device != null && Number.isFinite(device) && device > 0) return device;
  const browser = browserCasting() ? browserPlace?.duration : null;
  if (browser != null && Number.isFinite(browser) && browser > 0) return browser;
  const span = playedSpan(video());
  if (span > 0) return span + origin;
  return 0;
}

function message(err: unknown): string {
  if (err instanceof Error && err.message.trim()) return err.message;
  return "Playback failed.";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/** Show or movie title. Artwork JSON must not be reported as the name. */
function humanTitle(name: string | null | undefined): string {
  const text = (name ?? "").trim();
  if (!text) return "";
  const lower = text.toLowerCase();
  if (text.startsWith("{") || lower.startsWith("%7b")) return "";
  if (text.includes('"poster"') || lower.includes("%22poster%22")) return "";
  if (lower.includes("%7b") && lower.includes("poster")) return "";
  return text;
}

function progressReportName(target: PlayTarget): string {
  const named = humanTitle(target.progressName);
  if (named) return named;
  const show = humanTitle(target.showTitle);
  if (target.progressKind === "movie") return humanTitle(target.title) || show;
  return show;
}

async function postProgress(force: boolean, keepalive = false): Promise<void> {
  if (skipping) return;
  const target = currentTarget;
  if (!target?.trackProgress) return;
  const now = Date.now();
  if (!force && now - lastPost < 5000) return;
  // A browser TV reports its own place. This page's video is not the one playing.
  if (castMode === "browser" && !attached) return;
  // Nothing attached and no place known (between a cast and Play): saving now would reset it.
  if (!castMode && !attached && deviceLeftAt <= 0) return;
  const duration = progressDuration();
  const position = sourcePosition();
  if (!Number.isFinite(duration) || duration <= 0) return;
  if (!Number.isFinite(position) || position < 0) return;
  lastPost = now;
  const body: Record<string, unknown> = {
    id: target.progressId,
    kind: target.progressKind,
    name: progressReportName(target),
    file: target.file,
    position,
    duration,
  };
  if (target.art) body.art = target.art;
  if (target.progressKind === "tv") {
    if (target.season != null) body.season = target.season;
    if (target.episode) body.episode = target.episode;
    body.episode_name = target.title;
  }
  // Pause, stop, and leaving the page write the library now. The ticker does not.
  if (force) body.flush = true;
  await api.progress(body, keepalive);
  // A page showing this title's episodes refreshes what it says was watched.
  if (force) window.dispatchEvent(new CustomEvent("portal:progress", { detail: { id: target.progressId } }));
}

function stopTicker(): void {
  if (ticker) window.clearInterval(ticker);
  ticker = 0;
}

function startTicker(token: number): void {
  stopTicker();
  ticker = window.setInterval(() => {
    if (token !== generation) return;
    void postProgress(false).catch(() => undefined);
    void refreshNext(token);
    void heartbeat(token);
  }, 8000);
}

/**
 * Tells the server this page still has its stream, paused or not; one that
 * stops asking is treated as gone. A stream the server had to end says why.
 */
async function heartbeat(token: number): Promise<void> {
  const id = currentJob;
  if (!id || !attached || castMode || skipping) return;
  const job = await api.job(id).catch(() => null);
  if (token !== generation || currentJob !== id || skipping) return;
  if (job?.state === "failed" && job.error) setStatus(job.error);
}

/** The job last asked for a next episode found after it started. */
let nextAsked = "";

/**
 * A season's last episode starts without a next one; Portal lists the next
 * season while it plays. Asked once, near the end, so the up-next card and
 * Next carry on into it.
 */
async function refreshNext(token: number): Promise<void> {
  const id = currentJob;
  if (!id || currentNext?.file || nextAsked === id) return;
  const left = progressDuration() - sourcePosition();
  if (!(left > 0 && left <= 90)) return;
  nextAsked = id;
  const next = await jobNext(id);
  if (token !== generation || currentJob !== id || !next) return;
  currentNext = next;
  paintNext();
}

async function jobNext(id: string): Promise<NextItem | null> {
  try {
    const job = await api.job(id);
    return job.next?.file ? job.next : null;
  } catch {
    return null;
  }
}

function destroyHls(): void {
  pendingAttach = null;
  if (hls) hls.destroy();
  hls = null;
  const media = video();
  media.pause();
  detachSubtitle();
  media.removeAttribute("src");
  media.load();
}

/*
 * Subtitles. `prefs().subtitles` is "off" or "auto" (on, Portal picks), and
 * `subtitleLanguage` the language of the last pick. One track is attached at
 * a time: the explicit pick for this item, else one in the last picked
 * language (in the video, then online), else an embedded track in a preferred
 * language, else the best online result, else the first embedded track.
 * Online results are searched once per played item; `subsToken` changes
 * with the item so a late answer for an earlier one is dropped.
 */

type OnlineState = {
  /** The job the results belong to. */
  key: string;
  state: "idle" | "loading" | "done" | "error";
  items: SubtitleCandidate[];
  best: string | null;
  notes: string[];
  error: string;
};

function noOnline(key = ""): OnlineState {
  return { key, state: "idle", items: [], best: null, notes: [], error: "" };
}

let online: OnlineState = noOnline();
let subsToken = 0;
/** This item's explicit choice: "e:<embedded id>" or "o:<online id>". */
let subPick: string | null = null;
let subDelay = 0;
/** The attached track: "e:<id>", or "o:<id>@<origin>" since online files are cut to the stream. */
let activeSub = "";
let activeSubBlob = "";
/** The online file being fetched, keyed like `activeSub`. */
let subLoading = "";
let failedOnline = new Set<string>();
let subSettings: SubtitleSettings | null = null;
/** The dock status line this module last wrote, so it only clears its own. */
let subNotice = "";
/** An error stays until the user acts again; progress notes go when a track loads. */
let subNoticeSticky = false;

function subtitlesOn(): boolean {
  return prefs().subtitles !== "off";
}

function subNote(text: string, sticky = false): void {
  subNotice = text;
  subNoticeSticky = sticky;
  setStatus(text);
}

function clearSubNote(keepErrors = false): void {
  if (!subNotice || (keepErrors && subNoticeSticky)) return;
  // While a TV plays, the cast's own line comes back.
  if (el("dock-status").textContent === subNotice) setStatus(castMode === "device" ? deviceLine : "");
  subNotice = "";
}

/** Forgets the last item's subtitles; a later answer for it is ignored. */
function resetSubtitles(): void {
  subsToken += 1;
  online = noOnline();
  subPick = null;
  subDelay = 0;
  failedOnline = new Set();
  clearSubNote();
  paintSubtitleState();
}

/** What should show now, as "e:<id>" or "o:<id>", or null for nothing. */
function chosenSubtitle(): string | null {
  if (!subtitlesOn()) return null;
  if (subPick?.startsWith("e:") && currentSubs.some((track) => "e:" + track.id === subPick)) return subPick;
  if (subPick?.startsWith("o:") && online.items.some((item) => "o:" + item.id === subPick)) return subPick;
  const usable = online.items.filter((item) => !failedOnline.has(item.id));
  // The language of the last pick comes first, in the video, then online.
  const language = prefs().subtitleLanguage;
  if (language) {
    const track = preferredTrack(currentSubs, [language]);
    if (track) return "e:" + track.id;
    const item = usable.find((candidate) => sameLanguage(candidate.language, language));
    if (item) return "o:" + item.id;
  }
  return autoSubtitle(currentSubs, usable, preferredLanguages(subSettings));
}

function chosenName(choice: string | null): string {
  if (!choice) return "";
  const id = choice.slice(2);
  if (choice.startsWith("e:")) {
    const track = currentSubs.find((item) => String(item.id) === id);
    return track ? trackName(track) : "";
  }
  const item = online.items.find((candidate) => candidate.id === id);
  return item ? candidateTitle(item) : "";
}

function detachSubtitle(): void {
  removeTracks(video());
  if (activeSubBlob) URL.revokeObjectURL(activeSubBlob);
  activeSubBlob = "";
  activeSub = "";
  subLoading = "";
}

/** The attached track shows; Chrome can reset modes when the stream changes. */
function showActiveTrack(): void {
  const list = video().textTracks;
  for (let index = 0; index < list.length; index += 1) list[index].mode = activeSub ? "showing" : "disabled";
}

function attachTrack(src: string, label: string, lang: string, key: string, blob: boolean): void {
  const media = video();
  const node = document.createElement("track");
  node.kind = "subtitles";
  node.label = label || "Subtitles";
  const srclang = trackLanguage(lang);
  if (srclang) node.srclang = srclang;
  node.src = src;
  node.default = true;
  node.addEventListener("load", () => {
    if (activeSub !== key) return;
    shiftCues(node.track, subDelay);
    placeCues(true);
    clearSubNote(true);
  });
  node.addEventListener("error", () => {
    if (activeSub !== key) return;
    subNote("These subtitles could not be loaded.", true);
  });
  media.appendChild(node);
  node.track.mode = "showing";
  activeSub = key;
  activeSubBlob = blob ? src : "";
}

/** Attaches whatever `chosenSubtitle` says, fetching an online file first. */
function applySubtitles(): void {
  paintSubtitleState();
  const want = attached ? chosenSubtitle() : null;
  if (!want) {
    if (activeSub || subLoading) detachSubtitle();
    return;
  }
  const key = want.startsWith("o:") ? want + "@" + origin : want;
  if (key === activeSub) {
    showActiveTrack();
    return;
  }
  if (key === subLoading) return;
  detachSubtitle();
  const id = want.slice(2);
  if (want.startsWith("e:")) {
    const track = currentSubs.find((item) => String(item.id) === id);
    if (track) attachTrack(track.url, trackName(track), track.lang ?? "", key, false);
    paintSubtitleState();
    return;
  }
  const item = online.items.find((candidate) => candidate.id === id);
  const token = subsToken;
  subLoading = key;
  paintSubtitleState();
  void onlineTrackUrl(id, origin).then(
    (url) => {
      if (token !== subsToken || subLoading !== key) {
        URL.revokeObjectURL(url);
        return;
      }
      subLoading = "";
      attachTrack(url, item ? candidateTitle(item) : "Subtitles", item?.language ?? "", key, true);
      // A pick is remembered by its language once it loads, for the next video.
      if (subPick === want && item?.language) setPrefs({ subtitleLanguage: item.language });
      paintSubtitleState();
      if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
    },
    (err: unknown) => {
      if (token !== subsToken || subLoading !== key) return;
      subLoading = "";
      failedOnline.add(id);
      if (subPick === want) subPick = null;
      subNote(err instanceof Error && err.message ? err.message : "Those subtitles could not be loaded.", true);
      applySubtitles();
      if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
    },
  );
}

/** Searches online once per played item. */
function searchSubtitles(job: PlayJob): void {
  const target = currentTarget;
  if (!target || online.key === job.id) return;
  const token = subsToken;
  const key = job.id;
  online = { ...noOnline(key), state: "loading" };
  if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
  void (async () => {
    const settings = await subtitleSettings();
    if (token !== subsToken || online.key !== key) return;
    subSettings = settings;
    const body = searchFor(infoSeed(target), job.release, settings);
    let next: OnlineState;
    if (!body) {
      next = { ...noOnline(key), state: "done" };
    } else {
      try {
        const results = cleanResults(await api.searchSubtitles(body));
        next = { key, state: "done", ...results, error: "" };
      } catch (err) {
        next = { ...noOnline(key), state: "error", error: message(err) };
      }
    }
    if (token !== subsToken || online.key !== key) return;
    online = next;
    const waitingForOne = subtitlesOn() && !activeSub && !subLoading;
    applySubtitles();
    // Casting with CC on and nothing on the TV yet: the search just found one.
    if (deviceNeedsSubtitle()) void syncDeviceSubtitles();
    if (waitingForOne && subNotice && !chosenSubtitle()) subNote(noSubtitlesText());
    if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
  })();
}

function noSubtitlesText(): string {
  if (online.state === "error") return "Portal could not search for subtitles: " + online.error;
  return online.notes[0] || "No subtitles found for this video.";
}

function subtitleMenu(): HTMLElement {
  return el("dock-subs-menu");
}

function subtitleMenuOpen(): boolean {
  return !subtitleMenu().hidden;
}

function focusedMenuIndex(): number {
  const items = menuItems(subtitleMenu());
  const at = items.indexOf(document.activeElement as HTMLButtonElement);
  return at < 0 ? -1 : at;
}

function hideSubtitleMenu(restoreFocus = false): void {
  const menu = subtitleMenu();
  const wasOpen = !menu.hidden;
  menu.hidden = true;
  menu.replaceChildren();
  const opener = document.getElementById("dock-subs-more");
  if (opener) opener.setAttribute("aria-expanded", "false");
  if (restoreFocus && wasOpen && opener instanceof HTMLButtonElement) opener.focus();
}

function paintSubtitleState(): void {
  const button = el<HTMLButtonElement>("dock-subs");
  const more = el<HTMLButtonElement>("dock-subs-more");
  const on = subtitlesOn();
  const ready = !!currentJob;
  const tvPending = castMode === "device" && on && !!deviceView?.subtitle_pending;
  const loading = tvPending || !!subLoading || (on && !activeSub && !deviceChoice() && online.state === "loading");
  // While a TV plays, the button names what the TV shows.
  const name = on ? chosenName(deviceChoice() ?? chosenSubtitle()) : "";
  button.disabled = !ready;
  more.disabled = !ready;
  button.classList.toggle("is-on", on);
  button.classList.toggle("is-loading", ready && loading);
  button.setAttribute("aria-pressed", on ? "true" : "false");
  const state = !on ? "off" : loading ? "on, loading" : name ? "on, " + name : "on";
  button.title = "Subtitles " + state + " (c)";
  button.setAttribute("aria-label", "Subtitles " + state);
  if (!subtitleMenuOpen()) more.setAttribute("aria-expanded", "false");
}

/** CC: subtitles on (Portal picks) or off, remembered for the next video. */
function toggleSubtitles(): void {
  if (!currentJob) return;
  const on = !subtitlesOn();
  if (!on) subPick = null;
  setPrefs({ subtitles: on ? "auto" : "off", subtitlesSet: true });
  applySubtitles();
  if (castMode === "device") {
    void syncDeviceSubtitles();
    return;
  }
  if (!subtitlesOn()) {
    clearSubNote();
    return;
  }
  if (activeSub || subLoading) return;
  if (online.state === "loading" || online.state === "idle") subNote("Looking for subtitles…");
  else subNote(noSubtitlesText());
}

/**
 * Off, Auto (Portal picks, in your languages), or a track for this video. A
 * track's language is remembered, so the next video starts in it; an online
 * pick's once it has loaded.
 */
function pickSubtitle(choice: string): void {
  if (choice === "off") {
    subPick = null;
    setPrefs({ subtitles: "off", subtitlesSet: true });
  } else if (choice === "auto") {
    subPick = null;
    setPrefs({ subtitles: "auto", subtitlesSet: true, subtitleLanguage: null });
  } else if (choice.startsWith("e:")) {
    subPick = choice;
    const track = currentSubs.find((item) => "e:" + item.id === choice);
    const language = track?.lang?.trim() ?? "";
    setPrefs(language ? { subtitles: "auto", subtitlesSet: true, subtitleLanguage: language } : { subtitles: "auto", subtitlesSet: true });
  } else {
    subPick = choice;
    failedOnline.delete(choice.slice(2));
    setPrefs({ subtitles: "auto", subtitlesSet: true });
  }
  clearSubNote();
  hideSubtitleMenu(true);
  applySubtitles();
  if (castMode === "device") void syncDeviceSubtitles();
}

/** The subtitle this page would show, as the TV needs it. */
function castSubtitle(): CastSubtitle | null {
  const choice = chosenSubtitle();
  if (!choice) return null;
  const id = choice.slice(2);
  if (choice.startsWith("e:")) {
    const track = currentSubs.find((item) => String(item.id) === id);
    return track ? { kind: "embedded", id: track.id, lang: track.lang ?? null } : null;
  }
  const item = online.items.find((candidate) => candidate.id === id);
  return item ? { kind: "online", id: item.id, lang: item.language } : null;
}

/** While casting to a TV, it shows the subtitle this page would (or none). */
let deviceSubsSeq = 0;

async function syncDeviceSubtitles(): Promise<void> {
  const id = castReceiverId;
  if (castMode !== "device" || !id) return;
  // The last change wins; an earlier one still in flight reports nothing.
  const mine = ++deviceSubsSeq;
  const choice = subtitlesOn() ? castSubtitle() : null;
  if (subtitlesOn() && !choice) {
    subNote(online.state === "loading" ? "Looking for subtitles…" : noSubtitlesText());
    return;
  }
  try {
    await api.castDeviceSubtitles(id, choice);
    if (mine !== deviceSubsSeq || castMode !== "device" || castReceiverId !== id) return;
    if (deviceView) deviceView = { ...deviceView, subtitle: choice ?? undefined, subtitle_pending: false };
    if (choice) subNote("Subtitles are on for the TV.");
    else clearSubNote();
    if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
  } catch (err) {
    if (mine !== deviceSubsSeq) return;
    subNote(castFail(err), true);
    // A file Portal could not fetch or read is not picked again; Auto tries the next one.
    // A TV that moved on or did not answer (409, 5xx) says nothing about the file.
    const fileFault = err instanceof ApiError && err.status >= 400 && err.status < 500 && err.status !== 409;
    if (choice?.kind !== "online" || !fileFault || failedOnline.has(choice.id)) return;
    failedOnline.add(choice.id);
    if (subPick === "o:" + choice.id) subPick = null;
    const next = castSubtitle();
    if (next && next.id !== choice.id) void syncDeviceSubtitles();
    if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
  }
}

/** The subtitle the TV shows, as a menu choice. */
function deviceChoice(): string | null {
  const shown = castMode === "device" ? deviceView?.subtitle : null;
  if (!shown) return null;
  return (shown.kind === "embedded" ? "e:" : "o:") + shown.id;
}

/** The TV has no subtitle yet and Portal is not already finding it one. */
function deviceNeedsSubtitle(): boolean {
  return castMode === "device" && subtitlesOn() && !deviceView?.subtitle && !deviceView?.subtitle_pending;
}

function setDelay(value: number): void {
  subDelay = clampDelay(value);
  const list = video().textTracks;
  for (let index = 0; index < list.length; index += 1) shiftCues(list[index], subDelay);
  placeCues(true);
  const label = document.getElementById("dock-subs-delay");
  if (label) label.textContent = delayLabel(subDelay);
}

function menuRadio(label: string, detail: string, checked: boolean, onClick: () => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.setAttribute("role", "menuitemradio");
  node.setAttribute("aria-checked", checked ? "true" : "false");
  node.className = "sub-choice";
  const main = document.createElement("span");
  main.className = "sub-main";
  main.textContent = label;
  node.append(main);
  if (detail) {
    const meta = document.createElement("span");
    meta.className = "sub-meta";
    meta.textContent = detail;
    node.append(meta);
  }
  node.addEventListener("click", onClick);
  return node;
}

function menuHeading(text: string): HTMLElement {
  const node = document.createElement("div");
  node.className = "sub-heading";
  node.setAttribute("role", "presentation");
  node.textContent = text;
  return node;
}

function menuNote(text: string, error = false): HTMLElement {
  const node = document.createElement("p");
  node.className = error ? "sub-note is-error" : "sub-note";
  node.setAttribute("role", "status");
  node.textContent = text;
  return node;
}

function showSubtitleMenu(focus = 0): void {
  const menu = subtitleMenu();
  const on = subtitlesOn();
  const chosen = chosenSubtitle();
  const explicit = subPick;
  menu.replaceChildren();
  menu.append(menuRadio("Off", "", !on, () => pickSubtitle("off")));
  const tv = castMode === "device";
  const tvName = tv ? chosenName(deviceChoice()) : "";
  const picked = on && !explicit ? chosenName(chosen) : "";
  const autoDetail = tv && on && deviceView?.subtitle_pending
    ? "Finding subtitles for the TV…"
    : tv && on && !explicit && tvName
      ? "On the TV: " + tvName
      : picked
        ? (subLoading ? "Loading " : "Showing ") + picked
        : online.state === "loading"
          ? "Searching online…"
          : "Picks the best match in your languages";
  menu.append(menuRadio("Auto", autoDetail, on && !explicit, () => pickSubtitle("auto")));
  if (currentSubs.length) {
    menu.append(menuHeading("In the video"));
    for (const track of currentSubs) {
      const value = "e:" + track.id;
      const name = trackName(track);
      const language = languageName(track.lang ?? "");
      const detail = language && language !== name ? language : "";
      menu.append(menuRadio(name, detail, on && explicit === value, () => pickSubtitle(value)));
    }
  }
  menu.append(menuHeading("Online"));
  if (online.state === "loading") menu.append(menuNote("Searching online…"));
  else if (online.state === "error") menu.append(menuNote("Search failed: " + online.error, true));
  else if (online.state === "done" && !online.items.length) menu.append(menuNote("No online subtitles found."));
  else if (online.state === "idle") menu.append(menuNote("Not searched yet."));
  for (const item of online.items) {
    const value = "o:" + item.id;
    const failed = failedOnline.has(item.id);
    const detail = candidateDetail(item, item.id === online.best) + (failed ? " · Failed to load" : "");
    menu.append(menuRadio(candidateTitle(item), detail, on && explicit === value, () => pickSubtitle(value)));
  }
  for (const note of online.notes) menu.append(menuNote(note, true));
  // Timing moves the cues in this page's video; a TV shows the file as it is.
  if (castingAway()) {
    menu.append(menuButton("Close", () => hideSubtitleMenu(true)));
    openSubtitleMenu(menu, focus);
    return;
  }
  menu.append(menuHeading("Timing"));
  const timing = document.createElement("div");
  timing.className = "sub-timing";
  const label = document.createElement("span");
  label.id = "dock-subs-delay";
  label.className = "sub-delay";
  label.setAttribute("aria-live", "polite");
  label.textContent = delayLabel(subDelay);
  const earlier = menuButton("−0.5 s", () => setDelay(subDelay - DELAY_STEP));
  earlier.setAttribute("aria-label", "Show subtitles half a second earlier");
  const later = menuButton("+0.5 s", () => setDelay(subDelay + DELAY_STEP));
  later.setAttribute("aria-label", "Show subtitles half a second later");
  const reset = menuButton("Reset", () => setDelay(0));
  reset.setAttribute("aria-label", "Reset subtitle timing");
  timing.append(earlier, label, later, reset);
  menu.append(timing);
  menu.append(menuButton("Close", () => hideSubtitleMenu(true)));
  openSubtitleMenu(menu, focus);
}

function openSubtitleMenu(menu: HTMLElement, focus: number): void {
  menu.hidden = false;
  fitSubtitleMenu(menu);
  el("dock-subs-more").setAttribute("aria-expanded", "true");
  if (focus >= 0) focusMenuItem(menu, focus);
}

/** The stage clips what overflows it, so the menu scrolls within the room above the controls. */
function fitSubtitleMenu(menu: HTMLElement): void {
  const wrap = el("dock-subs-wrap");
  const stage = wrap.closest(".stage") ?? el("dock-stage");
  const room = wrap.getBoundingClientRect().top - stage.getBoundingClientRect().top - 12;
  menu.style.maxHeight = room > 0 ? "min(26rem, " + Math.max(96, Math.floor(room)) + "px)" : "";
}

function onSubtitleMenuClick(): void {
  if (!currentJob) return;
  if (subtitleMenuOpen()) {
    hideSubtitleMenu(true);
    return;
  }
  castMenuGen += 1;
  hideCastMenu();
  paintCast();
  if (online.state === "idle" && attached && currentJob) {
    const job = lastReadyJob;
    if (job) searchSubtitles(job);
  }
  showSubtitleMenu();
}

/**
 * Playback starts before Portal has read the file's own subtitles; they
 * join the CC menu when ready. A seek attaches a new session, which checks
 * for itself.
 */
function awaitSubtitles(job: PlayJob): void {
  if (!job.subtitles_pending) return;
  const token = generation;
  // While a TV plays it, the tracks still belong in the CC menu.
  const current = (): boolean => token === generation && currentJob === job.id && (attached || castMode === "device");
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
      if (now.subtitles?.length) {
        lastReadyJob = now;
        if (attached) fillSubtitles(now.subtitles);
        else currentSubs = now.subtitles;
        if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
        if (deviceNeedsSubtitle()) void syncDeviceSubtitles();
      }
      return;
    }
  })();
}

/** The job's embedded tracks, then whatever should show. */
function fillSubtitles(tracks: SubtitleTrack[]): void {
  currentSubs = tracks;
  applySubtitles();
  const media = video();
  const apply = () => applySubtitles();
  if (pendingSubtitleApply) media.removeEventListener("loadeddata", pendingSubtitleApply);
  pendingSubtitleApply = apply;
  media.addEventListener("loadeddata", apply, { once: true });
  window.setTimeout(apply, 300);
}

function paintScrubFill(bar: HTMLInputElement): void {
  const max = Number(bar.max);
  const at = Number(bar.value);
  const fill = Number.isFinite(max) && max > 0 && Number.isFinite(at) ? Math.min(100, Math.max(0, (at / max) * 100)) : 0;
  bar.style.setProperty("--scrub-fill", fill.toFixed(2) + "%");
}

function paintHours(duration: number): void {
  const hours = duration >= 3600;
  el("dock-elapsed").classList.toggle("has-hours", hours);
  el("dock-total").classList.toggle("has-hours", hours);
}

/** Fraction across the bar, accounting for the thumb so the tip matches the track. */
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
  const tip = el("dock-hover");
  tip.hidden = true;
  tip.textContent = "";
}

function hideHover(): void {
  hoverX = null;
  concealHover();
}

/** Time under the pointer, not the playhead. */
function placeHover(clientX: number): void {
  const bar = el<HTMLInputElement>("dock-bar");
  const tip = el("dock-hover");
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
  if (dragging || skipping) return;
  const position = sourcePosition();
  const duration = progressDuration();
  setText(el("dock-elapsed"), formatClock(position));
  setText(el("dock-total"), formatClock(duration));
  paintHours(duration);
  const bar = el<HTMLInputElement>("dock-bar");
  const max = Number.isFinite(duration) && duration > 0 ? Math.floor(duration) : 0;
  bar.max = String(max);
  const at = max > 0 ? Math.min(Math.floor(position), max) : 0;
  bar.value = String(at);
  bar.disabled = max <= 0 || !currentJob;
  paintSeekText(bar, seekValueText(at, max));
  paintScrubFill(bar);
  paintBuffered(bar, video(), origin, max);
  paintUpNext();
  if (hoverX == null) concealHover();
  else placeHover(hoverX);
}

/** The last 20 seconds offer the next episode. */
function paintUpNext(): void {
  const card = el("dock-upnext");
  const next = currentNext?.file ? currentNext : null;
  const duration = progressDuration();
  const left = duration - sourcePosition();
  const show =
    !!next && started && !castMode && duration > 60 && left > 0 && left <= 20 &&
    upNextDismissed !== next.file;
  if (show && next) setText(el("dock-upnext-title"), nextDetail(next) || humanTitle(next.title) || "Next episode");
  if (card.hidden === !show) return;
  card.hidden = !show;
  if (show) wake();
}

function resetClock(): void {
  dragging = false;
  el("dock-chrome").classList.remove("is-scrubbing");
  const bar = el<HTMLInputElement>("dock-bar");
  bar.disabled = true;
  bar.value = "0";
  bar.max = "0";
  paintSeekText(bar, seekValueText(0, 0));
  paintScrubFill(bar);
  bar.style.setProperty("--scrub-buffer", "0%");
  hideHover();
  el("dock-elapsed").textContent = formatClock(0);
  el("dock-total").textContent = formatClock(0);
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
  return next.kind === "episode" || (next.season != null && !!next.episode);
}

function paintNext(): void {
  const button = el<HTMLButtonElement>("dock-next");
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

function paintLinks(): void {
  const button = el<HTMLButtonElement>("dock-links");
  const ready = !!(currentTarget?.file && currentJob);
  button.disabled = !ready;
  const label = ready ? "Choose a stream link" : "Links unavailable";
  button.textContent = label;
  button.title = label;
  button.setAttribute("aria-label", label);
}

function playWithLinks(): void {
  if (!currentTarget?.file) return;
  const resume = sourcePosition();
  play({
    ...currentTarget,
    manualLinks: true,
    resumeSeconds: resume > 60 ? resume : undefined,
  });
}

function syncPlayButton(): void {
  const media = video();
  const playing = castMode === "device"
    ? !devicePaused
    : browserCasting()
      ? !(browserPlace?.paused ?? false)
      : !media.paused && !media.ended && !skipping;
  const button = el<HTMLButtonElement>("dock-play");
  const label = playing ? "Pause" : "Play";
  button.classList.toggle("is-playing", playing);
  button.setAttribute("aria-label", label);
  button.title = label + " (k)";
  paintStage();
}

function syncFullscreen(): void {
  const button = el<HTMLButtonElement>("dock-full");
  const on = document.fullscreenElement === el("dock-stage");
  const label = on ? "Exit fullscreen" : "Fullscreen";
  el("dock-stage").classList.toggle("is-full", on);
  button.classList.toggle("is-on", on);
  button.setAttribute("aria-label", label);
  button.title = label + " (f)";
  wake();
}

/** Moves the video to `local` seconds if the browser can seek there without a new stream. */
function seekInPlace(local: number): boolean {
  const media = video();
  const ranges = media.seekable;
  if (!Number.isFinite(local) || local < 0 || !ranges.length) return false;
  const start = ranges.start(0);
  const end = ranges.end(ranges.length - 1);
  // Keep clear of the live edge, where the next segment may not exist yet.
  if (local < start || local > end - 2) return false;
  media.currentTime = local;
  return true;
}

function skipBy(delta: number): void {
  if (castMode === "device" && deviceSeeking) {
    // The TV is still moving; the press runs from where it lands.
    deviceQueued = (deviceQueued ?? deviceSeekTarget) + delta;
    return;
  }
  if (browserCasting() && browserSeeking) {
    browserQueued = (browserQueued ?? browserSeekTarget) + delta;
    return;
  }
  if (skipping) {
    // Keep the press: it runs from where the seek in flight lands.
    if (castMode !== "device" && !browserCasting()) queuedSkip = (queuedSkip ?? skipTarget) + delta;
    return;
  }
  if (el<HTMLInputElement>("dock-bar").disabled) return;
  void skipTo(sourcePosition() + delta);
}

/** The TV's volume as this page knows it: set from here, reported by the TV, or not known. */
function tvVolumeNow(): { level: number | null; muted: boolean } {
  if (tvVolume) return { level: tvVolume.level, muted: tvVolume.muted };
  const level = deviceView?.volume;
  const known = typeof level === "number" && Number.isFinite(level);
  return { level: known ? clampVolume(level) : null, muted: deviceView?.muted === true };
}

/** While a Cast device plays, Volume and Mute are the TV's. A dragged slider sends where it stops. */
function setTvVolume(change: { level?: number; muted?: boolean }): void {
  const id = castReceiverId;
  if (castMode !== "device" || !id) return;
  const now = tvVolumeNow();
  tvVolume = { level: change.level ?? now.level ?? prefs().volume, muted: change.muted ?? now.muted, since: Date.now() };
  tvVolumeSend = { ...tvVolumeSend, ...change };
  paintVolume();
  window.clearTimeout(tvVolumeTimer);
  tvVolumeTimer = window.setTimeout(() => {
    const send = tvVolumeSend;
    tvVolumeSend = {};
    if (castMode !== "device" || castReceiverId !== id) return;
    void api.castDeviceVolume(id, send).catch((err: unknown) => {
      if (castMode === "device" && castReceiverId === id) setStatus(castFail(err));
    });
  }, 300);
}

function toggleMute(): void {
  if (castMode === "device" && castReceiverId) {
    setTvVolume({ muted: !tvVolumeNow().muted });
    return;
  }
  const media = video();
  if (media.muted || prefs().volume <= 0) {
    media.muted = false;
    if (prefs().volume <= 0) {
      setPrefs({ volume: 0.5 });
      el<HTMLInputElement>("dock-volume").value = String(prefs().volume);
      media.volume = prefs().volume;
    }
  } else {
    media.muted = true;
  }
  paintVolume();
}

function nudgeVolume(delta: number): void {
  if (castMode === "device" && castReceiverId) {
    // A step from a level the TV has not reported could jump it to full volume.
    const level = tvVolumeNow().level;
    if (level != null) setTvVolume({ level: clampVolume(Math.round((level + delta) * 20) / 20), muted: false });
    wake();
    return;
  }
  const media = video();
  setPrefs({ volume: clampVolume(Math.round((prefs().volume + delta) * 20) / 20) });
  media.volume = prefs().volume;
  media.muted = false;
  el<HTMLInputElement>("dock-volume").value = String(prefs().volume);
  paintVolume();
  wake();
}

function togglePip(): void {
  const media = video();
  if (document.pictureInPictureElement) void document.exitPictureInPicture().catch(() => undefined);
  else if (media.readyState > 0) void media.requestPictureInPicture().catch(() => undefined);
}

function togglePlay(): void {
  if (skipping) return;
  if (castMode === "device" && castReceiverId) {
    void toggleDevicePlay();
    return;
  }
  if (browserCasting()) {
    void toggleBrowserPlay();
    return;
  }
  if (startFailed && currentTarget && !castMode) {
    // The last try found no stream; Play asks the add-on again.
    play(currentTarget);
    return;
  }
  if (!castMode && !attached && currentJob && lastReadyJob) {
    // The TV played this one; play it here from where the TV left it.
    const at = deviceLeftAt;
    const end = progressDuration();
    if (at > 60 && (end <= 0 || end - at > 30)) void skipTo(at);
    else if (lastReadyJob) attachPlayback(lastReadyJob);
    return;
  }
  const media = video();
  if (media.paused || media.ended) void media.play().catch(() => undefined);
  else media.pause();
}

async function toggleBrowserPlay(): Promise<void> {
  if (!browserCasting() || castBusy) return;
  const paused = browserPlace?.paused ?? false;
  const position = browserPosition() ?? browserPlace?.position ?? 0;
  castBusy = true;
  paintCast();
  try {
    await api.castCommand(paused ? "play" : "pause");
    if (!browserCasting() || !browserPlace) return;
    browserPlace = { ...browserPlace, position, paused: !paused, updatedMs: Date.now() };
    browserSeenAt = Date.now();
    syncPlayButton();
    paintClock();
  } catch (err) {
    setStatus(castFail(err));
  } finally {
    castBusy = false;
    paintCast();
  }
}

/** Asks the browser TV to move to `at` source seconds. The TV performs the seek. */
async function seekBrowser(at: number): Promise<void> {
  if (!browserCasting() || castBusy) return;
  castBusy = true;
  browserSeeking = true;
  browserSeekTarget = at;
  browserSeekSince = Date.now();
  browserQueued = null;
  paintCast();
  paintClock();
  try {
    await api.castCommand("seek", at);
    if (!browserCasting()) {
      browserSeeking = false;
      return;
    }
    if (browserPlace) {
      browserPlace = { ...browserPlace, position: at, updatedMs: Date.now() };
      browserSeenAt = Date.now();
    }
    paintClock();
  } catch (err) {
    browserSeeking = false;
    setStatus(castFail(err));
  } finally {
    castBusy = false;
    paintCast();
    const queued = browserQueued;
    browserQueued = null;
    if (queued != null && browserCasting()) void skipTo(queued);
  }
}

async function toggleDevicePlay(): Promise<void> {
  const id = castReceiverId;
  if (!id || castBusy) return;
  const pause = !devicePaused;
  holdDevicePlay(pause);
  castBusy = true;
  paintCast();
  try {
    if (pause) await api.castDevicePause(id);
    else await api.castDeviceResume(id);
  } catch (err) {
    if (castMode === "device" && castReceiverId === id) {
      holdDevicePlay(!pause);
      deviceWant = null;
    }
    const text = castFail(err);
    setStatus(text);
  } finally {
    castBusy = false;
    paintCast();
  }
}

function toggleFullscreen(): void {
  const stage = el("dock-stage");
  // The minimized stage has no box of its own to fill the screen with.
  if (isMini() && !document.fullscreenElement) setMini(false);
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => undefined);
  else if (!castingAway()) void stage.requestFullscreen().catch(() => undefined);
}

async function skipTo(seconds: number): Promise<void> {
  if (skipping) return;
  const id = currentJob;
  const token = generation;
  if (!id) return;
  const at = clampSkip(seconds, progressDuration());
  if (castMode === "device" && castReceiverId) {
    await seekDevice(castReceiverId, at);
    return;
  }
  if (browserCasting()) {
    await seekBrowser(at);
    return;
  }
  // Inside what this page already has, move in place: instant, no new stream,
  // and quick presses of ±10 add up.
  if (attached && !castMode && seekInPlace(at - origin)) {
    paintClock();
    return;
  }
  skipping = true;
  skipTarget = at;
  queuedSkip = null;
  const mine = ++skipGeneration;
  paintStage();
  video().pause();
  showPlay({ item: currentTarget?.title || "Playback", phase: "skipping", at: formatClock(at) });
  el<HTMLInputElement>("dock-bar").disabled = true;
  syncPlayButton();
  try {
    // The server replaces the stream's files; stop fetching the old ones meanwhile.
    hls?.stopLoad();
    const job = await api.seek(id, at);
    if (token !== generation || mine !== skipGeneration) return;
    skipping = false;
    if (!job.playlist) {
      clearPlayActivity();
      const text = "The stream did not arrive.";
      setStatus(text);
      paintClock();
      return;
    }
    currentNext = job.next ?? null;
    paintNext();
    if (job.title) setDockTitle(job.title);
    attachPlayback(job);
  } catch (err) {
    if (token !== generation || mine !== skipGeneration) return;
    await recoverSkip(id, at, message(err), () => token === generation && mine === skipGeneration);
  } finally {
    if (mine === skipGeneration) {
      skipping = false;
      syncPlayButton();
      const queued = queuedSkip;
      queuedSkip = null;
      if (queued !== null && token === generation) void skipTo(queued);
    }
  }
}

/**
 * The server may have stopped the old stream before a seek failed, and Play
 * cannot start a dead one again. Carry on with the job if it still plays;
 * otherwise ask for the item again from the place that was asked for.
 */
async function recoverSkip(id: string, at: number, failure: string, mine: () => boolean): Promise<void> {
  const live = await api.job(id).catch(() => null);
  if (!mine()) return;
  skipping = false;
  if (live?.state === "ready" && live.playlist) {
    // The same stream means the seek was refused before it was touched.
    const untouched = live.playlist === lastReadyJob?.playlist;
    if (attached && untouched) {
      hls?.startLoad();
      void video().play().catch(() => undefined);
    } else {
      attachPlayback(live);
    }
    clearPlayActivity();
    if (untouched) setStatus(failure);
    paintClock();
    return;
  }
  const target = currentTarget;
  if (!target) {
    clearPlayActivity();
    setStatus(failure);
    paintClock();
    return;
  }
  play({ ...target, resumeSeconds: at > 60 ? at : undefined });
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
  if (!playlist) {
    clearPlayActivity();
    const text = "The stream did not arrive.";
    setStatus(text);
    return;
  }
  origin = originFrom(job);
  jobDuration = job.duration != null && Number.isFinite(job.duration) && job.duration >= 0 ? job.duration : 0;
  destroyHls();
  attached = true;
  lastReadyJob = job;
  fillSubtitles(job.subtitles ?? []);
  paintAudio(job);
  awaitSubtitles(job);
  searchSubtitles(job);
  started = false;
  buffering = false;
  const media = video();
  media.volume = prefs().volume;
  hls = attachHls(media, playlist, (detail) => {
    // Out of retries: say why instead of spinning. Play picks up from here.
    deviceLeftAt = sourcePosition();
    destroyHls();
    attached = false;
    started = false;
    buffering = false;
    const text = streamFailure(detail);
    setStatus(text);
    syncPlayButton();
  });
  // A browser TV plays it while casting; this one stays paused.
  if (castMode !== "browser") void media.play().catch(() => undefined);
  clearPlayActivity();
  setNotice("");
  setStatus("");
  paintClock();
  startTicker(generation);
}

function secretTitle(title: string): boolean {
  const text = title.toLowerCase();
  return text.includes("password") || text.includes("pin") || text.includes("api key") || text.includes("token");
}

/** A keyboard or file window that is asking for a link or a file, for any add-on. */
function wantsLocalFile(prompt: Prompt): boolean {
  if (secretTitle(prompt.title)) return false;
  const text = `${prompt.title} ${prompt.message}`.toLowerCase();
  return /link|url|zip|\bfile\b|path|location|package|browse|http/.test(text);
}

/** Where focus was before an add-on question opened, to go back to after. */
let promptReturn: HTMLElement | null = null;

function hidePrompt(): void {
  shownPrompt = "";
  const root = el("prompt");
  const hadFocus = root.contains(document.activeElement);
  root.classList.remove("open");
  root.replaceChildren();
  const back = promptReturn;
  promptReturn = null;
  if (hadFocus && back?.isConnected) back.focus();
}

/**
 * After a settings RunPlugin action, poll Kodi dialogs into Portal's prompt UI
 * until they clear (or a short idle if nothing appears).
 */
/**
 * Mirrors the dialogs an add-on action opens until they close. A dialog that
 * stays up (a device sign-in waits for the code to be entered) keeps this
 * watching for up to 15 minutes.
 */
export async function awaitAddonPrompts(
  onNote?: (text: string, prompt: Prompt | null) => void,
): Promise<"prompted" | "idle" | "settings"> {
  const started = Date.now();
  const limit = started + 15 * 60_000;
  let deadline = started + 120_000;
  let saw = false;
  let clearSince: number | null = null;
  while (Date.now() < deadline) {
    let prompt: Prompt | null = null;
    try {
      const read = await api.prompt();
      prompt = read.prompt;
      // The add-on opened its settings in Kodi: Portal's own form takes over.
      if (read.settings_addon) {
        hidePrompt();
        location.hash = settingsHref(read.settings_addon);
        return "settings";
      }
    } catch {
      await sleep(400);
      continue;
    }
    if (prompt) {
      saw = true;
      clearSince = null;
      deadline = Math.min(limit, Math.max(deadline, Date.now() + 120_000));
      if (prompt.kind === "progress") {
        hidePrompt();
        onNote?.(progressNote(prompt), prompt);
      } else if (promptHasWords(prompt)) {
        renderPrompt(prompt);
        onNote?.(prompt.title.trim() || "Waiting for your answer…", null);
      } else {
        hidePrompt();
        onNote?.("Working…", null);
      }
    } else {
      hidePrompt();
      if (saw) {
        if (clearSince === null) clearSince = Date.now();
        else if (Date.now() - clearSince > 1500) return "prompted";
      } else if (Date.now() - started > 4000) {
        return "idle";
      }
    }
    await sleep(400);
  }
  hidePrompt();
  return saw ? "prompted" : "idle";
}

function progressNote(prompt: Prompt): string {
  const title = prompt.title.trim();
  const message = prompt.message.trim();
  if (title && message && title !== message) return `${title}\n${message}`;
  return title || message || "Working…";
}

function button(label: string, onClick: () => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.textContent = label;
  node.addEventListener("click", onClick);
  return node;
}

/**
 * True while an answer is on its way. Kodi's dialog stays open until Portal
 * has worked the answer through it, so a poll meanwhile still reports it; it
 * must not come back up and take a second answer.
 */
let answering = false;

async function answer(prompt: Prompt, action: string, extra?: { text?: string; index?: number }): Promise<void> {
  if (answering) return;
  hidePrompt();
  // A cancelled link choice goes back to the item as it was, from where it was.
  const back = action === "cancel" && prompt.kind === "select" && currentTarget?.manualLinks ? currentTarget : null;
  answering = true;
  try {
    await api.answerPrompt({ id: prompt.id, action, ...extra });
  } catch (err) {
    setStatus(message(err));
  } finally {
    answering = false;
  }
  if (back && back === currentTarget) play({ ...back, manualLinks: false });
}

function promptHasWords(prompt: Prompt): boolean {
  if (prompt.title.trim()) return true;
  if (prompt.message.trim()) return true;
  return (prompt.options ?? []).some((option) => option.trim());
}

function renderPrompt(prompt: Prompt | null): void {
  const root = el("prompt");
  if (!prompt || prompt.kind === "progress" || !promptHasWords(prompt)) {
    hidePrompt();
    return;
  }
  hideCastMenu();
  if (answering || prompt.id === shownPrompt) return;
  shownPrompt = prompt.id;
  // The question sits outside the player; in fullscreen it would be answered unseen,
  // and under an open Info panel it could not be answered at all.
  if (document.fullscreenElement) void document.exitFullscreen().catch(() => undefined);
  if (infoOpen()) closeInfo(false);
  root.classList.add("open");
  root.replaceChildren();
  const card = document.createElement("form");
  card.className = "prompt-card";
  card.setAttribute("role", "dialog");
  card.setAttribute("aria-modal", "true");
  let field: HTMLInputElement | null = null;
  // Enter in the text field answers the prompt, like the Submit button.
  card.addEventListener("submit", (event) => {
    event.preventDefault();
    if (field) void answer(prompt, "submit", { text: field.value });
  });
  const heading = prompt.title.trim() || "The add-on is asking";
  const title = document.createElement("h2");
  title.textContent = heading;
  card.setAttribute("aria-label", heading);
  card.append(title);
  if (prompt.message) {
    const copy = document.createElement("p");
    copy.className = "prompt-copy";
    copy.textContent = prompt.message;
    card.append(copy);
  }
  const actions = document.createElement("div");
  actions.className = "prompt-actions";
  let choices: HTMLElement | null = null;
  if (prompt.kind === "text") {
    const input = document.createElement("input");
    input.type = secretTitle(prompt.title) ? "password" : "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.setAttribute("aria-label", heading);
    field = input;
    card.append(input);
    actions.append(button("Submit", () => void answer(prompt, "submit", { text: input.value })));
    if (wantsLocalFile(prompt)) {
      const file = document.createElement("input");
      file.type = "file";
      file.accept = ".zip,.xml,.json,.txt";
      file.setAttribute("aria-label", "File on this PC");
      file.addEventListener("change", () => {
        const chosen = file.files?.[0];
        if (!chosen) return;
        file.disabled = true;
        void api
          .uploadIncoming(chosen)
          .then((path) => answer(prompt, "submit", { text: path }))
          .catch((err) => {
            file.disabled = false;
            setStatus(message(err));
          });
      });
      card.append(file);
    }
  } else if (prompt.kind === "select") {
    const list = document.createElement("div");
    list.className = "prompt-list";
    list.setAttribute("role", "listbox");
    list.setAttribute("aria-label", heading);
    (prompt.options ?? []).forEach((label, index) => {
      const choice = button(label, () => void answer(prompt, "submit", { index }));
      choice.setAttribute("role", "option");
      list.append(choice);
    });
    card.append(list);
    choices = list;
  } else if (prompt.kind === "yesno") {
    actions.append(button("Yes", () => void answer(prompt, "yes")));
    actions.append(button("No", () => void answer(prompt, "no")));
  } else if (prompt.kind === "ok") {
    actions.append(button("Ok", () => void answer(prompt, "ok")));
  }
  if (prompt.cancellable) actions.append(button("Cancel", () => void answer(prompt, "cancel")));
  card.append(actions);
  // Escape cancels, like any dialog, when the add-on allows it.
  card.addEventListener("keydown", (event) => {
    if (event.key === "Tab") {
      // A modal question keeps Tab inside it, like the Info dialog.
      const items = Array.from(card.querySelectorAll<HTMLElement>("input, button"));
      const first = items[0];
      const last = items[items.length - 1];
      if (first && last && event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (first && last && !event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
      return;
    }
    if (event.key !== "Escape" || !prompt.cancellable) return;
    event.preventDefault();
    event.stopPropagation();
    void answer(prompt, "cancel");
  });
  root.append(card);
  if (!promptReturn && document.activeElement instanceof HTMLElement && !root.contains(document.activeElement)) {
    promptReturn = document.activeElement;
  }
  if (field) field.focus();
  else (choices?.querySelector("button") ?? actions.querySelector("button"))?.focus();
}

function playBody(target: PlayTarget): Record<string, unknown> {
  const body: Record<string, unknown> = {
    kind: target.kind,
    id: target.id,
    file: target.file,
    title: target.title,
    quality: prefs().quality,
  };
  if (target.season != null) body.season = target.season;
  if (target.episode) body.episode = target.episode;
  if (target.art) body.art = target.art;
  if (target.showTitle) body.show_title = target.showTitle;
  if (target.showId) body.show_id = target.showId;
  if (target.resumeSeconds != null && target.resumeSeconds > 60) body.resume_seconds = target.resumeSeconds;
  if (target.manualLinks) body.manual_links = true;
  const audioLanguage = startupAudioLanguage(prefs().audioLanguage);
  if (audioLanguage) body.audio_language = audioLanguage;
  return body;
}

export function targetFromNext(next: NextItem): PlayTarget {
  const episode = next.kind === "episode";
  const showId = next.show_id || next.id;
  return {
    kind: next.kind,
    id: episode ? showId : next.id,
    file: next.file,
    title: next.title,
    season: next.season ?? undefined,
    episode: next.episode ?? undefined,
    art: next.art,
    showTitle: humanTitle(next.show_title) || undefined,
    showId: episode ? showId : undefined,
    progressKind: episode ? "tv" : "movie",
    progressId: episode ? showId : next.id,
    progressName: episode ? humanTitle(next.show_title) : humanTitle(next.title),
    trackProgress: next.kind === "movie" || next.kind === "episode",
  };
}

async function releaseCurrent(report: boolean): Promise<void> {
  // Teardown clears the play ticket. The episode list must not browse the show that is leaving.
  episodeWait = false;
  const id = currentJob;
  const deviceId = castMode === "device" ? castReceiverId : null;
  skipGeneration += 1;
  skipping = false;
  queuedSkip = null;
  // Build the final progress post now (a device cast's place included); the request runs
  // after every field below is cleared, so a play() that starts while it is in flight is
  // never reset by this release.
  const posted = report ? postProgress(true).catch(() => undefined) : Promise.resolve();
  currentJob = null;
  castReceiverId = null;
  castMode = null;
  pendingRecast = null;
  deviceLeftAt = 0;
  devicePaused = false;
  leaveDeviceState();
  leaveBrowserState();
  currentNext = null;
  sawProgress = false;
  canceling = false;
  cancelPromptId = "";
  clearPlayActivity();
  stopTicker();
  currentTarget = null;
  lastReadyJob = null;
  resetSubtitles();
  hideSubtitleMenu();
  window.clearTimeout(audioTimer);
  audioTimer = 0;
  audioPending = false;
  paintAudio(null);
  paintSaveButtons();
  origin = 0;
  jobDuration = 0;
  attached = false;
  started = false;
  buffering = false;
  el("dock-upnext").hidden = true;
  destroyHls();
  hidePrompt();
  hideCastMenu();
  paintNext();
  paintLinks();
  watchCastSession();
  resetClock();
  syncPlayButton();
  if (deviceId) void api.castDeviceStop(deviceId).catch(() => undefined);
  await posted;
  if (id) await api.stop(id).catch(() => undefined);
}

async function poll(token: number, jobId: string): Promise<void> {
  let misses = 0;
  while (token === generation) {
    let job: PlayJob;
    try {
      job = await api.job(jobId);
      misses = 0;
    } catch (err) {
      if (token !== generation) return;
      // Portal can be slow to answer while Kodi is busy; one miss is not the end of the job.
      if (++misses < 4) {
        await sleep(1000);
        continue;
      }
      hidePrompt();
      clearPlayActivity();
      failStart(message(err));
      return;
    }
    if (token !== generation) return;
    if (job.state === "working" || job.state === "prompt") {
      try {
        // The job already read Kodi's dialog. Asking again doubles that work
        // every quarter second while a stream is resolving.
        const prompt = job.prompt_known ? (job.prompt ?? null) : (await api.prompt()).prompt;
        if (token !== generation) return;
        paintPlayStep(token, job, prompt);
      } catch {
        /* The next poll will try again. A prompt failure is not the end of the job. */
      }
      // Short, so a stream starts playing soon after it is ready.
      await sleep(250);
      continue;
    }
    hidePrompt();
    if (job.state === "browse" && job.browse_path) {
      const path = job.browse_path;
      // The add-on handed back a folder. Listing episodes now would browse Kodi again on top of that.
      episodeWait = false;
      // A manual link choice the add-on answered with a folder: its sources.
      const picking = currentTarget?.manualLinks ? currentTarget : undefined;
      // Folder/source-list result — drop stream-resolve copy before navigating.
      clearPlayActivity();
      setStatus("");
      const mine = ++generation;
      await releaseCurrent(false);
      if (mine !== generation) return;
      hideDock();
      onBrowse(path, picking);
      return;
    }
    if (job.state === "failed") {
      clearPlayActivity();
      failStart(job.error || "Playback failed.");
      return;
    }
    if (job.state === "ready" && job.playlist) {
      currentNext = job.next ?? null;
      paintNext();
      if (job.title) setDockTitle(job.title);
      const recast = pendingRecast;
      pendingRecast = null;
      if (recast) {
        lastReadyJob = job;
        origin = originFrom(job);
        jobDuration = job.duration != null && Number.isFinite(job.duration) && job.duration >= 0 ? job.duration : 0;
        clearPlayActivity();
        paintClock();
        void castChoice(recast);
        return;
      }
      if (castMode === "device" || castMode === "browser") {
        // The TV plays it. The dock follows and does not start the video here.
        lastReadyJob = job;
        paintAudio(job);
        jobDuration = job.duration != null && Number.isFinite(job.duration) && job.duration >= 0 ? job.duration : 0;
        clearPlayActivity();
        paintClock();
        if (castMode === "browser" && castReceiverId && !browserClock) followReceiver(castReceiverName || "the TV");
        return;
      }
      attachPlayback(job);
      return;
    }
    await sleep(500);
  }
}

/** Where focus was when playback started, to go back to when the dock closes. */
let dockOpener: HTMLElement | null = null;

export function play(target: PlayTarget): void {
  void loadHls();
  const active = document.activeElement;
  if (active instanceof HTMLElement && !el("dock").contains(active) && active !== document.body) dockOpener = active;
  // A fresh start with nothing focused must not hand focus to a control from an earlier play.
  else if (!el("dock").classList.contains("open")) dockOpener = null;
  const token = ++generation;
  void start(target, token);
}

/** A play picked from the library opens the full player. Next episode and autoplay keep it minimized. */
export function playFromLibrary(target: PlayTarget): void {
  play(target);
  // After play, so the episode hold is set before theater (if it comes back) asks for the list.
  setMini(false);
}

function paintPlayStep(token: number, job: PlayJob, prompt: Prompt | null): void {
  if (!currentTarget || token !== generation) {
    clearPlayActivity();
    return;
  }
  const item = job.title || currentTarget.title || "";
  const links = playLinks();
  if (prompt?.kind === "progress") {
    hidePrompt();
    sawProgress = true;
    if (prompt.id !== cancelPromptId) {
      canceling = false;
      cancelPromptId = "";
    }
    const progressId = prompt.id;
    const described = {
      item,
      phase: "working" as const,
      heading: prompt.title,
      message: prompt.message,
      progress: prompt.progress,
      canceling,
      links,
    };
    showPlay(
      described,
      prompt.cancellable && !canceling
        ? () => {
            if (token !== generation) return;
            canceling = true;
            cancelPromptId = progressId;
            showPlay({ ...described, canceling: true }, null, true);
            void api.answerPrompt({ id: progressId, action: "cancel" }).catch((err) => {
              if (token !== generation) return;
              const text = message(err);
              setStatus(text);
            });
          }
        : null,
      canceling,
    );
    return;
  }
  if (prompt && promptHasWords(prompt)) {
    renderPrompt(prompt);
    showPlay({ item, phase: "answering", heading: prompt.title, links });
    return;
  }
  hidePrompt();
  showPlay({ item, phase: sawProgress ? "preparing" : "waiting", links });
}

async function start(target: PlayTarget, token: number): Promise<void> {
  const report = !!currentTarget?.trackProgress;
  // Next, a quality change or another link while casting goes to the same TV,
  // and so does Play after one of those could not start.
  const recast: CastChoice | null =
    castMode && castReceiverId
      ? { kind: castMode, id: castReceiverId, name: castReceiverName || "the TV" }
      : pendingRecast;
  // Files.GetDirectory during a resolve steals Kodi from the source search. Hold the list until this item is on screen.
  const hold = ++episodeHold;
  holdEpisodes = true;
  episodeWait = false;
  const links = !!target.manualLinks;
  try {
    await releaseCurrent(report);
    if (token !== generation) return;
    pendingRecast = recast;
    startFailed = false;
    currentTarget = target;
    paintSaveButtons();
    upNextDismissed = "";
    origin = appliedSeek(target.resumeSeconds);
    sawProgress = false;
    canceling = false;
    cancelPromptId = "";
    showDock(target.title);
    showPlay({ item: target.title, phase: "opening", links });
  } finally {
    if (hold === episodeHold) {
      holdEpisodes = false;
      if (token === generation) requestEpisodes();
    }
  }
  if (token !== generation) return;
  setNotice("");
  // Quality and subtitles as last chosen on any screen, not only this one.
  const shared = syncPrefs();
  try {
    const status = await api.status();
    if (token !== generation) return;
    if (status.state !== "running") showPlay({ item: target.title, phase: "starting", links });
  } catch {
    if (token !== generation) return;
    showPlay({ item: target.title, phase: "starting", links });
  }
  await shared;
  if (token !== generation) return;
  let accepted: { id: string };
  try {
    accepted = await api.play(playBody(target));
  } catch (err) {
    if (token !== generation) return;
    clearPlayActivity();
    failStart(message(err));
    return;
  }
  if (token !== generation) {
    await api.stop(accepted.id).catch(() => undefined);
    return;
  }
  currentJob = accepted.id;
  paintLinks();
  watchCastSession();
  await poll(token, accepted.id);
}

function castMenu(): HTMLElement {
  return el("dock-cast-menu");
}

function castMenuOpen(): boolean {
  return !castMenu().hidden;
}

function hideCastMenu(restoreFocus = false): void {
  const menu = castMenu();
  const wasOpen = !menu.hidden;
  menu.hidden = true;
  menu.replaceChildren();
  const node = document.getElementById("dock-cast");
  if (node) node.setAttribute("aria-expanded", "false");
  if (restoreFocus && wasOpen && node instanceof HTMLButtonElement) node.focus();
}

function menuItems(menu: HTMLElement): HTMLButtonElement[] {
  return Array.from(menu.querySelectorAll<HTMLButtonElement>('button[role^="menuitem"]'));
}

function focusMenuItem(menu: HTMLElement, index: number): void {
  const items = menuItems(menu);
  if (!items.length) return;
  const at = Math.max(0, Math.min(index, items.length - 1));
  items[at]?.focus();
}

/** Arrow / Home / End / Escape for dock Cast and Subtitles menus. */
function handleDockMenuKey(event: KeyboardEvent, menu: HTMLElement, close: () => void): boolean {
  const key = event.key;
  if (key === "Escape") {
    event.preventDefault();
    close();
    return true;
  }
  const items = menuItems(menu);
  if (!items.length) return false;
  if (key === "ArrowDown" || key === "ArrowUp" || key === "Home" || key === "End") {
    event.preventDefault();
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    let next = current;
    if (key === "Home") next = 0;
    else if (key === "End") next = items.length - 1;
    else if (key === "ArrowDown") next = current < 0 ? 0 : (current + 1) % items.length;
    else next = current < 0 ? items.length - 1 : (current - 1 + items.length) % items.length;
    items[next]?.focus();
    return true;
  }
  if (key === "Tab") {
    close();
    return false;
  }
  return false;
}

function paintCast(): void {
  const node = el<HTMLButtonElement>("dock-cast");
  const casting = !!castReceiverId && !!currentJob;
  const label = casting ? "Casting" : "Cast";
  node.disabled = castBusy || castScanning || !currentJob;
  node.textContent = label;
  node.title = casting ? "Casting" : currentJob ? "Cast this video" : "Cast";
  node.setAttribute("aria-label", label);
  if (!castMenuOpen()) node.setAttribute("aria-expanded", "false");
  node.classList.toggle("is-casting", casting);
}

function watchCastSession(): void {
  const token = ++castWatch;
  const job = currentJob;
  if (!job) {
    castReceiverId = null;
    paintCast();
    return;
  }
  void loadCastSession()
    .then((session) => {
      if (token !== castWatch || currentJob !== job) return;
      if (castMode === "device") {
        paintCast();
        return;
      }
      castReceiverId = activeReceiver(session, job);
      castMode = castReceiverId ? "browser" : null;
      paintCast();
    })
    .catch(() => {
      if (token !== castWatch) return;
      paintCast();
    });
}

function castPageUrl(url: string): string {
  const trimmed = url.trim();
  if (!trimmed) return "";
  if (/#\/cast\/?$/.test(trimmed)) return trimmed;
  return trimmed.replace(/\/+$/, "") + "/#/cast";
}

function castFail(err: unknown): string {
  if (err instanceof Error && err.message.trim()) return err.message;
  return "Cast failed.";
}

function textField(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim() : "";
}

function activeReceiver(session: CastSession, job: string | null): string | null {
  const nested = session.session && typeof session.session === "object" ? session.session : session;
  const receiverId = textField(nested.receiver_id) || textField(session.receiver_id);
  const jobId = textField(nested.job_id) || textField(session.job_id);
  if (!receiverId || !job) return null;
  if (jobId && jobId !== job) return null;
  return receiverId;
}

async function loadReceivers(): Promise<CastReceiver[]> {
  const body = await api.castReceivers();
  return (body.items ?? []).filter((item) => textField(item.id)).map((item) => ({
    id: item.id.trim(),
    name: textField(item.name) || item.id.trim(),
  }));
}

async function loadDevices(): Promise<CastDevice[]> {
  const body = await api.castDevices();
  return (body.items ?? []).filter((item) => textField(item.id)).map((item) => ({
    id: item.id.trim(),
    name: deviceLabel(item),
    host: textField(item.host),
    model: textField(item.model),
  }));
}

function deviceLabel(device: CastDevice): string {
  const name = textField(device.name) || "Cast device";
  const model = textField(device.model);
  if (!model || name.toLowerCase().includes(model.toLowerCase())) return name;
  return name + " · " + model;
}

async function loadCastSession(): Promise<CastSession> {
  return api.castSession();
}

async function loadCastUrl(): Promise<string> {
  const info = await api.castInfo();
  const url = (info as { url?: unknown }).url;
  return typeof url === "string" ? castPageUrl(url) : "";
}

function menuButton(label: string, onClick: () => void): HTMLButtonElement {
  const node = button(label, onClick);
  node.setAttribute("role", "menuitem");
  return node;
}

type CastChoice =
  | { kind: "browser"; id: string; name: string }
  | { kind: "device"; id: string; name: string };

function showCastMenu(choices: CastChoice[], casting: boolean): void {
  const menu = castMenu();
  menu.replaceChildren();
  if (casting) menu.append(menuButton("Stop casting", () => void stopCasting()));
  choices.forEach((choice) => {
    const current =
      choice.id === castReceiverId &&
      (choice.kind === "device" ? castMode === "device" : castMode !== "device");
    const label = current ? choice.name + " · playing" : choice.name;
    menu.append(menuButton(label, () => (current ? hideCastMenu(true) : void castChoice(choice))));
  });
  menu.append(menuButton("Cancel", () => hideCastMenu(true)));
  menu.hidden = false;
  el("dock-cast").setAttribute("aria-expanded", "true");
  focusMenuItem(menu, 0);
}

function castChoices(devices: CastDevice[], receivers: CastReceiver[]): CastChoice[] {
  const deviceIds = new Set(devices.map((device) => device.id));
  const choices: CastChoice[] = devices.map((device) => ({
    kind: "device",
    id: device.id,
    name: device.name,
  }));
  receivers.forEach((receiver) => {
    if (deviceIds.has(receiver.id)) return;
    choices.push({ kind: "browser", id: receiver.id, name: receiver.name });
  });
  return choices;
}

function castMediaUrl(playlist: string, lanBase: string): string {
  const path = playlist.trim();
  const base = lanBase.trim().replace(/\/+$/, "");
  if (!path) return "";
  if (/^https?:\/\//i.test(path)) {
    try {
      const url = new URL(path);
      const host = url.hostname.replace(/^\[|\]$/g, "").toLowerCase();
      if ((host === "localhost" || host === "127.0.0.1" || host === "::1") && base) {
        const root = new URL(base + "/");
        url.protocol = root.protocol;
        url.host = root.host;
      }
      return url.toString();
    } catch {
      return "";
    }
  }
  if (!base) return "";
  return base + (path.startsWith("/") ? path : "/" + path);
}

function castThumb(art: string | null | undefined, lanBase: string): string | undefined {
  const resolved = artUrl(art);
  if (!resolved) return undefined;
  if (/^https?:\/\//i.test(resolved)) {
    const absolute = castMediaUrl(resolved, lanBase);
    return absolute || undefined;
  }
  if (resolved.startsWith("/")) {
    const absolute = castMediaUrl(resolved, lanBase);
    return absolute || undefined;
  }
  return undefined;
}

async function castOpenHint(): Promise<string> {
  try {
    const page = await loadCastUrl();
    if (page) return "No Cast device answered. Open " + page + " on a browser TV.";
  } catch {
    /* The status line still tells them to open the cast page. */
  }
  return "No Cast device answered. Open http://…/#/cast on a browser TV.";
}

function castReplayFromTarget(target: PlayTarget | null): CastReplay | null {
  if (!target?.file?.trim()) return null;
  return {
    kind: target.kind,
    id: target.id,
    file: target.file,
    title: target.title || "Playback",
    season: target.season,
    episode: target.episode,
    art: target.art,
    showTitle: target.showTitle,
    showId: target.showId,
  };
}

async function castChoice(choice: CastChoice): Promise<void> {
  if (choice.kind === "device") {
    await castToDevice(choice);
    return;
  }
  await castTo({ id: choice.id, name: choice.name });
}

async function castTo(receiver: CastReceiver): Promise<void> {
  hideCastMenu();
  const job = currentJob;
  if (!job) return;
  const title = (currentTarget?.title || el("dock-title").textContent || "").trim();
  const replay = castReplayFromTarget(currentTarget);
  const previous = castMode === "device" ? castReceiverId : null;
  castBusy = true;
  paintCast();
  try {
    if (previous && previous !== receiver.id) await api.castDeviceStop(previous).catch(() => undefined);
    await api.setCastSession(receiver.id, job, title || undefined, replay);
    if (currentJob !== job) return;
    const at = sourcePosition();
    const dur = progressDuration();
    castReceiverId = receiver.id;
    castReceiverName = receiver.name;
    castMode = "browser";
    devicePaused = false;
    // The stream already includes where we are once that is past the first minute.
    // A shorter seek is ignored by the server, so the TV starts with the stream.
    const tvAt = at > 60 ? at : origin;
    browserPlace = { position: tvAt, duration: dur, paused: false, updatedMs: Date.now() };
    browserSeenAt = Date.now();
    browserSeeking = false;
    browserQueued = null;
    // The TV has the stream now; stop loading it here so Play cannot start it again.
    if (attached) destroyHls();
    attached = false;
    syncPlayButton();
    setStatus("Casting to " + receiver.name + ".");
    setNotice("");
    if (at > 60) {
      try {
        await api.castCommand("seek", at);
      } catch (err) {
        if (currentJob === job) setStatus(castFail(err));
      }
    }
    if (currentJob !== job) return;
    leaveDeviceState();
    followReceiver(receiver.name);
  } catch (err) {
    if (currentJob !== job) return;
    const text = castFail(err);
    setStatus(text);
  } finally {
    castBusy = false;
    paintCast();
  }
}

/** Seconds into `job`'s stream (which starts at its position in the source) where a cast should begin. */
function castStart(job: PlayJob): number {
  const base = job.position != null && Number.isFinite(job.position) && job.position >= 0 ? job.position : origin;
  const here = devicePosition() ?? (attached ? origin + video().currentTime : deviceLeftAt);
  return Number.isFinite(here) ? Math.max(0, here - base) : 0;
}

async function castToDevice(device: { id: string; name: string }): Promise<void> {
  hideCastMenu();
  const jobId = currentJob;
  if (!jobId) return;
  const previous = castMode === "device" && castReceiverId && castReceiverId !== device.id ? castReceiverId : null;
  castBusy = true;
  paintCast();
  try {
    const [job, info] = await Promise.all([api.job(jobId), api.castInfo()]);
    if (currentJob !== jobId) return;
    const playlist = typeof job.playlist === "string" ? job.playlist : "";
    const base = typeof info.url === "string" ? info.url.trim() : "";
    const url = castMediaUrl(playlist, base);
    if (!url) {
      const text = base
        ? "The stream is not ready to cast."
        : !info.lan
          ? "Turn on Allow TVs on this network, then cast again."
          : "Portal could not find this PC's network address. If you just turned on Allow TVs, restart Portal; otherwise check this PC is on the TV's network.";
      setStatus(text);
      return;
    }
    const title = (currentTarget?.title || job.title || el("dock-title").textContent || "").trim();
    const thumb = castThumb(currentTarget?.art, base);
    // The TV picks up where things are now (this browser, another TV, or where a cast stopped).
    const start = castStart(job);
    if (previous) await api.castDeviceStop(previous).catch(() => undefined);
    await api.castDevicePlay(
      device.id,
      {
        url,
        title: title || undefined,
        thumb,
        job_id: jobId,
        start,
        subtitle: subtitlesOn() ? (castSubtitle() ?? undefined) : undefined,
      },
      castReplayFromTarget(currentTarget),
    );
    if (currentJob !== jobId) return;
    castReceiverId = device.id;
    castReceiverName = device.name;
    castMode = "device";
    devicePaused = false;
    leaveBrowserState();
    // The TV has the stream now; stop loading it here. Play after the cast
    // picks up from where the TV left it.
    if (attached) destroyHls();
    attached = false;
    syncPlayButton();
    setStatus("Casting to " + device.name + ".");
    setNotice("");
    followDevice(device.name);
  } catch (err) {
    if (currentJob !== jobId) return;
    const text = castFail(err);
    setStatus(text);
  } finally {
    castBusy = false;
    paintCast();
  }
}

async function stopCasting(): Promise<void> {
  hideCastMenu();
  const deviceId = castMode === "device" ? castReceiverId : null;
  castBusy = true;
  paintCast();
  try {
    if (deviceId) await api.castDeviceStop(deviceId);
    if (!deviceId) await api.clearCastSession();
    else await api.clearCastSession().catch(() => undefined);
    const left = deviceId ? devicePosition() : browserPosition();
    castReceiverId = null;
    castMode = null;
    devicePaused = false;
    if (left != null) deviceLeftAt = left;
    leaveDeviceState();
    leaveBrowserState();
    syncPlayButton();
    paintClock();
    setStatus("Stopped casting.");
    setNotice("");
  } catch (err) {
    const text = castFail(err);
    setStatus(text);
  } finally {
    castBusy = false;
    paintCast();
  }
}

/*
 * Following a cast. The TV moves on to the next episode by itself: a browser
 * TV starts the job and names it in the cast session, and for a Cast device
 * Portal's watch does (`/api/cast/device`). The dock polls whichever applies
 * and takes on the new job, so Stop, the clock, and the title stay right.
 */
const DEVICE_POLL = 3000;
const RECEIVER_POLL = 1000;
const PLAY_KINDS: PlayKind[] = ["movie", "episode", "song", "file"];

type CastItem = {
  kind?: string | null;
  id?: string | null;
  file?: string | null;
  title?: string | null;
  season?: number | null;
  episode?: number | null;
  art?: string | null;
  show_title?: string | null;
  show_id?: string | null;
};

function targetFromCast(item: CastItem | null | undefined): PlayTarget | null {
  const file = textField(item?.file);
  const id = textField(item?.id);
  const kind = PLAY_KINDS.find((known) => known === item?.kind);
  if (!item || !file || !id || !kind) return null;
  return targetFromNext({
    kind,
    id,
    file,
    title: textField(item.title),
    season: item.season,
    episode: item.episode,
    art: item.art,
    show_title: item.show_title,
    show_id: item.show_id,
  });
}

/** Stops following a device cast; the clock goes back to the dock's own video. */
function leaveDeviceState(): void {
  deviceWatch += 1;
  deviceLandSince = 0;
  if (deviceClock) window.clearInterval(deviceClock);
  deviceClock = 0;
  deviceView = null;
  deviceSeenAt = 0;
  deviceFreshAt = 0;
  deviceWant = null;
  deviceLine = "";
  tvVolume = null;
  tvVolumeSend = {};
  window.clearTimeout(tvVolumeTimer);
  paintVolume();
}

/** The TV moved on to `jobId`. The dock shows it and no longer plays the last one. */
function adoptCastJob(jobId: string, item: CastItem | null | undefined, title: string | null | undefined): void {
  currentJob = jobId;
  currentTarget = targetFromCast(item) ?? currentTarget;
  lastReadyJob = null;
  deviceLeftAt = 0;
  currentNext = null;
  origin = 0;
  jobDuration = 0;
  upNextDismissed = "";
  if (castMode === "browser") {
    browserPlace = { position: 0, duration: 0, paused: false, updatedMs: Date.now() };
    browserSeenAt = Date.now();
    browserSeeking = false;
    browserQueued = null;
  }
  if (attached) destroyHls();
  attached = false;
  started = false;
  resetSubtitles();
  clearPlayActivity();
  setDockTitle(textField(title) || currentTarget?.title || "");
  paintSaveButtons();
  paintNext();
  paintLinks();
  paintClock();
  applyTheater();
  void loadCastJob(jobId);
}

/** The cast job being looked up, so a device poll does not start a second look and a second subtitle wait. */
let castJobLoading: string | null = null;

/** Fills in the next episode and length of a job the TV started. */
async function loadCastJob(jobId: string): Promise<void> {
  if (castJobLoading === jobId) return;
  castJobLoading = jobId;
  try {
    const job = await api.job(jobId);
    if (currentJob !== jobId) return;
    if (job.state === "ready" && job.playlist) {
      lastReadyJob = job;
      // The CC menu lists the TV's subtitles for this episode too.
      if (!attached) currentSubs = job.subtitles ?? [];
      searchSubtitles(job);
      awaitSubtitles(job);
    }
    currentNext = job.next ?? null;
    if (job.duration != null && Number.isFinite(job.duration) && job.duration > 0) jobDuration = job.duration;
    paintNext();
    paintClock();
  } catch {
    /* The next look at the cast tries again. */
  } finally {
    if (castJobLoading === jobId) castJobLoading = null;
  }
}

/** The cast ended on the TV (or was replaced). The dock stays on the item, ready to play here. */
function leaveBrowserClock(): void {
  if (browserClock) window.clearInterval(browserClock);
  browserClock = 0;
}

function leaveBrowserState(): void {
  leaveBrowserClock();
  browserPlace = null;
  browserSeenAt = 0;
  browserSeeking = false;
  browserSeekTarget = 0;
  browserSeekSince = 0;
  browserQueued = null;
  receiverWatch += 1;
}

function castEnded(text: string): void {
  const left = devicePosition() ?? browserPosition();
  if (left != null) deviceLeftAt = left;
  castReceiverId = null;
  castMode = null;
  devicePaused = false;
  leaveDeviceState();
  leaveBrowserState();
  syncPlayButton();
  paintCast();
  paintClock();
  setStatus(text);
}

function followDevice(name: string): void {
  leaveDeviceState();
  const token = deviceWatch;
  const deviceId = castReceiverId;
  // Between looks the clock moves on by itself while the TV plays.
  deviceClock = window.setInterval(() => {
    if (token === deviceWatch && !dragging) paintClock();
  }, 1000);
  void (async () => {
    while (token === deviceWatch) {
      try {
        const view = await api.castDevice();
        if (token !== deviceWatch || castMode !== "device" || castReceiverId !== deviceId) return;
        applyDeviceView(view, name);
      } catch {
        /* Portal did not answer this time; the next look tries again. */
      }
      await sleep(DEVICE_POLL);
    }
  })();
}

function applyDeviceView(view: DeviceCast, name: string): void {
  const jobId = textField(view.job_id);
  if (!view.device_id) {
    // Portal is not watching this cast any more (it restarted, or the job had gone); the TV may still play it.
    castEnded("Portal stopped following " + name + ". It may still be playing there; cast again to control it here.");
    return;
  }
  if (view.device_id !== castReceiverId) {
    castEnded("Stopped casting to " + name + ".");
    return;
  }
  if (jobId && jobId !== currentJob) adoptCastJob(jobId, view.item, view.title);
  else if (jobId && !lastReadyJob && view.state !== "next") void loadCastJob(jobId);
  const previous = deviceView;
  const want = deviceWant;
  const waiting =
    view.active && !!want && (view.state === "paused") !== want.paused && Date.now() - want.since < DEVICE_WANT_MS;
  if (waiting && deviceView) {
    // The TV has not shown the last Play or Pause yet; this older report would undo it on screen.
    deviceView = { ...view, state: deviceView.state, position: deviceView.position, updated_ms: deviceView.updated_ms };
  } else {
    deviceWant = null;
    // A report seen before moves nothing on: the clock runs on from the first sight of it.
    const fresh = !deviceView || deviceView.updated_ms !== view.updated_ms;
    const age = Date.now() - view.updated_ms;
    deviceView = view;
    if (fresh) {
      deviceFreshAt = Date.now();
      deviceSeenAt = age >= 0 && age < 10_000 ? view.updated_ms : Date.now();
    }
  }
  const shown = (seen: DeviceCast | null): string => {
    const sub = seen?.subtitle;
    return (sub ? sub.kind + ":" + sub.id : "") + (seen?.subtitle_pending ? "…" : "");
  };
  if (shown(view) !== shown(previous)) {
    paintSubtitleState();
    if (subtitleMenuOpen()) showSubtitleMenu(focusedMenuIndex());
  }
  // The TV's own volume, once a change from here has had time to reach it.
  if (tvVolume && Date.now() - tvVolume.since > 6000 && typeof view.volume === "number") tvVolume = null;
  paintVolume();
  if (deviceLandSince && !castBusy) {
    const there =
      (view.state === "playing" || view.state === "paused") &&
      typeof view.position === "number" &&
      Math.abs(view.position - deviceSeekTarget) < 4;
    if (there || Date.now() - deviceLandSince > DEVICE_LAND_MS) settleDeviceSeek(view.device_id ?? "");
  }
  if (!view.active) {
    const failed = view.state === "failed";
    castEnded(textField(view.note) || (failed ? "Casting failed." : "Casting ended."));
    return;
  }
  const paused = waiting && want ? want.paused : view.state === "paused";
  if (paused !== devicePaused) {
    devicePaused = paused;
    syncPlayButton();
  }
  // Written when it changes, so an error or a subtitle note stays until there is news.
  const line = deviceStatusLine(view.state, name, Date.now() - deviceFreshAt > DEVICE_STALE_MS);
  if (line !== deviceLine) {
    deviceLine = line;
    setStatus(line);
  }
  paintClock();
}

function deviceStatusLine(state: DeviceCast["state"], name: string, quiet: boolean): string {
  if (state === "next") return "Starting the next episode on " + name + "…";
  if (quiet) return "Waiting for " + name + " to answer…";
  if (state === "starting") return "Starting on " + name + "…";
  if (state === "buffering") return "Loading on " + name + "…";
  return "Casting to " + name + ".";
}

/** Play or Pause shows at once; the clock holds where the TV is while it takes a moment. */
function holdDevicePlay(paused: boolean): void {
  const at = devicePosition();
  if (deviceView && at != null) deviceView = { ...deviceView, position: at, state: paused ? "paused" : "playing" };
  deviceSeenAt = Date.now();
  deviceFreshAt = Date.now();
  devicePaused = paused;
  deviceWant = { paused, since: Date.now() };
  syncPlayButton();
  paintClock();
}

/**
 * Asks for a place in the title. The server knows where the TV's stream
 * starts, even right after a seek restarted it, and places the seek in it.
 */
async function seekDevice(id: string, at: number): Promise<void> {
  if (castBusy) return;
  castBusy = true;
  deviceSeeking = true;
  deviceSeekTarget = at;
  deviceQueued = null;
  paintCast();
  let moving = false;
  try {
    await api.castDeviceSeek(id, at);
    if (castMode !== "device" || castReceiverId !== id) return;
    if (deviceView) deviceView = { ...deviceView, position: at };
    deviceSeenAt = Date.now();
    deviceFreshAt = Date.now();
    // The TV reloads at the new place. A seek sent before it is there is
    // dropped, so presses meanwhile wait until it says it has landed.
    moving = true;
    deviceLandSince = Date.now();
    paintClock();
  } catch (err) {
    const text = castFail(err);
    setStatus(text);
  } finally {
    castBusy = false;
    if (moving) paintCast();
    else settleDeviceSeek(id);
  }
}

/** The TV is where it was sent, or gave up getting there: queued presses go now. */
function settleDeviceSeek(id: string): void {
  deviceLandSince = 0;
  deviceSeeking = false;
  paintCast();
  const queued = deviceQueued;
  deviceQueued = null;
  if (queued !== null && castMode === "device" && castReceiverId === id) void skipTo(queued);
}

/** A browser cast Portal ended because its TV stopped saying it was there. */
type LostCast = NonNullable<CastSession["lost"]>;

function lostCast(session: CastSession, receiverId: string | null): LostCast | null {
  const lost = session.lost;
  if (!lost || typeof lost !== "object" || !receiverId || textField(lost.receiver_id) !== receiverId) return null;
  return lost;
}

function followReceiver(name: string): void {
  leaveBrowserClock();
  const token = ++receiverWatch;
  const receiverId = castReceiverId;
  const mine = (): boolean => token === receiverWatch && castMode === "browser" && castReceiverId === receiverId;
  browserClock = window.setInterval(() => {
    if (mine() && !dragging) paintClock();
  }, 1000);
  void (async () => {
    while (mine()) {
      let session: CastSession;
      try {
        session = await api.castSession();
      } catch {
        await sleep(RECEIVER_POLL);
        continue;
      }
      if (!mine()) return;
      const nested = session.session && typeof session.session === "object" ? session.session : session;
      const liveReceiver = textField(nested.receiver_id) || textField(session.receiver_id);
      const jobId = textField(nested.job_id) || textField(session.job_id);
      if (!jobId || liveReceiver !== receiverId) {
        const lost = lostCast(session, receiverId);
        if (!lost) {
          castEnded(name + " stopped casting.");
          return;
        }
        // The TV went quiet. It stopped where it last said it was, not where
        // the clock would have run on to, and Play here resumes from there.
        const last = (textField(lost.job_id) === currentJob ? readPlace(lost) : null) ?? browserPlace;
        if (last) browserPlace = { ...last, paused: true };
        browserSeeking = false;
        castEnded("Lost " + name + ".");
        return;
      }
      if (jobId !== currentJob) {
        adoptCastJob(jobId, nested, nested.title);
        setStatus("Casting to " + name + ".");
      }
      const place = readPlace(nested) ?? readPlace(session);
      if (place && browserSeeking) {
        const close = Math.abs(place.position - browserSeekTarget) < 3;
        const waited = Date.now() - browserSeekSince > 8000;
        if (close || waited) browserSeeking = false;
      }
      // Presses made while the TV was still moving run from where it landed.
      if (!browserSeeking && browserQueued != null && !castBusy) {
        const queued = browserQueued;
        browserQueued = null;
        void skipTo(queued);
      }
      if (place && !browserSeeking && !dragging) {
        browserPlace = place;
        const age = Date.now() - place.updatedMs;
        browserSeenAt = age >= 0 && age < 15_000 ? place.updatedMs : Date.now();
        syncPlayButton();
        paintClock();
      }
      await sleep(RECEIVER_POLL);
    }
  })();
}

/** A dashboard opened while Portal casts to a device picks that cast up. */
async function resumeDeviceCast(): Promise<void> {
  const token = generation;
  let view: DeviceCast;
  try {
    view = await api.castDevice();
  } catch {
    return;
  }
  const jobId = textField(view.job_id);
  const deviceId = textField(view.device_id);
  if (token !== generation || currentJob || !view.active || !jobId || !deviceId) return;
  const name = textField(view.device_name) || "Cast device";
  currentTarget = null;
  showDock(textField(view.title) || textField(view.item?.title) || "Playback");
  castReceiverId = deviceId;
  castReceiverName = name;
  castMode = "device";
  devicePaused = view.state === "paused";
  adoptCastJob(jobId, view.item, view.title);
  followDevice(name);
  applyDeviceView(view, name);
  syncPlayButton();
  paintCast();
}

async function onCastClick(): Promise<void> {
  if (!currentJob) return;
  if (castMenuOpen()) {
    castMenuGen += 1;
    hideCastMenu(true);
    paintCast();
    return;
  }
  if (castBusy || castScanning) return;
  hideSubtitleMenu();
  const job = currentJob;
  const menuToken = ++castMenuGen;
  // Its own flag: Pause, ±10 and the volume still reach the TV while the search runs.
  castScanning = true;
  paintCast();
  const priorStatus = el("dock-status").textContent || "";
  setStatus("Looking for devices…");
  const clearLooking = (): void => {
    if (el("dock-status").textContent === "Looking for devices…") setStatus(priorStatus);
  };
  try {
    const [receiverResult, deviceResult, session] = await Promise.all([
      loadReceivers()
        .then((items) => ({ ok: true as const, items }))
        .catch((err: unknown) => ({ ok: false as const, err })),
      loadDevices()
        .then((items) => ({ ok: true as const, items }))
        .catch((err: unknown) => ({ ok: false as const, err })),
      loadCastSession().catch(() => ({})),
    ]);
    if (menuToken !== castMenuGen || currentJob !== job || subtitleMenuOpen()) {
      clearLooking();
      return;
    }
    const receivers = receiverResult.ok ? receiverResult.items : [];
    const devices = deviceResult.ok ? deviceResult.items : [];
    if (castMode !== "device") {
      castReceiverId = activeReceiver(session, job);
      castMode = castReceiverId ? "browser" : null;
    } else if (castReceiverId && !devices.some((device) => device.id === castReceiverId)) {
      /* The TV dropped off the last scan. Keep the local cast until Stop. */
    }
    const choices = castChoices(devices, receivers);
    const casting = !!castReceiverId;
    if (choices.length === 0) {
      if (!deviceResult.ok && receivers.length === 0) throw deviceResult.err;
      if (!receiverResult.ok && devices.length === 0 && !deviceResult.ok) throw receiverResult.err;
      if (casting) {
        clearLooking();
        showCastMenu([], true);
        return;
      }
      const hint = await castOpenHint();
      if (menuToken !== castMenuGen || currentJob !== job || subtitleMenuOpen()) {
        clearLooking();
        return;
      }
      setStatus(hint);
      return;
    }
    if (casting) {
      clearLooking();
      showCastMenu(choices, true);
      return;
    }
    if (choices.length === 1) {
      castScanning = false;
      await castChoice(choices[0]);
      return;
    }
    clearLooking();
    showCastMenu(choices, false);
  } catch (err) {
    if (menuToken !== castMenuGen || currentJob !== job) return;
    const text = castFail(err);
    setStatus(text);
  } finally {
    castScanning = false;
    paintCast();
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

function paintVolume(): void {
  const volume = el<HTMLInputElement>("dock-volume");
  // While a Cast device plays, the controls show and set the TV's volume.
  const tv = castMode === "device" && !!castReceiverId ? tvVolumeNow() : null;
  const level = tv ? (tv.level ?? prefs().volume) : prefs().volume;
  const pct = Math.round(level * 100);
  const muted = tv ? tv.muted || level <= 0 : prefs().volume <= 0 || video().muted;
  if (document.activeElement !== volume && volume.value !== String(level)) volume.value = String(level);
  const what = tv ? "TV volume" : "Volume";
  volume.setAttribute("aria-label", muted ? what + ", muted" : what + ", " + pct + "%");
  volume.setAttribute("aria-valuetext", muted ? "Muted" : pct + "%");
  volume.title = tv ? "TV volume" : "";
  volume.style.setProperty("--volume-fill", (muted ? 0 : pct) + "%");
  const mute = el<HTMLButtonElement>("dock-mute");
  const word = muted ? "Unmute" : "Mute";
  mute.classList.toggle("is-muted", muted);
  mute.setAttribute("aria-label", tv ? word + " the TV" : word);
  mute.title = (tv ? word + " the TV" : word) + " (m)";
}

function onKey(event: KeyboardEvent): void {
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  if (castMenuOpen()) {
    handleDockMenuKey(event, castMenu(), () => {
      castMenuGen += 1;
      hideCastMenu(true);
      paintCast();
    });
    return;
  }
  if (subtitleMenuOpen()) {
    handleDockMenuKey(event, subtitleMenu(), () => {
      hideSubtitleMenu(true);
      paintSubtitleState();
    });
    return;
  }
  if (!el("dock").classList.contains("open") || el("prompt").classList.contains("open")) return;
  if (typingTarget(event.target, event.key)) return;
  const key = event.key;
  // Arrows on an episode row move through the list. They must not seek the video.
  const side = document.getElementById("dock-theater-side");
  if (
    side &&
    !side.hidden &&
    event.target instanceof Node &&
    side.contains(event.target) &&
    (key === "ArrowLeft" || key === "ArrowRight" || key === "ArrowUp" || key === "ArrowDown")
  ) {
    if (moveTheaterFocus(event.target, key)) event.preventDefault();
    return;
  }
  wake();
  if (key === " " || key === "Spacebar" || key === "k" || key === "K") {
    event.preventDefault();
    togglePlay();
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
  if (key === "ArrowUp" || key === "ArrowDown") {
    event.preventDefault();
    nudgeVolume(key === "ArrowUp" ? 0.05 : -0.05);
    return;
  }
  if (key === "f" || key === "F") {
    event.preventDefault();
    toggleFullscreen();
    return;
  }
  if (key === "t" || key === "T") {
    event.preventDefault();
    toggleTheater();
    return;
  }
  if (key === "c" || key === "C") {
    event.preventDefault();
    toggleSubtitles();
    return;
  }
  if (key === "Escape") {
    if (document.fullscreenElement === el("dock-stage")) {
      event.preventDefault();
      void document.exitFullscreen().catch(() => undefined);
      return;
    }
    if (document.body.classList.contains("theater")) {
      const panel = el("dock-episodes");
      // Escape collapses the list only when focus is inside it. The next Escape leaves theater.
      if (panel.classList.contains("is-open") && event.target instanceof Node && panel.contains(event.target)) {
        event.preventDefault();
        setEpisodesOpen(false);
        el<HTMLButtonElement>("dock-episodes-tab").focus();
        return;
      }
      event.preventDefault();
      theaterWanted = false;
      applyTheater();
      return;
    }
  }
  if ((key === "n" || key === "N") && currentNext?.file) {
    event.preventDefault();
    play(targetFromNext(currentNext));
    return;
  }
  if (key === "ArrowLeft" || key === "ArrowRight") {
    event.preventDefault();
    skipBy(key === "ArrowLeft" ? -10 : 10);
  }
}

function audioKey(tracks: AudioTrack[]): string {
  return tracks.map((track) => track.index + "\n" + track.label + "\n" + (track.lang ?? "")).join("\n");
}

/** Shows the file's audio tracks. One track stays visible and cannot be changed. */
function paintAudio(job: PlayJob | null): void {
  const select = document.getElementById("dock-audio");
  if (!(select instanceof HTMLSelectElement)) return;
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
  const chosen = tracks.some((track) => track.index === job?.audio_index) ? job?.audio_index ?? null : tracks[0]?.index ?? null;
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
  select.title = tracks.length < 2 ? tracks[0].label : "Audio track";
  fillingAudio = false;
}

/**
 * Switches the playing file onto the picked track. The add-on is not asked
 * again. A tagged pick is remembered; a track with no language is not, so it
 * does not wipe the language saved earlier.
 */
function applyAudio(): void {
  audioPending = false;
  const select = document.getElementById("dock-audio");
  if (!(select instanceof HTMLSelectElement) || fillingAudio) return;
  const raw = select.value;
  if (!/^\d+$/.test(raw)) return;
  const index = Number(raw);
  if (index === currentAudioIndex) return;
  const chosen = currentAudio.find((track) => track.index === index);
  if (!chosen) {
    paintAudio(lastReadyJob);
    return;
  }
  if (chosen.lang) setPrefs({ audioLanguage: chosen.lang });
  const id = currentJob;
  const resume = sourcePosition();
  if (!id) return;
  const restore = (err?: unknown): void => {
    if (typeof err === "string") setStatus(err);
    else if (err) setStatus(message(err));
    paintAudio(lastReadyJob);
    if (castMode || !attached) return;
    hls?.startLoad();
    void video().play().catch(() => undefined);
  };
  if (castMode === "browser") {
    setStatus("Switching audio…");
    void api.setAudio(id, index, resume).then(
      (job) => {
        if (currentJob !== id) return;
        if (!job.playlist) {
          restore(job.error || "The audio track did not start.");
          return;
        }
        lastReadyJob = job;
        paintAudio(job);
        setStatus("");
      },
      (err) => {
        if (currentJob !== id) return;
        restore(err);
      },
    );
    return;
  }
  if (castMode === "device" && castReceiverId) {
    const device = { id: castReceiverId, name: castReceiverName || "the TV" };
    setStatus("Switching audio…");
    void api.setAudio(id, index, resume).then(
      (job) => {
        if (currentJob !== id) return;
        if (!job.playlist) {
          restore(job.error || "The audio track did not start.");
          return;
        }
        lastReadyJob = job;
        origin = originFrom(job);
        paintAudio(job);
        void castToDevice(device);
      },
      (err) => {
        if (currentJob !== id) return;
        restore(err);
      },
    );
    return;
  }
  if (!attached || !lastReadyJob || skipping) {
    paintAudio(lastReadyJob);
    return;
  }
  const token = generation;
  const mine = ++skipGeneration;
  skipping = true;
  skipTarget = resume;
  queuedSkip = null;
  video().pause();
  hls?.stopLoad();
  showPlay({ item: currentTarget?.title || "Playback", phase: "preparing" });
  el<HTMLInputElement>("dock-bar").disabled = true;
  syncPlayButton();
  void api
    .setAudio(id, index, resume)
    .then(
      (job) => {
        if (token !== generation || mine !== skipGeneration) return;
        skipping = false;
        if (job.playlist) attachPlayback(job);
        else restore(job.error || "The audio track did not start.");
      },
      (err) => {
        if (token !== generation || mine !== skipGeneration) return;
        skipping = false;
        restore(err);
      },
    )
    .finally(() => {
      if (mine !== skipGeneration) return;
      skipping = false;
      syncPlayButton();
      const queued = queuedSkip;
      queuedSkip = null;
      if (queued !== null && token === generation && attached) void skipTo(queued);
    });
}

/** The tallest picture a quality asks for. Auto is 1080p; Source has no limit. */
function qualityHeight(quality: Quality): number {
  if (quality === "source") return Infinity;
  if (quality === "auto") return 1080;
  const height = Number(quality);
  return Number.isFinite(height) ? height : 1080;
}

export function mount(options: { onBrowse: (path: string, picking?: PlayTarget) => void }): void {
  onBrowse = options.onBrowse;
  if (mounted) return;
  mounted = true;
  const quality = el<HTMLSelectElement>("dock-quality");
  const audio = el<HTMLSelectElement>("dock-audio");
  const auto = el<HTMLInputElement>("dock-auto");
  const volume = el<HTMLInputElement>("dock-volume");
  const bar = el<HTMLInputElement>("dock-bar");
  quality.value = prefs().quality;
  auto.checked = prefs().autoplay;
  volume.value = String(prefs().volume);
  video().volume = prefs().volume;
  // Choices made on another screen show here; quality applies from the next video.
  onPrefs((next) => {
    quality.value = next.quality;
    auto.checked = next.autoplay;
    if (currentJob) applySubtitles();
    else paintSubtitleState();
  });
  void syncPrefs();
  const refresh = (): void => {
    if (document.visibilityState === "visible") void syncPrefs();
  };
  document.addEventListener("visibilitychange", refresh);
  window.addEventListener("focus", refresh);
  // Arrow keys on the focused menu change it once per press; wait for the last
  // one so stepping from Auto to 480p restarts the stream once, not four times.
  let qualityTimer = 0;
  quality.addEventListener("change", () => {
    window.clearTimeout(qualityTimer);
    qualityTimer = window.setTimeout(applyQuality, QUALITY_SETTLE);
  });
  audio.addEventListener("change", () => {
    if (fillingAudio) return;
    audioPending = true;
    window.clearTimeout(audioTimer);
    audioTimer = window.setTimeout(applyAudio, QUALITY_SETTLE);
  });
  const applyQuality = (): void => {
    const value = quality.value as Quality;
    const chosen: Quality = QUALITIES.includes(value) ? value : "auto";
    const before = prefs().quality;
    setPrefs({ quality: chosen });
    if (!currentTarget) return;
    const target = currentTarget;
    const resume = sourcePosition();
    const replay = (): void => play({ ...target, resumeSeconds: resume > 60 ? resume : undefined });
    const id = currentJob;
    // The copy playing was picked for the old height. A taller one is a
    // different copy, which the add-on has to find again.
    const taller = qualityHeight(chosen) > qualityHeight(before);
    if (!id || !attached || !lastReadyJob || castMode || skipping || taller) {
      replay();
      return;
    }
    // The playing job keeps its stream; only the transcode restarts, here, in the new quality.
    const token = generation;
    const mine = ++skipGeneration;
    skipping = true;
    // ±10 pressed meanwhile adds up from where the new stream starts, as during a skip.
    skipTarget = resume;
    queuedSkip = null;
    video().pause();
    // The server replaces the stream's files; stop fetching the old ones meanwhile.
    hls?.stopLoad();
    showPlay({ item: target.title || "Playback", phase: "preparing" });
    el<HTMLInputElement>("dock-bar").disabled = true;
    syncPlayButton();
    void api
      .setQuality(id, chosen, resume)
      .then(
        (job) => {
          if (token !== generation || mine !== skipGeneration) return;
          skipping = false;
          if (job.playlist) attachPlayback(job);
          else replay();
        },
        () => {
          if (token !== generation || mine !== skipGeneration) return;
          skipping = false;
          replay();
        },
      )
      .finally(() => {
        if (mine !== skipGeneration) return;
        skipping = false;
        syncPlayButton();
        const queued = queuedSkip;
        queuedSkip = null;
        if (queued !== null && token === generation && attached) void skipTo(queued);
      });
  };
  auto.addEventListener("change", () => {
    setPrefs({ autoplay: auto.checked });
  });
  volume.addEventListener("input", () => {
    if (castMode === "device" && castReceiverId) {
      setTvVolume({ level: clampVolume(Number(volume.value)), muted: false });
      return;
    }
    setPrefs({ volume: clampVolume(Number(volume.value)) });
    video().volume = prefs().volume;
    video().muted = false;
    paintVolume();
  });
  el("dock-mute").addEventListener("click", () => toggleMute());
  el("dock-back").addEventListener("click", () => skipBy(-10));
  el("dock-forward").addEventListener("click", () => skipBy(10));
  el("dock-big-play").addEventListener("click", () => togglePlay());
  el("dock-stage-cancel").addEventListener("click", () => stageCancel?.());
  const pip = el<HTMLButtonElement>("dock-pip");
  pip.hidden = !document.pictureInPictureEnabled;
  pip.addEventListener("click", () => togglePip());
  el("dock-upnext-play").addEventListener("click", () => {
    if (currentNext?.file) play(targetFromNext(currentNext));
  });
  el("dock-upnext-dismiss").addEventListener("click", () => {
    upNextDismissed = currentNext?.file ?? "";
    el("dock-upnext").hidden = true;
  });
  const stage = el("dock-stage");
  dropFocusAfterClick(stage);
  dropFocusAfterClick(el("dock"));
  stage.addEventListener("pointermove", () => wake());
  stage.addEventListener("pointerdown", () => wake());
  stage.addEventListener("focusin", () => wake());
  bar.addEventListener("input", () => {
    dragging = true;
    el("dock-chrome").classList.add("is-scrubbing");
    const at = clampSkip(Number(bar.value), progressDuration());
    el("dock-elapsed").textContent = formatClock(at);
    paintSeekText(bar, seekValueText(at, Number(bar.max)));
    paintScrubFill(bar);
  });
  bar.addEventListener("change", () => {
    dragging = false;
    el("dock-chrome").classList.remove("is-scrubbing");
    void skipTo(Number(bar.value));
  });
  onScrubRelease(
    () => dragging,
    () => {
      dragging = false;
      el("dock-chrome").classList.remove("is-scrubbing");
      paintClock();
    },
  );
  const scrub = el("dock-scrub");
  scrub.addEventListener("pointermove", (event) => {
    hoverX = event.clientX;
    placeHover(event.clientX);
  });
  scrub.addEventListener("pointerleave", () => hideHover());
  scrub.addEventListener("pointercancel", () => hideHover());
  el("dock-play").addEventListener("click", () => togglePlay());
  el("dock-full").addEventListener("click", () => toggleFullscreen());
  el("dock-theater").addEventListener("click", () => toggleTheater());
  el("dock-episodes-tab").addEventListener("click", () => setEpisodesOpen(!episodesOpen));
  onVideoClick(video(), () => togglePlay(), () => toggleFullscreen());
  video().addEventListener("play", () => syncPlayButton());
  video().addEventListener("playing", () => {
    started = true;
    buffering = false;
    syncPlayButton();
  });
  video().addEventListener("waiting", () => {
    buffering = true;
    paintStage();
    checkStall();
  });
  video().addEventListener("canplay", () => {
    buffering = false;
    paintStage();
  });
  video().addEventListener("progress", () => paintClock());
  video().addEventListener("volumechange", () => paintVolume());
  video().addEventListener("pause", () => {
    syncPlayButton();
    void postProgress(true).catch(() => undefined);
  });
  video().addEventListener("timeupdate", () => paintClock());
  el("dock-subs").addEventListener("click", () => toggleSubtitles());
  el("dock-subs-more").addEventListener("click", () => onSubtitleMenuClick());
  el("dock-links").addEventListener("click", () => playWithLinks());
  el("dock-cast").addEventListener("click", () => void onCastClick());
  document.addEventListener("pointerdown", (event) => {
    const target = event.target;
    if (!(target instanceof Node)) return;
    if (castMenuOpen()) {
      const castWrap = document.getElementById("dock-cast-wrap");
      if (!(castWrap && castWrap.contains(target))) {
        castMenuGen += 1;
        hideCastMenu();
        paintCast();
      }
    }
    if (subtitleMenuOpen()) {
      const subsWrap = document.getElementById("dock-subs-wrap");
      if (!(subsWrap && subsWrap.contains(target))) {
        hideSubtitleMenu();
        paintSubtitleState();
      }
    }
  });
  el("dock-next").addEventListener("click", () => {
    if (!currentNext?.file) return;
    play(targetFromNext(currentNext));
  });
  wireSavedButton(el<HTMLButtonElement>("dock-watchlist"), "watchlist");
  wireSavedButton(el<HTMLButtonElement>("dock-bookmark"), "bookmarks");
  el("dock-info").addEventListener("click", () => openTargetInfo());
  // One button, so focus stays on it: Expand after minimizing, back in the dock after expanding.
  el("dock-mini").addEventListener("click", () => setMini(!isMini()));
  el("dock-close").addEventListener("click", () => {
    if (infoOpen()) closeInfo(false);
    generation += 1;
    const released = releaseCurrent(true);
    const back = dockOpener;
    dockOpener = null;
    const hadFocus = el("dock").contains(document.activeElement);
    hideDock();
    // Keyboard users land back on what they played, not at the top of the page.
    if (hadFocus && back?.isConnected) back.focus();
    void released;
  });
  video().addEventListener("ended", () => {
    syncPlayButton();
    const token = generation;
    const known = currentNext?.file ? currentNext : null;
    const jobId = currentJob;
    // While a TV plays it, the TV (or Portal, for a Cast device) moves on and the dock follows.
    const casting = !!castMode;
    void (async () => {
      await postProgress(true).catch(() => undefined);
      if (token !== generation || casting) return;
      // A next season found late, after the near-the-end look.
      const next = known ?? (jobId ? await jobNext(jobId) : null);
      if (token !== generation || !next) return;
      await syncPrefs();
      if (token !== generation) return;
      if (prefs().autoplay) play(targetFromNext(next));
    })();
  });
  document.addEventListener("fullscreenchange", () => syncFullscreen());
  const dock = document.getElementById("dock");
  if (dock && typeof ResizeObserver !== "undefined") {
    new ResizeObserver(() => fitPlayingPadding()).observe(dock);
  }
  const head = document.querySelector(".top");
  if (head && typeof ResizeObserver !== "undefined") {
    new ResizeObserver(() => placeTheaterHead()).observe(head);
  }
  window.addEventListener("resize", () => fitPlayingPadding());
  document.addEventListener("keydown", onKey);
  // Save the place when the tab is hidden or closed; the 8s ticker may not run again.
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) void postProgress(true).catch(() => undefined);
  });
  // A killed tab can cancel the hidden-page post. keepalive lets this one finish.
  window.addEventListener("pagehide", (event) => {
    void postProgress(true, true).catch(() => undefined);
    // Nobody watches this stream once the page is closed, unless a TV plays it.
    // Left running, a transcode would go on for an hour or more.
    if (!event.persisted && currentJob && !castMode) void api.stop(currentJob, true).catch(() => undefined);
  });
  paintSubtitleState();
  paintLinks();
  paintCast();
  paintSaveButtons();
  paintVolume();
  syncPlayButton();
  syncFullscreen();
  void resumeDeviceCast();
}
