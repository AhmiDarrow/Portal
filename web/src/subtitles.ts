/**
 * Online subtitles shared by the dock player and the TV cast page: what to
 * search for, the user's languages, labels for results, loading a result as
 * a track, and moving cues for the timing adjustment.
 */
import {
  api,
  type MediaInfo,
  type SubtitleCandidate,
  type SubtitleSearch,
  type SubtitleSettings,
  type SubtitleTrack,
} from "./api";

/** Languages offered on the Server page, by code as the service knows them. */
export const COMMON_LANGUAGES: ReadonlyArray<readonly [string, string]> = [
  ["en", "English"],
  ["es", "Spanish"],
  ["fr", "French"],
  ["de", "German"],
  ["it", "Italian"],
  ["pt-BR", "Portuguese (Brazil)"],
  ["pt-PT", "Portuguese"],
  ["nl", "Dutch"],
  ["sv", "Swedish"],
  ["da", "Danish"],
  ["no", "Norwegian"],
  ["fi", "Finnish"],
  ["pl", "Polish"],
  ["cs", "Czech"],
  ["hu", "Hungarian"],
  ["ro", "Romanian"],
  ["el", "Greek"],
  ["tr", "Turkish"],
  ["ru", "Russian"],
  ["uk", "Ukrainian"],
  ["hr", "Croatian"],
  ["sr", "Serbian"],
  ["bg", "Bulgarian"],
  ["ar", "Arabic"],
  ["he", "Hebrew"],
  ["fa", "Persian"],
  ["hi", "Hindi"],
  ["th", "Thai"],
  ["vi", "Vietnamese"],
  ["id", "Indonesian"],
  ["ms", "Malay"],
  ["zh-CN", "Chinese (Simplified)"],
  ["zh-TW", "Chinese (Traditional)"],
  ["ja", "Japanese"],
  ["ko", "Korean"],
];

/** ISO 639-2 tags that video files use for their tracks. */
const THREE_LETTER: Record<string, string> = {
  eng: "en", spa: "es", fre: "fr", fra: "fr", ger: "de", deu: "de", ita: "it", por: "pt", dut: "nl",
  nld: "nl", swe: "sv", dan: "da", nor: "no", nob: "no", nno: "no", fin: "fi", pol: "pl", cze: "cs",
  ces: "cs", hun: "hu", rum: "ro", ron: "ro", gre: "el", ell: "el", tur: "tr", rus: "ru", ukr: "uk",
  hrv: "hr", srp: "sr", bul: "bg", ara: "ar", heb: "he", per: "fa", fas: "fa", hin: "hi", tha: "th",
  vie: "vi", ind: "id", may: "ms", msa: "ms", chi: "zh", zho: "zh", jpn: "ja", kor: "ko", slv: "sl",
  slo: "sk", slk: "sk", est: "et", lav: "lv", lit: "lt", ice: "is", isl: "is", cat: "ca",
};

/** "en" from "en-US", "eng" or "EN". Empty when there is no tag. */
export function primaryLanguage(tag: string | null | undefined): string {
  const text = (tag ?? "").trim().toLowerCase().replace(/_/g, "-");
  if (!text) return "";
  const primary = text.split("-")[0] ?? "";
  return THREE_LETTER[primary] ?? primary;
}

/**
 * Portuguese (Brazil) and Portuguese (Portugal) are different choices, and so
 * are Simplified and Traditional Chinese. A tag with no region still matches
 * either. Other regions, such as en-US and en-GB, are the same language.
 */
export function sameLanguage(a: string | null | undefined, b: string | null | undefined): boolean {
  const left = languageIdentity(a);
  const right = languageIdentity(b);
  if (!left.primary || left.primary !== right.primary) return false;
  if (left.primary !== "pt" && left.primary !== "zh") return true;
  if (!left.region || !right.region) return true;
  return left.region === right.region;
}

/** Primary code, plus pt-BR, pt-PT, zh-CN, or zh-TW when the tag names one. */
function languageIdentity(tag: string | null | undefined): { primary: string; region: string } {
  const text = (tag ?? "").trim().toLowerCase().replace(/_/g, "-");
  const primary = primaryLanguage(text);
  if (!primary || (primary !== "pt" && primary !== "zh")) return { primary, region: "" };
  if (primary === "pt") {
    if (text.includes("br")) return { primary, region: "pt-BR" };
    if (text.split("-").length > 1) return { primary, region: "pt-PT" };
    return { primary, region: "" };
  }
  if (/tw|hk|mo|hant/.test(text)) return { primary, region: "zh-TW" };
  if (/cn|sg|hans/.test(text)) return { primary, region: "zh-CN" };
  return { primary, region: "" };
}

/** The browser's languages as codes the service knows ("en", "pt-BR"), best first. */
export function browserLanguages(): string[] {
  const list = typeof navigator !== "undefined" ? [...(navigator.languages ?? []), navigator.language] : [];
  const out: string[] = [];
  for (const raw of list) {
    if (!raw) continue;
    const lower = raw.toLowerCase();
    let code = primaryLanguage(raw);
    if (code === "pt") code = lower.includes("br") ? "pt-BR" : "pt-PT";
    if (code === "zh") code = /tw|hk|mo|hant/.test(lower) ? "zh-TW" : "zh-CN";
    if (code && !out.includes(code)) out.push(code);
    if (out.length === 4) break;
  }
  return out.length ? out : ["en"];
}

let settingsAt = 0;
let settingsValue: SubtitleSettings | null = null;

/** Settings as the page uses them, whatever an older or odd service sent. */
export function cleanSubtitleSettings(raw: unknown): SubtitleSettings {
  const row = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const languages = Array.isArray(row.languages)
    ? row.languages.filter((code): code is string => typeof code === "string" && code.trim() !== "")
    : [];
  return {
    languages,
    hearing_impaired: row.hearing_impaired === true,
    opensubtitles: row.opensubtitles === true,
    opensubtitles_user: row.opensubtitles_user === true,
    subdl: row.subdl === true,
    subsource: row.subsource === true,
  };
}

/** The saved subtitle settings, kept for a minute. Null when they cannot be read. */
export async function subtitleSettings(): Promise<SubtitleSettings | null> {
  if (settingsValue && Date.now() - settingsAt < 60_000) return settingsValue;
  try {
    settingsValue = cleanSubtitleSettings(await api.subtitleSettings());
    settingsAt = Date.now();
  } catch {
    settingsValue = null;
  }
  return settingsValue;
}

/** Called after the Server page saves, so the next search uses the new languages. */
export function rememberSubtitleSettings(settings: SubtitleSettings | null): void {
  settingsValue = settings;
  settingsAt = settings ? Date.now() : 0;
}

/** The languages to prefer: saved ones, else the browser's. */
export function preferredLanguages(settings: SubtitleSettings | null): string[] {
  return settings?.languages?.length ? settings.languages : browserLanguages();
}

/**
 * The audio language to ask for when a file starts. A saved pick wins. Before
 * one exists, the subtitle languages (or the browser's) are used, so English
 * is not left off just because another track is the file's default.
 */
export function startupAudioLanguage(saved: string | null | undefined): string | null {
  const text = (saved ?? "").trim();
  if (text) return text;
  const preferred = (preferredLanguages(settingsValue)[0] ?? "").trim();
  return preferred || null;
}

function clean(text: string | null | undefined): string {
  return (text ?? "").trim();
}

/**
 * The search for a title, or null when there is nothing to search by (a
 * song, a file without a title). Languages go along only when none are saved.
 */
export function searchFor(
  seed: MediaInfo,
  release: string | null | undefined,
  settings: SubtitleSettings | null,
): SubtitleSearch | null {
  const episode = seed.kind === "episode" || (seed.season != null && !!seed.episode && seed.kind !== "movie");
  const title = clean(seed.title);
  const show = clean(seed.show_title);
  if (episode && !(show && seed.season != null && seed.episode)) return null;
  if (!episode && seed.kind !== "movie") return null;
  if (!episode && !title) return null;
  const body: SubtitleSearch = { kind: episode ? "episode" : "movie" };
  if (title) body.title = title;
  if (show) body.show_title = show;
  if (seed.year) body.year = seed.year;
  if (episode) {
    body.season = seed.season;
    body.episode = seed.episode;
  }
  if (seed.imdb_id) body.imdb_id = seed.imdb_id;
  if (seed.tmdb_id) body.tmdb_id = seed.tmdb_id;
  if (seed.tvdb_id) body.tvdb_id = seed.tvdb_id;
  if (clean(release)) body.release = clean(release);
  if (!settings?.languages?.length) body.languages = browserLanguages();
  return body;
}

/** Results the menus can show: rows with an id, source and language. */
export function cleanResults(raw: unknown): { items: SubtitleCandidate[]; best: string | null; notes: string[] } {
  const row = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {};
  const text = (value: unknown): value is string => typeof value === "string" && value.trim() !== "";
  const items = (Array.isArray(row.items) ? row.items : []).filter((item): item is SubtitleCandidate => {
    const candidate = item as Partial<SubtitleCandidate> | null;
    return !!candidate && text(candidate.id) && text(candidate.source) && text(candidate.language);
  });
  const named = items.map((item) => ({ ...item, language_name: text(item.language_name) ? item.language_name : item.language }));
  const best = text(row.best) && named.some((item) => item.id === row.best) ? row.best : (named[0]?.id ?? null);
  const notes = Array.isArray(row.notes) ? row.notes.filter(text) : [];
  return { items: named, best, notes };
}

/** "English · 720p.BluRay.MACRO". */
export function candidateTitle(item: SubtitleCandidate): string {
  const release = clean(item.release);
  return release ? item.language_name + " · " + release : item.language_name;
}

/** "Gestdown · HI · Best match". */
export function candidateDetail(item: SubtitleCandidate, best: boolean): string {
  const parts = [item.source];
  if (item.hearing_impaired) parts.push("HI");
  if (item.match === "hash") parts.push("Exact match");
  else if (item.match === "release") parts.push("Matches this release");
  if (best) parts.push("Best match");
  return parts.join(" · ");
}

/** Server label, else its language tag; a bare code like "rum" reads as "Romanian". */
export function trackName(track: SubtitleTrack): string {
  const name = baseTrackName(track);
  return track.forced && !/forced/i.test(name) ? name + " (forced)" : name;
}

function baseTrackName(track: SubtitleTrack): string {
  const label = track.label.trim();
  const lang = (track.lang ?? "").trim();
  if (label && label.toLowerCase() !== lang.toLowerCase()) return label;
  const named = languageName(label || lang);
  if (named) return named;
  return label || lang || "Track " + track.id;
}

/** The browser's name for a 2- or 3-letter language code, or "" when it has none. */
export function languageName(code: string): string {
  if (!/^[a-z]{2,3}(-[a-z0-9]{2,8})?$/i.test(code)) return "";
  try {
    const text = code.trim().replace(/_/g, "-");
    const primary = primaryLanguage(text);
    const region = text.split("-")[1];
    const query = region && primary.length === 2 ? primary + "-" + region : primary;
    const name = new Intl.DisplayNames(undefined, { type: "language" }).of(query);
    return name && name.toLowerCase() !== code.toLowerCase() ? name : "";
  } catch {
    return "";
  }
}

/** An embedded track in the first preferred language that has one. A forced track only covers foreign parts. */
export function preferredTrack(tracks: SubtitleTrack[], languages: string[]): SubtitleTrack | null {
  for (const language of languages) {
    const found = tracks.find((track) => !track.forced && sameLanguage(track.lang, language));
    if (found) return found;
  }
  return null;
}

/**
 * What Auto shows, as "e:<id>" or "o:<id>": language by language in the
 * user's order, the video's own full track and then an online result; after
 * that the best online result, then any full track, then any track.
 */
export function autoSubtitle(
  tracks: SubtitleTrack[],
  online: { id: string; language?: string | null }[],
  languages: string[],
): string | null {
  for (const language of languages) {
    const track = preferredTrack(tracks, [language]);
    if (track) return "e:" + track.id;
    const item = online.find((candidate) => sameLanguage(candidate.language, language));
    if (item) return "o:" + item.id;
  }
  if (online[0]) return "o:" + online[0].id;
  const full = tracks.find((track) => !track.forced) ?? tracks[0];
  return full ? "e:" + full.id : null;
}

/** An online subtitle as a blob URL a `<track>` can load. Throws with the service's detail. */
export async function onlineTrackUrl(id: string, offset: number): Promise<string> {
  const text = await api.subtitleFile(id, offset);
  return URL.createObjectURL(new Blob([text], { type: "text/vtt" }));
}

/** Each cue's time as loaded, before any delay. */
const loadedTimes = new WeakMap<TextTrackCue, [number, number]>();

/** Moves every cue `delay` seconds later (earlier when negative) than it loaded. */
export function shiftCues(track: TextTrack | null | undefined, delay: number): void {
  if (!track) return;
  for (const cue of Array.from(track.cues ?? [])) {
    let times = loadedTimes.get(cue);
    if (!times) {
      times = [cue.startTime, cue.endTime];
      loadedTimes.set(cue, times);
    }
    const start = Math.max(0, times[0] + delay);
    const end = Math.max(start, times[1] + delay);
    // The cue rejects a start that passes its current end, and an end that
    // precedes its current start. Move the far edge first.
    if (delay >= 0) {
      cue.endTime = end;
      cue.startTime = start;
    } else {
      cue.startTime = start;
      cue.endTime = end;
    }
  }
}

export const DELAY_STEP = 0.5;
const DELAY_LIMIT = 600;

export function clampDelay(value: number): number {
  if (!Number.isFinite(value)) return 0;
  const rounded = Math.round(value * 10) / 10;
  return Math.max(-DELAY_LIMIT, Math.min(DELAY_LIMIT, rounded));
}

/** "Delay +1.0 s", "Delay −0.5 s", "Delay 0.0 s". */
export function delayLabel(delay: number): string {
  const value = clampDelay(delay);
  const sign = value > 0 ? "+" : value < 0 ? "−" : "";
  return "Delay " + sign + Math.abs(value).toFixed(1) + " s";
}
