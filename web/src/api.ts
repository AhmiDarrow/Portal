export type EngineState = "running" | "starting" | "stopped";
export type ItemKind = "open" | "play" | "run";
export type TitleKind = "movie" | "tv";
export type PlayKind = "movie" | "episode" | "song" | "file";
export type Quality = "auto" | "source" | "2160" | "1080" | "720" | "480";

export type Status = {
  state: EngineState;
  pid: number | null;
  version: string | null;
  started_by_us: boolean;
  window_visible: boolean;
  webserver: boolean;
  error: string | null;
};

export type Source = {
  addon_id: string | null;
  name: string | null;
  path: string | null;
};

export type AddonCard = {
  id: string;
  name: string;
  path: string;
  art?: string | null;
  /** A copy under the Kodi profile. One that ships with Kodi is left alone. */
  removable?: boolean;
};

export type BrowseItem = {
  id: string;
  label: string;
  path: string;
  kind: ItemKind;
  art?: string | null;
  title_kind?: TitleKind | null;
  season?: number | null;
  episode?: number | null;
  show_title?: string | null;
  show_path?: string | null;
  settings_addon?: string | null;
  /** What the add-on (and, later, online lookups) know about this title. */
  info?: MediaInfo | null;
};

export type MediaKind = "movie" | "tv" | "episode";

export type CastMember = { name: string; role?: string | null; thumb?: string | null };

export type MediaLink = { label: string; url: string };

export type MediaInfo = {
  kind?: MediaKind | null;
  title?: string | null;
  original_title?: string | null;
  show_title?: string | null;
  season?: number | null;
  episode?: number | null;
  year?: number | null;
  premiered?: string | null;
  plot?: string | null;
  tagline?: string | null;
  genres?: string[] | null;
  runtime_minutes?: number | null;
  mpaa?: string | null;
  /** 0 to 10. */
  rating?: number | null;
  votes?: number | null;
  studios?: string[] | null;
  directors?: string[] | null;
  writers?: string[] | null;
  countries?: string[] | null;
  cast?: CastMember[] | null;
  imdb_id?: string | null;
  tmdb_id?: string | null;
  tvdb_id?: string | null;
  trailer?: string | null;
  poster?: string | null;
  fanart?: string | null;
  /** Where the info came from, e.g. "Kodi add-on", "TVmaze". */
  sources?: string[] | null;
  links?: MediaLink[] | null;
};

export type SavedState = { watchlist: boolean; bookmarks: boolean };

export type MetadataSettings = { online: boolean; tmdb: boolean };

/** Body of PUT /api/watchlist/{id} and /api/bookmarks/{id}. */
export type ListPut = {
  kind: TitleKind;
  name: string;
  art?: string | null;
  year?: number;
  imdb_id?: string;
};

export type BrowsePage = {
  path: string;
  items: BrowseItem[];
};

export type Card = {
  id: string;
  kind: TitleKind;
  name: string;
  art?: string | null;
  fraction?: number | null;
  resume_seconds?: number | null;
  season?: number | null;
  episode?: number | null;
  episode_name?: string | null;
  file?: string | null;
  year?: number | null;
  imdb_id?: string | null;
};

export type Home = {
  continue_watching: Card[];
  watchlist: Card[];
  bookmarks: Card[];
};

export type Episode = {
  season: number;
  episode: number;
  name: string;
  path: string;
  art?: string | null;
  position: number;
  duration: number;
  finished: boolean;
  resumable: boolean;
};

export type Season = {
  number: number;
  episodes: Episode[];
};

export type EpisodeRef = {
  season: number;
  episode: number;
  name: string;
  path: string;
  art?: string | null;
};

/** An add-on folder that lists one season of a show. */
export type SeasonFolder = {
  season: number;
  path: string;
};

export type TitleDetail = {
  id: string;
  kind: TitleKind;
  name: string;
  art?: string | null;
  bookmarked: boolean;
  watchlisted: boolean;
  resume_seconds?: number | null;
  watched: boolean;
  seasons: Season[];
  /** The show's season folders, lowest season first; missing until its season list was seen. */
  season_folders?: SeasonFolder[];
  next?: EpisodeRef | null;
  file?: string | null;
  year?: number | null;
  imdb_id?: string | null;
};

export type HistoryEntry = {
  id: string;
  kind: TitleKind;
  name: string;
  art?: string | null;
  at: number;
  position: number;
  duration: number;
  finished: boolean;
  season?: number | null;
  episode?: number | null;
  file?: string | null;
  slot: number;
};

/** The title a history row showed. Rows shift while something plays, so Portal checks the slot still holds it. */
export type HistoryRef = Pick<HistoryEntry, "id" | "season" | "episode">;

function historyRef(row?: HistoryRef): Record<string, unknown> {
  if (!row) return {};
  return { id: row.id, season: row.season ?? undefined, episode: row.episode ?? undefined };
}

function historyQuery(row?: HistoryRef): string {
  if (!row) return "";
  const query = new URLSearchParams({ id: row.id });
  if (row.season != null) query.set("season", String(row.season));
  if (row.episode != null) query.set("episode", String(row.episode));
  return "?" + query.toString();
}

export type FieldKind ="bool" | "text" | "number" | "select" | "action" | "header" | "secret";

export type SettingsField = {
  id?: string | null;
  kind: FieldKind;
  label: string;
  value: string;
  options?: { value: string; label: string }[];
  secret: boolean;
  enabled: boolean;
  action?: string;
};

export type SettingsDoc = {
  addon_id: string;
  categories: { label: string; fields: SettingsField[] }[];
};

export type PairMe = { paired: boolean; this_pc: boolean };
export type PairStart = { code: string; ticket: string; expires_in: number };
export type PairedDevice = {
  id: string;
  name: string;
  paired_at: number;
  last_seen: number;
  address: string;
};

export type Prompt = {
  id: string;
  kind: "text" | "select" | "yesno" | "ok" | "progress";
  title: string;
  message: string;
  options?: string[];
  cancellable: boolean;
  progress?: number | null;
};

export type SubtitleTrack = {
  id: number;
  label: string;
  lang?: string | null;
  url: string;
  /** Only the foreign-language parts; never picked as the full subtitle. */
  forced?: boolean;
};

export type NextItem = {
  kind: PlayKind;
  id: string;
  file: string;
  title: string;
  season?: number | null;
  episode?: number | null;
  show_id?: string | null;
  show_title?: string | null;
  art?: string | null;
};

export type RepoCard = {
  id: string;
  name: string;
  /** A copy under the Kodi profile. One that ships with Kodi is left alone. */
  removable?: boolean;
};

export type RepoAddon = {
  id: string;
  name: string;
  version: string;
  summary: string;
  installed: boolean;
  /** Installed under the Kodi profile, so it can be removed. */
  removable?: boolean;
  /** `video`, or `module` for a Python module other add-ons use. */
  kind?: "video" | "module";
};

export type RemoveResult = {
  id: string;
  name: string;
  kind: string;
  applies?: string | null;
};

export type RepoDetail = {
  id: string;
  name: string;
  items: RepoAddon[];
};

export type InstallResult = {
  id: string;
  name: string;
  kind: string;
  wrote: boolean;
  applies?: "now" | "next-start" | null;
};

/** One audio stream in the playing file. `index` is the ffprobe stream index. */
export type AudioTrack = {
  index: number;
  label: string;
  lang?: string | null;
};

/** Tracks the player can offer. A stream without a usable index is left out. */
export function audioTracksOf(job: { audio?: AudioTrack[] | null } | null | undefined): AudioTrack[] {
  const out: AudioTrack[] = [];
  for (const track of job?.audio ?? []) {
    if (typeof track?.index !== "number" || !Number.isInteger(track.index) || track.index < 0) continue;
    const label = typeof track.label === "string" && track.label.trim() ? track.label.trim() : "Audio";
    const lang = typeof track.lang === "string" && track.lang.trim() ? track.lang.trim() : null;
    out.push({ index: track.index, label, lang });
    if (out.length === 32) break;
  }
  return out;
}

export type PlayJob = {
  id: string;
  state: "working" | "prompt" | "browse" | "ready" | "failed";
  error?: string | null;
  session?: string | null;
  playlist?: string | null;
  subtitles?: SubtitleTrack[];
  /** Subtitles in the file are still being read; `subtitles` fills in shortly after playback starts. */
  subtitles_pending?: boolean;
  next?: NextItem | null;
  browse_path?: string | null;
  title?: string | null;
  position?: number | null;
  duration?: number | null;
  /** The playing file's name, when its address ends in one; ranks online subtitles. */
  release?: string | null;
  /** Audio tracks in the file. Empty until the stream has been read. */
  audio?: AudioTrack[];
  /** The ffprobe index of the track that is playing. */
  audio_index?: number | null;
  /** The dialog Kodi is showing, when this response already read it. */
  prompt?: Prompt | null;
  /** True when `prompt` was read for this response, including when there is no dialog. */
  prompt_known?: boolean;
};

/** `POST /api/subtitles/search`. */
export type SubtitleSearch = {
  kind?: MediaKind | null;
  title?: string | null;
  show_title?: string | null;
  year?: number | null;
  season?: number | null;
  episode?: number | null;
  imdb_id?: string | null;
  tmdb_id?: string | null;
  tvdb_id?: string | null;
  release?: string | null;
  languages?: string[];
};

export type SubtitleCandidate = {
  /** Opaque; pass to `subtitleFileUrl`. */
  id: string;
  source: string;
  language: string;
  language_name: string;
  release?: string | null;
  hearing_impaired?: boolean;
  downloads?: number | null;
  rating?: number | null;
  score: number;
  match: "hash" | "release" | "title";
};

export type SubtitleResults = {
  items: SubtitleCandidate[];
  best?: string | null;
  notes?: string[];
};

export type SubtitleSettings = {
  languages: string[];
  hearing_impaired: boolean;
  /** An OpenSubtitles API key is saved (never sent back). */
  opensubtitles: boolean;
  opensubtitles_user: boolean;
  /** A SubDL API key is saved (never sent back). */
  subdl: boolean;
  /** A SubSource API key is saved (never sent back). */
  subsource: boolean;
};

export type SubtitleSettingsPut = {
  languages?: string[];
  hearing_impaired?: boolean;
  opensubtitles_key?: string | null;
  opensubtitles_username?: string | null;
  opensubtitles_password?: string | null;
  subdl_key?: string | null;
  subsource_key?: string | null;
};

/** WebVTT for an online subtitle, lined up with a stream that starts `offset` seconds in. */
export function subtitleFileUrl(id: string, offset: number): string {
  const at = Number.isFinite(offset) && offset > 0 ? Math.round(offset * 1000) / 1000 : 0;
  return "/api/subtitles/file/" + encodeURIComponent(id) + "?offset=" + at;
}

export type UpdateState = "idle" | "checking" | "downloading" | "ready" | "installing" | "failed";

export type UpdateInfo = {
  current: string;
  latest: string | null;
  available: boolean;
  state: UpdateState;
  progress: number | null;
  error: string | null;
  release_url: string | null;
};

export class ApiError extends Error {
  status: number;

  constructor(status: number, detail: string) {
    super(detail);
    this.name = "ApiError";
    this.status = status;
  }
}

async function send<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, init);
  if (!response.ok) {
    let detail = `Request failed (${response.status})`;
    try {
      const body = (await response.json()) as { detail?: unknown; pair?: unknown };
      if (typeof body.detail === "string" && body.detail.trim()) detail = body.detail;
      // This device is not paired (or was removed): show the pairing number.
      if (response.status === 401 && body.pair === true) window.dispatchEvent(new Event("portal:pair"));
    } catch {
      /* The body was not the error object. */
    }
    throw new ApiError(response.status, detail);
  }
  return (await response.json()) as T;
}

function jsonInit(method: string, value?: unknown): RequestInit {
  if (value === undefined) return { method };
  return {
    method,
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
  };
}

export const api = {
  status: () => send<Status>("/api/status"),
  update: () => send<UpdateInfo>("/api/update"),
  checkUpdate: () => send<UpdateInfo>("/api/update/check", jsonInit("POST")),
  /** The service accepts this only from this PC; the header keeps other origins out. */
  installUpdate: () =>
    send<UpdateInfo>("/api/update/install", { method: "POST", headers: { "x-portal-launcher": "1" } }),
  source: () => send<Source>("/api/source"),
  setSource: (source: Source) => send<{ ok: boolean }>("/api/source", jsonInit("PUT", source)),
  home: () => send<Home>("/api/home"),
  browse: (path: string) => send<BrowsePage>("/api/browse?path=" + encodeURIComponent(path)),
  search: (q: string) => send<BrowsePage>("/api/search?q=" + encodeURIComponent(q)),
  title: (id: string) => send<TitleDetail>("/api/title/" + encodeURIComponent(id)),
  list: (name: "watchlist" | "bookmarks") => send<{ items: Card[] }>("/api/" + name),
  putList: (name: "watchlist" | "bookmarks", id: string, body: ListPut) =>
    send<{ ok: boolean }>("/api/" + name + "/" + encodeURIComponent(id), jsonInit("PUT", body)),
  deleteList: (name: "watchlist" | "bookmarks", id: string) =>
    send<{ ok: boolean }>("/api/" + name + "/" + encodeURIComponent(id), jsonInit("DELETE")),
  /** Watchlist and bookmark state for many titles in one call. */
  saved: (ids: string[]) =>
    send<{ items: Record<string, SavedState> }>("/api/saved", jsonInit("POST", { ids })),
  /** Merges what is known with online lookups; can take a few seconds. */
  info: (seed: MediaInfo) => send<MediaInfo>("/api/info", jsonInit("POST", seed)),
  metadata: () => send<MetadataSettings>("/api/metadata"),
  setMetadata: (body: { online?: boolean; tmdb_key?: string | null }) =>
    send<MetadataSettings>("/api/metadata", jsonInit("PUT", body)),
  /** Online subtitles for what is playing; can take a few seconds. */
  searchSubtitles: (body: SubtitleSearch) =>
    send<SubtitleResults>("/api/subtitles/search", jsonInit("POST", body)),
  /** The subtitle's WebVTT text; errors carry the service's detail. */
  subtitleFile: async (id: string, offset: number): Promise<string> => {
    const response = await fetch(subtitleFileUrl(id, offset));
    if (!response.ok) {
      let detail = `Request failed (${response.status})`;
      try {
        const body = (await response.json()) as { detail?: unknown };
        if (typeof body.detail === "string" && body.detail.trim()) detail = body.detail;
      } catch {
        /* The body was not the error object. */
      }
      throw new ApiError(response.status, detail);
    }
    return response.text();
  },
  subtitleSettings: () => send<SubtitleSettings>("/api/subtitles/settings"),
  setSubtitleSettings: (body: SubtitleSettingsPut) =>
    send<SubtitleSettings>("/api/subtitles/settings", jsonInit("PUT", body)),
  history: () => send<{ items: HistoryEntry[] }>("/api/history"),
  updateHistory: (slot: number, name: string, row?: HistoryRef) =>
    send<{ ok: boolean }>("/api/history/" + slot, jsonInit("PUT", { name, ...historyRef(row) })),
  deleteHistory: (slot: number, row?: HistoryRef) =>
    send<{ ok: boolean }>("/api/history/" + slot + historyQuery(row), jsonInit("DELETE")),
  clearHistory: () => send<{ ok: boolean }>("/api/history", jsonInit("DELETE")),
  progress: (body: Record<string, unknown>, keepalive = false) =>
    send<{ ok: boolean }>("/api/progress", { ...jsonInit("POST", body), keepalive }),
  addons: () => send<{ items: AddonCard[] }>("/api/addons"),
  settings: (id: string) => send<SettingsDoc>("/api/addons/" + encodeURIComponent(id) + "/settings"),
  writeSetting: (id: string, settingId: string, value: string) =>
    send<{ ok: boolean }>(
      "/api/addons/" + encodeURIComponent(id) + "/settings/" + encodeURIComponent(settingId),
      jsonInit("PUT", { value }),
    ),
  action: (id: string, settingId: string) =>
    send<{ ok: boolean; browse_path?: string; settings_addon?: string; ran?: boolean }>(
      "/api/addons/" + encodeURIComponent(id) + "/actions/" + encodeURIComponent(settingId),
      jsonInit("POST"),
    ),
  runPlugin: (file: string) =>
    send<{ ok: boolean; ran?: boolean }>("/api/run", jsonInit("POST", { file })),
  /** Saves a file where Kodi can read it and returns that path. */
  uploadIncoming: async (file: File): Promise<string> => {
    const body = new FormData();
    body.append("file", file, file.name);
    const response = await fetch("/api/incoming", { method: "POST", body });
    if (!response.ok) {
      let detail = `Request failed (${response.status})`;
      try {
        const parsed = (await response.json()) as { detail?: unknown };
        if (typeof parsed.detail === "string" && parsed.detail.trim()) detail = parsed.detail;
      } catch {
        /* The body was not the error object. */
      }
      throw new ApiError(response.status, detail);
    }
    const parsed = (await response.json()) as { path?: unknown };
    if (typeof parsed.path !== "string" || !parsed.path.trim()) {
      throw new ApiError(response.status, "The file was not saved.");
    }
    return parsed.path;
  },
  prompt: () => send<{ prompt: Prompt | null; settings_addon?: string }>("/api/prompt"),
  pairMe: () => send<PairMe>("/api/pair/me"),
  pairStart: () => send<PairStart>("/api/pair/start", jsonInit("POST", {})),
  pairStatus: (ticket: string) =>
    send<{ state: "waiting" | "paired" | "expired" }>("/api/pair/status?ticket=" + encodeURIComponent(ticket)),
  pairApprove: (code: string) => send<{ ok: boolean; name: string }>("/api/pair/approve", jsonInit("POST", { code })),
  devices: () => send<PairedDevice[]>("/api/devices"),
  removeDevice: (id: string) => send<{ ok: boolean }>("/api/devices/" + encodeURIComponent(id), jsonInit("DELETE")),
  answerPrompt: (body: { id: string; action: string; text?: string; index?: number }) =>
    send<{ ok: boolean }>("/api/prompt", jsonInit("POST", body)),
  play: (body: Record<string, unknown>) => send<{ id: string }>("/api/play", jsonInit("POST", body)),
  job: (id: string) => send<PlayJob>("/api/play/" + encodeURIComponent(id)),
  seek: (id: string, seconds: number) =>
    send<PlayJob>("/api/play/" + encodeURIComponent(id) + "/seek", jsonInit("POST", { seconds })),
  /** Restarts the job's stream at `seconds` in a new quality; the add-on is not asked again. */
  setQuality: (id: string, quality: Quality, seconds: number) =>
    send<PlayJob>("/api/play/" + encodeURIComponent(id) + "/quality", jsonInit("POST", { quality, seconds })),
  /** Restarts the job's stream at `seconds` on another audio track; the add-on is not asked again. */
  setAudio: (id: string, index: number, seconds: number) =>
    send<PlayJob>("/api/play/" + encodeURIComponent(id) + "/audio", jsonInit("POST", { index, seconds })),
  stop: (id: string, keepalive = false) =>
    send<{ ok: boolean }>("/api/play/" + encodeURIComponent(id), { ...jsonInit("DELETE"), keepalive }),
  repos: () => send<{ items: RepoCard[] }>("/api/repos"),
  repo: (id: string) => send<RepoDetail>("/api/repos/" + encodeURIComponent(id)),
  installZip: (url: string, name?: string) => {
    const trimmed = name?.trim() ?? "";
    return send<InstallResult>(
      "/api/repos",
      jsonInit("POST", trimmed ? { url, name: trimmed } : { url }),
    );
  },
  installAddon: (repoId: string, addonId: string) =>
    send<InstallResult>(
      "/api/repos/" + encodeURIComponent(repoId) + "/addons/" + encodeURIComponent(addonId),
      jsonInit("POST"),
    ),
  removeRepo: (id: string) =>
    send<RemoveResult>("/api/repos/" + encodeURIComponent(id), jsonInit("DELETE")),
  removeAddon: (id: string) =>
    send<RemoveResult>("/api/addons/" + encodeURIComponent(id), jsonInit("DELETE")),
  restart: () => send<Status>("/api/engine/restart", jsonInit("POST")),
  quit: () => send<Status>("/api/engine/quit", jsonInit("POST")),
  window: (visible: boolean) => send<Status>("/api/engine/window", jsonInit("POST", { visible })),
  castInfo: () => send<CastInfo>("/api/cast"),
  setCastLan: (lan: boolean) => send<CastInfo>("/api/cast", jsonInit("PUT", { lan })),
  app: () => send<AppInfo>("/api/app"),
  appAutostart: (on: boolean) => send<AppInfo>("/api/app/autostart", jsonInit("POST", { on })),
  appEncoder: (on: boolean) => send<AppInfo>("/api/app/encoder", jsonInit("POST", { on })),
  appRestart: () => send<{ ok: boolean }>("/api/app/restart", jsonInit("POST")),
  appQuit: () => send<{ ok: boolean }>("/api/app/quit", jsonInit("POST")),
  appLogs: () => send<{ ok: boolean }>("/api/app/logs", jsonInit("POST")),
  castPresence: (id: string, name: string) =>
    send<{ ok: boolean }>("/api/cast/presence", jsonInit("POST", { id, name })),
  castReceivers: () => send<{ items: CastReceiver[] }>("/api/cast/receivers"),
  castSession: () => send<CastSession>("/api/cast/session"),
  setCastSession: (receiver_id: string, job_id: string, title?: string, replay?: CastReplay | null) =>
    send<CastSession>(
      "/api/cast/session",
      jsonInit("PUT", {
        receiver_id,
        job_id,
        title,
        kind: replay?.kind,
        id: replay?.id,
        file: replay?.file,
        season: replay?.season,
        episode: replay?.episode,
        art: replay?.art,
        show_title: replay?.showTitle,
        show_id: replay?.showId,
      }),
    ),
  clearCastSession: () => send<{ ok: boolean }>("/api/cast/session", jsonInit("DELETE")),
  /** Where the browser TV is, so the dashboard clock can follow it. */
  castPlace: (body: { position: number; duration: number; paused: boolean }) =>
    send<CastSession>("/api/cast/session/place", jsonInit("POST", body)),
  /** Play, pause, or seek on the browser TV. A seek is in source seconds. */
  castCommand: (action: "play" | "pause" | "seek", seconds?: number) =>
    send<CastSession>("/api/cast/session/command", jsonInit("POST", { action, seconds })),
  castDevices: () => send<{ items: CastDevice[] }>("/api/cast/devices"),
  castDevicePlay: (
    id: string,
    body: { url: string; title?: string; thumb?: string; job_id?: string; start?: number; subtitle?: CastSubtitle },
    replay?: CastReplay | null,
  ) =>
    send<{ ok: boolean }>(
      "/api/cast/devices/" + encodeURIComponent(id) + "/play",
      jsonInit("POST", {
        ...body,
        kind: replay?.kind,
        id: replay?.id,
        file: replay?.file,
        season: replay?.season,
        episode: replay?.episode,
        art: replay?.art,
        show_title: replay?.showTitle,
        show_id: replay?.showId,
      }),
    ),
  castDevice: () => send<DeviceCast>("/api/cast/device"),
  prefs: () => send<Partial<SharedPrefs>>("/api/prefs"),
  setPrefs: (body: Partial<Omit<SharedPrefs, "stored">>, keepalive = false) =>
    send<Partial<SharedPrefs>>("/api/prefs", { ...jsonInit("PUT", body), keepalive }),
  castDeviceResume: (id: string) =>
    send<{ ok: boolean }>("/api/cast/devices/" + encodeURIComponent(id) + "/play", jsonInit("POST", {})),
  castDeviceStop: (id: string) =>
    send<{ ok: boolean }>("/api/cast/devices/" + encodeURIComponent(id) + "/stop", jsonInit("POST", {})),
  castDevicePause: (id: string) =>
    send<{ ok: boolean }>("/api/cast/devices/" + encodeURIComponent(id) + "/pause", jsonInit("POST", {})),
  /** Shows `subtitle` on the cast, or none for null. */
  castDeviceSubtitles: (id: string, subtitle: CastSubtitle | null) =>
    send<{ ok: boolean }>("/api/cast/devices/" + encodeURIComponent(id) + "/subtitles", jsonInit("POST", subtitle)),
  /** Moves the cast to a place in the title, in seconds; the server places it in the TV's stream. */
  castDeviceSeek: (id: string, seconds: number) =>
    send<{ ok: boolean }>(
      "/api/cast/devices/" + encodeURIComponent(id) + "/seek",
      jsonInit("POST", { seconds }),
    ),
  /** Sets the TV's own volume (0 to 1) and mute; either may be left out. */
  castDeviceVolume: (id: string, change: { level?: number; muted?: boolean }) =>
    send<{ ok: boolean }>("/api/cast/devices/" + encodeURIComponent(id) + "/volume", jsonInit("POST", change)),
};

export function artUrl(path: string | null | undefined): string | null {
  if (!path) return null;
  const lower = path.toLowerCase();
  if (lower.startsWith("image://") || lower.startsWith("special://")) {
    return "/api/image?path=" + encodeURIComponent(path);
  }
  if (lower.startsWith("http://") || lower.startsWith("https://")) return path;
  return null;
}

export function route(): { page: string; id: string } {
  const hash = location.hash.startsWith("#") ? location.hash.slice(1) : "";
  const raw = hash.startsWith("/") ? hash.slice(1) : hash;
  if (!raw) return { page: "home", id: "" };
  const slash = raw.indexOf("/");
  if (slash < 0) return { page: raw, id: "" };
  const page = raw.slice(0, slash);
  const rest = raw.slice(slash + 1);
  try {
    return { page, id: decodeURIComponent(rest) };
  } catch {
    return { page, id: rest };
  }
}

export function browseHref(path: string): string {
  return "#/browse/" + encodeURIComponent(path);
}

export function titleHref(id: string): string {
  return "#/title/" + encodeURIComponent(id);
}

/** A show's episodes in its add-on, opened at `season` or where watching left off. */
export function showHref(id: string, season?: number | null): string {
  const at = season != null && Number.isInteger(season) && season >= 0 ? "/" + season : "";
  return "#/show/" + encodeURIComponent(id) + at;
}

/** The show and season a `#/show/...` address names. */
export function showRoute(): { id: string; season: number | null } {
  const hash = location.hash.replace(/^#\/?/, "");
  if (!hash.startsWith("show/")) return { id: "", season: null };
  // The id is encoded whole, so a raw slash only ever comes before the season.
  const [raw, season] = hash.slice("show/".length).split("/");
  let id = raw ?? "";
  try {
    id = decodeURIComponent(id);
  } catch {
    /* Keep the raw text. */
  }
  const number = season && /^\d{1,3}$/.test(season) ? Number(season) : null;
  return { id, season: number };
}

export function settingsHref(id: string): string {
  return "#/settings/" + encodeURIComponent(id);
}

const SEASON_LABEL = /^(?:season|series)\s+(\d+)\b/i;
const MEMORY_KEY = "portal-seasons";

type SeasonMemory = {
  seasonToShow: Record<string, string>;
  seasonNumber: Record<string, number>;
  showName: Record<string, string>;
  labels: Record<string, string>;
};

function emptyMemory(): SeasonMemory {
  return { seasonToShow: {}, seasonNumber: {}, showName: {}, labels: {} };
}

function loadMemory(): SeasonMemory {
  try {
    const raw = sessionStorage.getItem(MEMORY_KEY);
    if (!raw) return emptyMemory();
    const parsed = JSON.parse(raw) as Partial<SeasonMemory>;
    return {
      seasonToShow: parsed.seasonToShow ?? {},
      seasonNumber: parsed.seasonNumber ?? {},
      showName: parsed.showName ?? {},
      labels: parsed.labels ?? {},
    };
  } catch {
    return emptyMemory();
  }
}

let memory = loadMemory();

function saveMemory(): void {
  try {
    sessionStorage.setItem(MEMORY_KEY, JSON.stringify(memory));
  } catch {
    /* Private mode or a full quota. The labels still last for this page view. */
  }
}

export function isSeasonLabel(label: string): boolean {
  return SEASON_LABEL.test(label.trim());
}

export function notePathLabel(path: string, label: string): void {
  if (!path || !label.trim()) return;
  memory.labels[path] = label.trim();
  saveMemory();
}

export function pathLabel(path: string): string {
  return memory.labels[path] ?? "";
}

export function rememberedName(id: string): string {
  return memory.showName[id] ?? "";
}

export function rememberFolder(folder: string, items: BrowseItem[]): void {
  const titled = items.find((item) => item.show_title)?.show_title;
  if (titled) memory.showName[folder] = titled;
  const own = memory.labels[folder];
  if (!memory.showName[folder] && own && !isSeasonLabel(own)) memory.showName[folder] = own;
  for (const item of items) {
    const match = SEASON_LABEL.exec(item.label.trim());
    if (item.kind !== "open" || !match || !item.path) continue;
    memory.seasonToShow[item.path] = folder;
    memory.seasonNumber[item.path] = Number(match[1]);
    if (memory.showName[folder]) memory.showName[item.path] = memory.showName[folder];
  }
  saveMemory();
}

/**
 * Season folders the library knows for a show, so its season pages find the
 * show even when this tab never opened its season list.
 */
export function rememberShowSeasons(showId: string, name: string, folders: SeasonFolder[]): void {
  if (!showId) return;
  const named = name.trim() && !isSeasonLabel(name) ? name.trim() : "";
  if (named) memory.showName[showId] = named;
  for (const folder of folders) {
    if (!folder.path || folder.path === showId) continue;
    memory.seasonToShow[folder.path] = showId;
    memory.seasonNumber[folder.path] = folder.season;
    if (named) memory.showName[folder.path] = named;
    if (!memory.labels[folder.path]) memory.labels[folder.path] = folder.season === 0 ? "Specials" : `Season ${folder.season}`;
  }
  saveMemory();
}

/** The season folders this tab has seen for a show, lowest season first. */
export function seasonFoldersFor(showId: string): SeasonFolder[] {
  const out: SeasonFolder[] = [];
  for (const [path, show] of Object.entries(memory.seasonToShow)) {
    const season = memory.seasonNumber[path];
    if (show === showId && typeof season === "number") out.push({ season, path });
  }
  return out.sort((a, b) => a.season - b.season);
}

/** The season a folder lists, when Portal saw it named in its show's season list. */
export function seasonNumber(folder: string): number | null {
  const season = memory.seasonNumber[folder];
  return typeof season === "number" ? season : null;
}

/** The show folder a season folder was opened from, if Portal saw it. */
export function seasonParent(folder: string): string {
  return memory.seasonToShow[folder] ?? "";
}

export function showFor(folder: string, item: BrowseItem): { id: string; season: number | null } | null {
  const mapped = memory.seasonToShow[folder];
  const season = item.season ?? memory.seasonNumber[folder] ?? null;
  if (mapped) return { id: mapped, season };
  const episode = item.episode ?? null;
  if ((item.title_kind === "tv" || season) && season && episode) return { id: folder, season };
  return null;
}

/** A heading for a folder Portal has no label for, guessed from its path. */
export function folderLabel(path: string): string {
  const mark = path.indexOf("?");
  if (mark >= 0) {
    const params = [...new URLSearchParams(path.slice(mark + 1))];
    const season = params.find(([key]) => /^season$/i.test(key))?.[1] ?? "";
    const episode = params.some(([key]) => /^episode$/i.test(key));
    if (/^\d+$/.test(season) && Number(season) > 0 && !episode) return `Season ${Number(season)}`;
    const named = params.find(([key, value]) => /name|title/i.test(key) && humanPart(value));
    if (named) return named[1].trim();
    const last = [...params].reverse().find(([, value]) => humanPart(value));
    if (last) return last[1].trim();
  }
  const base = mark >= 0 ? path.slice(0, mark) : path;
  const parts = base.split("/").filter((part) => part && part !== "plugin:");
  return parts.length ? decodeFolderPart(parts[parts.length - 1]) : path;
}

/** Text a person would read: not JSON, a URL, a bare number, or a placeholder. */
function humanPart(value: string): boolean {
  const text = value.trim();
  return (
    !!text &&
    text.length <= 120 &&
    !/^[[{]/.test(text) &&
    !/^[a-z][\w+.-]*:\/\//i.test(text) &&
    !/^\d+$/.test(text) &&
    !/^(none|null|true|false|undefined)$/i.test(text)
  );
}

function decodeFolderPart(part: string): string {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
}

/** What the tray program can do for the dashboard on this PC. */
export type AppInfo = {
  /** False when the tray program is not installed beside the server. */
  tray: boolean;
  /** Whether Portal starts with Windows; absent when unknown. */
  autostart?: boolean;
  /** What encodes the video: whether a graphics card may, and the card this PC has ("cpu" for none). */
  encoder?: { hardware: boolean; card: string };
};

export type CastInfo = {
  lan: boolean;
  bind?: string;
  url?: string | null;
};

export type CastDevice = {
  id: string;
  name: string;
  host: string;
  model?: string;
};

/** `GET /api/cast/device`: a Chromecast-type cast as Portal's watch last saw it. */
/** A subtitle a cast shows: a track in the video, or one from the online search. */
export type CastSubtitle =
  | { kind: "embedded"; id: number; lang?: string | null }
  | { kind: "online"; id: string; lang?: string | null };

export type DeviceCast = {
  active: boolean;
  /** The subtitle the TV shows, if any. */
  subtitle?: CastSubtitle | null;
  /** True while Portal is still finding the TV a subtitle in the wanted language. */
  subtitle_pending?: boolean;
  /** The TV's own volume (0 to 1) and mute, when it reports them. */
  volume?: number | null;
  muted?: boolean | null;
  device_id: string | null;
  device_name: string | null;
  job_id: string | null;
  title: string | null;
  item: {
    kind: string;
    id: string;
    file: string;
    title: string;
    season?: number | null;
    episode?: number | null;
    art?: string | null;
    show_title?: string | null;
    show_id?: string | null;
  } | null;
  state: "starting" | "playing" | "paused" | "buffering" | "idle" | "next" | "ended" | "stopped" | "failed" | null;
  /** Source seconds, like the dock's clock. */
  position: number | null;
  duration: number | null;
  /** Where the device's stream starts in the source; device seeks are relative to it. */
  origin: number;
  note: string | null;
  updated_ms: number;
};

/** Player choices Portal keeps for every screen (`/api/prefs`). */
export type SharedPrefs = {
  quality: Quality;
  subtitles: "off" | "auto";
  subtitle_language: string | null;
  /** Language to prefer when a file has more than one audio track. */
  audio_language: string | null;
  autoplay: boolean;
  /** False until a choice has been saved on this PC. */
  stored: boolean;
};

export type CastReceiver = {
  id: string;
  name: string;
  last_seen?: number | null;
};

export type CastReplay = {
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

export type CastSession = {
  receiver_id?: string | null;
  job_id?: string | null;
  title?: string | null;
  kind?: string | null;
  id?: string | null;
  file?: string | null;
  season?: number | null;
  episode?: number | null;
  art?: string | null;
  show_title?: string | null;
  show_id?: string | null;
  /** Where the browser TV last said it was. */
  place?: {
    position: number;
    duration: number;
    paused: boolean;
    updated_ms: number;
  } | null;
  /** The latest play, pause, or seek for the browser TV. */
  command?: {
    seq: number;
    action: string;
    seconds?: number | null;
  } | null;
  /** A browser cast Portal ended because its TV went quiet, with where it last was. */
  lost?: {
    receiver_id?: string | null;
    job_id?: string | null;
    place?: CastSession["place"];
  } | null;
  /** Present when the server nests the live session. */
  session?: {
    receiver_id?: string | null;
    job_id?: string | null;
    title?: string | null;
    kind?: string | null;
    id?: string | null;
    file?: string | null;
    season?: number | null;
    episode?: number | null;
    art?: string | null;
    show_title?: string | null;
    show_id?: string | null;
    place?: CastSession["place"];
    command?: CastSession["command"];
  } | null;
};
