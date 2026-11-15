/**
 * Player choices shared by the dock player and the TV cast page. Portal
 * keeps quality, subtitles on or off (and the language last picked), the
 * audio language last picked, and Autoplay next (`/api/prefs`), so every
 * browser and TV starts the same way.
 * Volume stays in each browser. localStorage keeps a copy, so a page that
 * cannot reach Portal still starts with the last choices it saw.
 */
import { api, type Quality, type SharedPrefs } from "./api";

export type Prefs = {
  quality: Quality;
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

type Shared = Omit<SharedPrefs, "stored">;
type Listener = (prefs: Prefs) => void;

const KEY = "portal-playback";
export const QUALITIES: Quality[] = ["auto", "source", "2160", "1080", "720", "480"];
const PUT_DELAY = 300;
const LANGUAGE = /^[a-z]{2,3}(-[a-z0-9]{2,8})?$/i;

let current: Prefs = readLocal();
const listeners = new Set<Listener>();
/** Bumped by every local change, so an older answer from Portal cannot undo it. */
let version = 0;
let pending: Partial<Shared> = {};
let putTimer = 0;
let first: Promise<void> | null = null;

export function isQuality(value: unknown): value is Quality {
  return typeof value === "string" && (QUALITIES as string[]).includes(value);
}

export function clampVolume(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) return 1;
  if (value < 0) return 0;
  if (value > 1) return 1;
  return value;
}

function cleanLanguage(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const text = value.trim().replace(/_/g, "-");
  return LANGUAGE.test(text) && text.toLowerCase() !== "und" ? text : null;
}

function defaults(): Prefs {
  return {
    quality: "auto",
    subtitles: "off",
    subtitlesSet: false,
    subtitleLanguage: null,
    audioLanguage: null,
    autoplay: true,
    volume: 1,
  };
}

/**
 * The copy in this browser. Subtitles start off: older versions saved "auto"
 * as a default with every other setting, so only an explicit choice (or an
 * embedded track id an older version saved for a pick) turns them on.
 */
function readLocal(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (!raw) return defaults();
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const saved = typeof parsed.subtitles === "string" ? parsed.subtitles : "off";
    const set = parsed.subtitlesSet === true;
    const on = saved !== "off" && (saved !== "auto" || set);
    return {
      quality: isQuality(parsed.quality) ? parsed.quality : "auto",
      subtitles: on ? "auto" : "off",
      subtitlesSet: set || (saved !== "off" && saved !== "auto"),
      subtitleLanguage: cleanLanguage(parsed.subtitleLanguage),
      audioLanguage: cleanLanguage(parsed.audioLanguage),
      autoplay: parsed.autoplay !== false,
      volume: clampVolume(parsed.volume),
    };
  } catch {
    return defaults();
  }
}

function writeLocal(): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    /* Private mode can reject localStorage. The choice still applies for this view. */
  }
}

/** Portal's answer, or null when it is not one (an older Portal, or offline). */
function fromServer(raw: unknown): (Shared & { stored: boolean }) | null {
  const row = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : null;
  if (!row || !isQuality(row.quality) || typeof row.autoplay !== "boolean") return null;
  if (row.subtitles !== "off" && row.subtitles !== "auto") return null;
  return {
    quality: row.quality,
    subtitles: row.subtitles,
    subtitle_language: cleanLanguage(row.subtitle_language),
    audio_language: cleanLanguage(row.audio_language),
    autoplay: row.autoplay,
    stored: row.stored === true,
  };
}

function notify(): void {
  for (const listener of listeners) {
    try {
      listener(current);
    } catch {
      /* One view's repaint must not stop the others. */
    }
  }
}

/** Takes Portal's choices; answers whether anything changed. */
function adopt(shared: Shared): boolean {
  const next: Prefs = {
    ...current,
    quality: shared.quality,
    subtitles: shared.subtitles,
    subtitlesSet: true,
    subtitleLanguage: shared.subtitle_language,
    audioLanguage: shared.audio_language,
    autoplay: shared.autoplay,
  };
  const changed =
    next.quality !== current.quality ||
    next.subtitles !== current.subtitles ||
    next.subtitleLanguage !== current.subtitleLanguage ||
    next.audioLanguage !== current.audioLanguage ||
    next.autoplay !== current.autoplay;
  current = next;
  writeLocal();
  if (changed) notify();
  return changed;
}

/** This browser's own choices, for a Portal that has none saved yet. */
function migration(local: Prefs): Partial<Shared> {
  const out: Partial<Shared> = {};
  if (local.quality !== "auto") out.quality = local.quality;
  if (!local.autoplay) out.autoplay = false;
  if (local.subtitlesSet) {
    out.subtitles = local.subtitles;
    if (local.subtitleLanguage) out.subtitle_language = local.subtitleLanguage;
  }
  if (local.audioLanguage) out.audio_language = local.audioLanguage;
  return out;
}

async function fetchShared(): Promise<void> {
  const asked = version;
  let answer: ReturnType<typeof fromServer>;
  try {
    answer = fromServer(await api.prefs());
  } catch {
    return;
  }
  if (!answer || asked !== version || Object.keys(pending).length) return;
  if (!answer.stored) {
    const moved = migration(current);
    if (Object.keys(moved).length) {
      try {
        const saved = fromServer(await api.setPrefs(moved));
        if (saved && asked === version) adopt(saved);
      } catch {
        /* Stay with this browser's choices; the next change saves them. */
      }
      return;
    }
  }
  adopt(answer);
}

function timeout(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}

/** The choices now. Views read this at every play start. */
export function prefs(): Prefs {
  return current;
}

/**
 * Reads Portal's choices (the first call also moves this browser's over if
 * Portal has none). Resolves after at most `wait` ms, so a slow or missing
 * Portal never holds up playback; a late answer still applies when it comes.
 */
export function syncPrefs(wait = 1500): Promise<void> {
  const run = first ? fetchShared() : (first = fetchShared());
  return Promise.race([run, timeout(wait)]);
}

async function flush(keepalive = false): Promise<void> {
  if (putTimer) window.clearTimeout(putTimer);
  putTimer = 0;
  const body = pending;
  pending = {};
  if (!Object.keys(body).length) return;
  const sent = version;
  try {
    const saved = fromServer(await api.setPrefs(body, keepalive));
    if (saved && sent === version && !Object.keys(pending).length) adopt(saved);
  } catch {
    /* Kept in this browser; the next change or refresh tries again. */
  }
}

/** Changes choices here at once and saves the shared ones to Portal shortly after. */
export function setPrefs(patch: Partial<Prefs>): void {
  version += 1;
  current = { ...current, ...patch };
  if (patch.subtitleLanguage !== undefined) current.subtitleLanguage = cleanLanguage(patch.subtitleLanguage);
  if (patch.audioLanguage !== undefined) current.audioLanguage = cleanLanguage(patch.audioLanguage);
  writeLocal();
  const shared: Partial<Shared> = {};
  if (patch.quality !== undefined) shared.quality = current.quality;
  if (patch.subtitles !== undefined) shared.subtitles = current.subtitles;
  if (patch.subtitleLanguage !== undefined) shared.subtitle_language = current.subtitleLanguage;
  if (patch.audioLanguage !== undefined) shared.audio_language = current.audioLanguage;
  if (patch.autoplay !== undefined) shared.autoplay = current.autoplay;
  notify();
  if (!Object.keys(shared).length) return;
  pending = { ...pending, ...shared };
  if (putTimer) window.clearTimeout(putTimer);
  putTimer = window.setTimeout(() => void flush(), PUT_DELAY);
}

/**
 * Reads this browser's copy again, e.g. when the TV page opens in a tab that
 * was already loaded. Portal's choices replace it once `syncPrefs` answers.
 */
export function reloadLocal(): void {
  if (Object.keys(pending).length) return;
  const volume = current.volume;
  current = readLocal();
  if (!Number.isFinite(current.volume)) current.volume = volume;
  notify();
}

/** Called when choices change here or arrive from Portal. Returns an unsubscribe. */
export function onPrefs(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

if (typeof window !== "undefined") {
  // A choice made just before the tab closes still reaches Portal.
  window.addEventListener("pagehide", () => void flush(true));
}
