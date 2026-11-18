import { formatClock } from "./player-time";
import {
  api,
  ApiError,
  artUrl,
  browseHref,
  folderLabel,
  isSeasonLabel,
  notePathLabel,
  pathLabel,
  rememberFolder,
  rememberedName,
  route,
  rememberShowSeasons,
  seasonFoldersFor,
  seasonNumber,
  seasonParent,
  settingsHref,
  showFor,
  showHref,
  showRoute,
  titleHref,
  type AddonCard,
  type BrowseItem,
  type BrowsePage,
  type Card,
  type Episode,
  type HistoryEntry,
  type HistoryRef,
  type MediaInfo,
  type MetadataSettings,
  type PairMe,
  type PairedDevice,
  type Prompt,
  type InstallResult,
  type RemoveResult,
  type RepoAddon,
  type RepoCard,
  type SeasonFolder,
  type SettingsDoc,
  type SettingsField,
  type Source,
  type Status,
  type SubtitleSettings,
  type SubtitleSettingsPut,
  type TitleDetail,
  type UpdateInfo,
} from "./api";
import { awaitAddonPrompts, mount, play, type PlayTarget } from "./player";
import { LAN_TEXT, restartPortal, wireAppMenu } from "./appmenu";
import { syncCast } from "./cast";
import { plainSettingLabel, settingsAddon } from "./settings-link";
import { mountThemes } from "./themes";
import { showPairing } from "./pair";
import { cleanEpisodeTitle, episodeCode } from "./titles";
import { PAGE_LOCALE, esc } from "./html";
import { COMMON_LANGUAGES, browserLanguages, cleanSubtitleSettings, rememberSubtitleSettings } from "./subtitles";
import { openInfo, runtime } from "./info";
import {
  infoButtonHtml,
  loadSaved,
  noteSaved,
  savedButtonHtml,
  toggleSaved,
  type SaveTitle,
} from "./saved";
import {
  beginActivity,
  describeAction,
  describeAddonInstall,
  describeBrowse,
  describeLibrary,
  describeRemove,
  describeRepositoryInstall,
  describeRepositoryOpen,
  describeSearch,
  describeServer,
  describeSettings,
  describeSource,
  endActivity,
  setNotice,
} from "./activity";

const main = document.querySelector<HTMLElement>("#main");
if (!main) throw new Error("Missing main");

const acts = new Map<string, (node: HTMLElement) => void>();
let actN = 0;
/** The last id handed out before the previous sweep of `acts`. */
let actsSwept = 0;
let actsSweepQueued = false;
let renderToken = 0;
/** Each service's next sign-in poll, so several on one page wait on their own. */
let categoryAddon = "";
let categoryIndex = 0;
let sourceBusy = false;
let sourceFlash = "";
let clearRepoUrl = false;
let openRepoId = "";
let openRepoName = "";
let openRepoItems: RepoAddon[] = [];
/** Modules stay folded under the video add-ons until asked for. */
let openRepoModules = false;
let loadedAddons: AddonCard[] = [];
let loadedSource: Source | null = null;
let repoCards: RepoCard[] = [];
const browsePages = new Map<string, BrowsePage>();
const BROWSE_CACHE_LIMIT = 60;
// Declared with the other state: the first render (before the code below loads)
// reads them, and a later `let` would not exist yet.
let searchSeq = 0;
/** Browser tab titles per page, so several Portal tabs can be told apart. */
const PAGE_TITLES: Record<string, string> = {
  "": "Home",
  home: "Home",
  browse: "Browse",
  title: "Details",
  show: "Show",
  watchlist: "Watchlist",
  bookmarks: "Bookmarks",
  history: "History",
  source: "Source",
  settings: "Add-on settings",
  server: "Server",
  cast: "TV",
};

const WATCHLIST_EMPTY = "Nothing on your watchlist yet. Use the Watchlist button on any movie or show to add it.";
const BOOKMARKS_EMPTY = "No bookmarks yet. Use the Bookmark button on any movie, show or episode to keep it here.";

/** The add-on Portal browses, so its top folder shows its name, not its id. */
let browseSource: { path: string; name: string } | null = null;

let openSeq = 0;
/** A source list an add-on opened as a folder, and the item its sources play. */
let sourcePick: { path: string; target: PlayTarget } | null = null;
let editing: { page: "watchlist" | "bookmarks" | "history"; key: string } | null = null;
/** The title each shown history slot holds, sent with edits so a shifted row is still found. */
const historyRows = new Map<string, HistoryRef>();
let pageTicket = 0;

mount({
  onBrowse(path, picking) {
    sourcePick = picking ? { path, target: picking } : null;
    location.hash = browseHref(path);
  },
});

main.addEventListener("click", (event) => {
  const target = event.target;
  if (!(target instanceof Element)) return;
  const node = target.closest<HTMLElement>("[data-act]");
  if (!node || !main.contains(node)) return;
  const fn = acts.get(node.dataset.act ?? "");
  if (!fn) return;
  event.preventDefault();
  fn(node);
});

// The hash is the router's, so the skip link moves focus itself instead of
// navigating to "#main", which would open a page that does not exist.
document.querySelector<HTMLAnchorElement>("a.skip")?.addEventListener("click", (event) => {
  event.preventDefault();
  document.getElementById("main")?.focus();
});

let progressTimer = 0;
// A pause, stop, or finished episode moves its show's progress; the show's page follows.
window.addEventListener("portal:progress", (event) => {
  const id = (event as CustomEvent<{ id?: string }>).detail?.id;
  const shown = shownShow;
  if (!id || shown?.id !== id) return;
  window.clearTimeout(progressTimer);
  const token = renderToken;
  progressTimer = window.setTimeout(() => void refreshLibraryShow(id, shown.path, token), 500);
});

window.addEventListener("hashchange", () => {
  if (!appStarted) return;
  const page = route().page || "home";
  if (editing && !editStays(page)) editing = null;
  void render();
});

/** This device's pairing, read once at start. Null when Portal did not answer. */
let pairInfo: PairMe | null = null;
let appStarted = false;
let serverDevices: PairedDevice[] = [];
let serverDevicesError = "";
let serverDevicesNote = "";
let pairBusy = false;

/** True while a status check is in flight; declared before the first refreshChip call. */
let chipBusy = false;

/** Update state, declared before the first refreshUpdate call. */
const UPDATE_POLL_MS = 30 * 60 * 1000;
const UPDATE_WATCH_MS = 1500;
const UPDATE_WATCH_LIMIT_MS = 15 * 60 * 1000;
let updateInfo: UpdateInfo | null = null;
let updateError = "";
let updateBusy = false;
let updateFetchedAt = 0;
let updateTimer: number | undefined;
let updatePolling = false;
let updateFromVersion = "";


function act(fn: (node: HTMLElement) => void): string {
  actN += 1;
  const id = String(actN);
  acts.set(id, fn);
  // Partial repaints (filtering a list, the update card) hand out ids
  // without a full render clearing the old ones, so they are swept after.
  if (!actsSweepQueued) {
    actsSweepQueued = true;
    window.setTimeout(sweepActs, 0);
  }
  return id;
}

/**
 * Drops actions no button in the page names any more. An id handed out since
 * the previous sweep is kept even so, in case its markup is not in yet.
 */
function sweepActs(): void {
  actsSweepQueued = false;
  const live = new Set<string>();
  for (const node of main!.querySelectorAll<HTMLElement>("[data-act]")) {
    live.add(node.dataset.act ?? "");
  }
  for (const id of acts.keys()) {
    if (Number(id) <= actsSwept && !live.has(id)) acts.delete(id);
  }
  actsSwept = actN;
}

function detail(err: unknown): string {
  // fetch rejects with a TypeError when Portal cannot be reached; its own text is "Failed to fetch".
  if (err instanceof TypeError) return "Portal is not answering.";
  if (err instanceof Error && err.message.trim()) return err.message;
  return "Portal could not load this page.";
}

function showPage(input: { title: string; detail: string }, delayMs = 0): number {
  pageTicket = beginActivity("page", input, { delayMs });
  return pageTicket;
}

function hidePage(): void {
  const ticket = pageTicket;
  if (!ticket) return;
  pageTicket = 0;
  endActivity(ticket);
}

function hideTicket(ticket: number): void {
  if (!ticket || pageTicket !== ticket) return;
  hidePage();
}

function noteKodi(token: number, open: () => boolean, down: { title: string; detail: string }): void {
  void api
    .status()
    .then((status) => {
      if (!open() || token !== renderToken) return;
      if (status.state !== "running") showPage(down);
    })
    .catch(() => undefined);
}

function artBlock(art: string | null | undefined, name: string): string {
  const url = artUrl(art);
  const letter = esc((name.trim().charAt(0) || "P").toUpperCase());
  if (!url) return `<div class="art empty" aria-hidden="true">${letter}</div>`;
  return `<img class="art" alt="" src="${esc(url)}" data-letter="${letter}" loading="lazy" decoding="async" />`;
}

function bar(fraction: number | null | undefined): string {
  if (typeof fraction !== "number" || !Number.isFinite(fraction)) return "";
  const width = Math.max(0, Math.min(100, fraction * 100));
  return `<div class="bar" aria-hidden="true"><span style="width: ${width}%"></span></div>`;
}

function episodeLine(season?: number | null, episode?: number | null, name?: string | null): string {
  const code = episodeCode(season, episode);
  const title = cleanEpisodeTitle(name || "", season, episode);
  if (code && title) return `${code} · ${title}`;
  return code || title;
}

function resumeOf(value: number | null | undefined): number | undefined {
  if (value == null || !Number.isFinite(value) || value <= 60) return undefined;
  return value;
}

function paintError(err: unknown): void {
  main!.innerHTML = `<p class="error">${esc(detail(err))}</p>`;
}

async function refreshChip(): Promise<void> {
  const chip = document.getElementById("chip");
  if (!chip || chipBusy) return;
  chipBusy = true;
  try {
    const status = await api.status();
    chip.textContent = status.state;
    chip.className = "chip " + status.state;
    chip.title = "Kodi engine: " + status.state;
  } catch {
    chip.textContent = "offline";
    chip.className = "chip stopped";
    chip.title = "Portal is not answering";
  } finally {
    chipBusy = false;
  }
}

function markNav(page: string): void {
  const here = page === "" ? "home" : page;
  document.querySelectorAll<HTMLAnchorElement>(".top nav a").forEach((link) => {
    const target = (link.getAttribute("href") ?? "").replace(/^#\/?/, "").split("/")[0] || "home";
    if (target === here) link.setAttribute("aria-current", "page");
    else link.removeAttribute("aria-current");
  });
}

async function render(): Promise<void> {
  hidePage();
  setNotice("");
  const token = ++renderToken;
  acts.clear();
  // A card, link or button in the page goes away with the repaint; focus then
  // moves to the new page instead of dropping to the top of the document.
  const focusWasInMain = main!.contains(document.activeElement);
  const here = route();
  markNav(here.page);
  document.title = (PAGE_TITLES[here.page] ?? "Portal") === "Portal" ? "Portal" : PAGE_TITLES[here.page] + " · Portal";
  try {
    if (here.page === "cast") {
      syncCast(true);
      main!.replaceChildren();
      return;
    }
    syncCast(false);
    if (here.page === "home" || here.page === "") await renderHome(token);
    else if (here.page === "browse") await renderBrowse(token, here.id);
    else if (here.page === "title") await renderTitle(token, here.id);
    else if (here.page === "show") await renderShow(token);
    else if (here.page === "watchlist") await renderList(token, "watchlist");
    else if (here.page === "bookmarks") await renderList(token, "bookmarks");
    else if (here.page === "history") await renderHistory(token);
    else if (isRemote() && (here.page === "source" || here.page === "settings" || here.page === "server")) {
      main!.innerHTML = `<h1>Settings</h1><p class="muted">Settings change only on the PC running Portal. This device can watch and cast.</p>`;
    } else if (here.page === "source") await renderSource(token);
    else if (here.page === "settings") await renderSettings(token, here.id);
    else if (here.page === "server") await renderServer(token);
    else main!.innerHTML = `<h1>Page not found</h1><p class="muted">Portal has no page at this address. <a href="#/">Back to Home</a></p>`;
  } catch (err) {
    if (token !== renderToken) return;
    hidePage();
    paintError(err);
  }
  if (focusWasInMain && token === renderToken && focusLost()) main!.focus({ preventScroll: true });
}

function focusLost(): boolean {
  return !document.activeElement || document.activeElement === document.body;
}

/**
 * Repaints part of a page and puts focus back on the control in the same place,
 * so a keyboard press on a button that the repaint replaces does not lose it.
 */
function keepFocus(root: HTMLElement, paint: () => void): void {
  const controls = (): HTMLElement[] => Array.from(root.querySelectorAll<HTMLElement>("button, a[href], input, select"));
  const active = document.activeElement;
  const index = active instanceof HTMLElement ? controls().indexOf(active) : -1;
  paint();
  if (index < 0 || !focusLost()) return;
  controls()[index]?.focus();
}

async function renderHome(token: number): Promise<void> {
  showPage(describeLibrary("home"), 350);
  try {
  const [home, source] = await Promise.all([api.home(), api.source().catch(() => null)]);
  if (token !== renderToken) return;
  const root = source?.path
    ? `<p><a href="${esc(browseHref(source.path))}">${esc(source.name || "Browse the source")}</a></p>`
    : isRemote()
      ? `<p class="muted">Choose a video add-on on the PC running Portal to fill the shelves.</p>`
      : `<p class="muted"><a href="#/source">Choose a video add-on</a> to fill the shelves.</p>`;
  main!.innerHTML = `
    <h1>Home</h1>
    ${root}
    ${shelf("Continue watching", home.continue_watching, "Nothing is in progress.")}
    ${shelf("Watchlist", home.watchlist, WATCHLIST_EMPTY, "watchlist")}
    ${shelf("Bookmarks", home.bookmarks, BOOKMARKS_EMPTY, "bookmarks")}
  `;
  bindRenames(main!);
  } finally {
    if (token === renderToken) hidePage();
  }
}

function shelf(title: string, cards: Card[], empty: string, manage?: "watchlist" | "bookmarks"): string {
  const body = cards.length
    ? `<div class="scroller">${cards.map((card) => cardHtml(card, manage)).join("")}</div>`
    : `<p class="muted">${esc(empty)}</p>`;
  return `<section class="shelf"><h2>${esc(title)}</h2>${body}</section>`;
}

function cardHtml(card: Card, manage?: "watchlist" | "bookmarks"): string {
  const extra = episodeLine(card.season, card.episode, card.episode_name);
  const kind = extra || (card.kind === "tv" ? "Show" : ["Movie", card.year ? String(card.year) : ""].filter(Boolean).join(" · "));
  const action = cardAction(card);
  const tools = manage ? listTools(manage, card) : "";
  // The add-on's poster for the title reads better on a shelf than an
  // episode still squeezed into a poster frame; the line below names the episode.
  const art = posterFromId(card.id) || card.art;
  // A show's episodes are a click away from any of its cards; Play already
  // opens them for a show with nothing to resume.
  const more =
    card.kind === "tv" && card.file
      ? `<a href="${esc(showHref(card.id, card.season))}">Episodes</a>`
      : `<a href="${esc(titleHref(card.id))}">Details</a>`;
  return `
    <article class="card">
      <button type="button" class="card-play" data-act="${act(() => void playCard(card))}" aria-label="${esc(action + ": " + card.name)}">
        ${artBlock(art, card.name)}
        <strong>${esc(card.name)}</strong>
        <span class="muted">${esc(kind)}</span>
        ${bar(card.fraction)}
        <span class="card-cta">${esc(action)}</span>
      </button>
      <div class="meta">
        ${more}
        ${tools}
      </div>
    </article>
  `;
}

/**
 * The poster an add-on put in a title's own address (some add-ons pass
 * `art={"poster": ...}` in their plugin links), if any.
 */
function posterFromId(id: string): string | null {
  return artFromId(id, ["poster", "tvshow.poster"]);
}

/** The first of `keys` an add-on put in the `art` of a title's address. */
function artFromId(id: string, keys: string[]): string | null {
  const at = id.indexOf("?");
  if (at < 0) return null;
  try {
    const raw = new URLSearchParams(id.slice(at + 1)).get("art");
    if (!raw) return null;
    const art = JSON.parse(raw) as Record<string, unknown>;
    for (const key of keys) {
      const value = art[key];
      if (typeof value === "string" && /^https:\/\//.test(value)) return value;
    }
    return null;
  } catch {
    return null;
  }
}

/** What pressing a card does, in words: Play, Resume 12:34, Play again, or Episodes. */
function cardAction(card: Card): string {
  if (card.kind === "tv" && !card.file) return "Episodes";
  const resume = resumeOf(card.resume_seconds);
  if (resume) return "Resume " + formatClock(resume);
  if (typeof card.fraction === "number" && card.fraction >= 0.95) return "Play again";
  return "Play";
}

function playCard(card: Card): void {
  if (card.kind === "tv") {
    const episodes = showHref(card.id, card.season);
    if (!card.file) {
      location.hash = episodes;
      return;
    }
    play({
      kind: "episode",
      id: card.id,
      showId: card.id,
      file: card.file,
      title: episodeLine(card.season, card.episode, card.episode_name) || card.episode_name || card.name,
      showTitle: card.name,
      season: card.season ?? undefined,
      episode: card.episode ?? undefined,
      art: card.art,
      resumeSeconds: resumeOf(card.resume_seconds),
      progressKind: "tv",
      progressId: card.id,
      progressName: card.name,
      trackProgress: true,
    });
    // The season opens under the player, so stopping lands on its episodes.
    location.hash = episodes;
    return;
  }
  play(movieTarget(card.id, card.file || card.id, card.name, card.art, resumeOf(card.resume_seconds)));
}

function movieTarget(id: string, file: string, name: string, art: string | null | undefined, resume?: number): PlayTarget {
  return {
    kind: "movie",
    id,
    file,
    title: name,
    art,
    resumeSeconds: resume,
    progressKind: "movie",
    progressId: id,
    progressName: name,
    trackProgress: true,
  };
}

/** Keeps the folders opened most recently, so a long session does not hold every page it saw. */
function cacheBrowse(path: string, page: BrowsePage): void {
  browsePages.delete(path);
  browsePages.set(path, page);
  if (browsePages.size <= BROWSE_CACHE_LIMIT) return;
  const oldest = browsePages.keys().next().value;
  if (oldest !== undefined) browsePages.delete(oldest);
}

function browseHeading(path: string): string {
  if (browseSource && path === browseSource.path && browseSource.name) return browseSource.name;
  return pathLabel(path) || folderLabel(path) || "Folder";
}

async function renderBrowse(token: number, path: string): Promise<void> {
  if (!path.trim()) {
    main!.innerHTML = `<p class="error">Missing a folder.</p>`;
    return;
  }
  if (!browseSource) {
    const source = await api.source().catch(() => null);
    if (token !== renderToken) return;
    if (source?.path) browseSource = { path: source.path, name: source.name || "" };
  }
  const cached = browsePages.get(path);
  const name = browseHeading(path);
  let open = true;
  if (cached && token === renderToken) {
    paintBrowse(cached, browseHeading(cached.path), false);
  }
  showPage(describeBrowse(name, cached ? "refresh" : "open"), cached ? 400 : 0);
  noteKodi(token, () => open, describeBrowse(name, "starting"));
  // A search started meanwhile owns the page; this folder's answer only refreshes the cache.
  const searchAtStart = searchSeq;
  const current = (): boolean => token === renderToken && searchSeq === searchAtStart;
  try {
    const page = await api.browse(path);
    open = false;
    cacheBrowse(path, page);
    if (!current()) return;
    paintBrowse(page, browseHeading(page.path), false);
    setNotice("");
  } catch (err) {
    open = false;
    if (!current()) return;
    if (cached) {
      setNotice(detail(err));
      return;
    }
    paintError(err);
  } finally {
    if (current()) hidePage();
  }
}

function paintBrowse(page: BrowsePage, heading: string, searched: boolean, query = "", checkLibrary = true): void {
  if (!searched && sourcePick && sourcePick.path === page.path) {
    shownShow = null;
    paintSources(page, sourcePick.target);
    return;
  }
  rememberFolder(page.path, page.items);
  const token = renderToken;
  const show = searched ? null : pageShow(page);
  const ids = page.items.map((item) => mediaOf(page.path, item)?.save.id ?? "");
  if (show) ids.push(show.save.id);
  const q = query.trim();
  const view = show ? showView(page, show) : null;
  shownShow = view ? { id: view.show.save.id, path: page.path } : null;
  const items = page.items.length
    ? view?.episodes
      ? `<div class="ep-list">${page.items.map((item) => episodeItem(page.path, item, view)).join("")}</div>`
      : `<div class="grid">${page.items.map((item) => browseCard(page.path, item, view)).join("")}</div>`
    : `<p class="muted">${searched ? (q ? `No results for “${esc(q)}”.` : "No results.") : "This folder is empty."}</p>`;
  const title = searched ? (q ? `Results for “${q}”` : "Results") : heading || "Folder";
  const search = `
    <form class="search">
      <input type="search" name="q" placeholder="Search this add-on" aria-label="Search this add-on" value="${esc(q)}" />
      <button type="submit">Search</button>
      ${searched ? `<button type="button" data-act="${act(() => void render())}">Clear</button>` : ""}
    </form>
  `;
  if (view) {
    document.title = view.show.save.name + " · Portal";
    // The show leads; searching the add-on is still there, under its episodes.
    main!.innerHTML = `${showHero(view, page, heading)}${seasonTabs(view, page.path)}${items}${search}`;
  } else {
    main!.innerHTML = `
      <div class="row">
        <div class="grow">
          <p class="muted">${searched ? "Search" : "Browse"}${isRemote() ? "" : ` · <a href="#/source">Source</a>`}</p>
          <h1>${esc(title)}</h1>
        </div>
      </div>
      ${search}
      ${items}
    `;
  }
  const form = main!.querySelector<HTMLFormElement>("form.search");
  form?.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = new FormData(form).get("q");
    void runSearch(typeof value === "string" ? value : "");
  });
  // Paint first; the saved state fills in when it arrives, unless the page moved on.
  void loadSaved(ids, () => token === renderToken);
  if (view) afterShowPaint(page, view, token, checkLibrary);
}

async function runSearch(query: string): Promise<void> {
  const token = renderToken;
  const q = query.trim();
  if (!q) {
    setNotice("Enter a search.");
    focusBrowseSearch();
    return;
  }
  // A newer search on the same page wins, whichever answers first.
  const mine = ++searchSeq;
  const current = (): boolean => token === renderToken && mine === searchSeq;
  let open = true;
  showPage(describeSearch(q, false));
  noteKodi(token, () => open && mine === searchSeq, describeSearch(q, true));
  try {
    const page = await api.search(q);
    open = false;
    if (!current()) return;
    const hadFocus = main!.contains(document.activeElement);
    paintBrowse(page, "Results", true, q);
    // The search box was repainted with the results; keep the keyboard in it.
    if (hadFocus && focusLost()) main!.querySelector<HTMLInputElement>("form.search input[name='q']")?.focus();
    setNotice("");
  } catch (err) {
    open = false;
    if (!current()) return;
    setNotice(detail(err));
  } finally {
    if (current()) hidePage();
  }
}

/** A browse item Portal can save or look up: a movie, an episode (saved as its show), or a show folder. */
type Media = {
  type: "movie" | "episode" | "show";
  save: SaveTitle;
  seed: MediaInfo;
  art: string | null | undefined;
};

type ShowRef = { id: string; name: string; art?: string | null; info?: MediaInfo | null };

/** Show folders opened from browse, keyed by the folder path. */
const openedShows = new Map<string, ShowRef>();

function isSettingsItem(item: BrowseItem): boolean {
  return !!(item.settings_addon || settingsAddon(item.path));
}

function showName(id: string, hint?: string | null): string {
  const given = hint?.trim();
  if (given) return given;
  const opened = openedShows.get(id)?.name;
  if (opened) return opened;
  const known = rememberedName(id);
  if (known && !isSeasonLabel(known)) return known;
  const label = pathLabel(id);
  if (label && !isSeasonLabel(label)) return label;
  return folderLabel(id);
}

function yearOf(info: MediaInfo | null | undefined): number | null {
  return typeof info?.year === "number" && Number.isFinite(info.year) ? info.year : null;
}

/** The show an episode item belongs to; show_path wins, as in the library. */
function episodeShow(
  folder: string,
  item: BrowseItem,
): { id: string; name: string; season: number | null; episode: number } | null {
  const info = item.info ?? null;
  const episode = item.episode ?? info?.episode ?? null;
  if (!episode) return null;
  const show = showFor(folder, item);
  if (!show && !item.show_path && info?.kind !== "episode") return null;
  const id = item.show_path || show?.id || seasonParent(folder) || folder;
  return {
    id,
    name: showName(id, item.show_title || info?.show_title),
    season: show?.season ?? item.season ?? info?.season ?? null,
    episode,
  };
}

function movieMedia(item: BrowseItem): Media {
  const info = item.info ?? null;
  const name = info?.title || item.label;
  return {
    type: "movie",
    save: { id: item.path, kind: "movie", name, art: item.art ?? null, year: yearOf(info), imdb_id: info?.imdb_id ?? null },
    seed: { ...(info ?? {}), kind: "movie", title: name },
    art: item.art,
  };
}

function mediaOf(folder: string, item: BrowseItem): Media | null {
  if (!item.path || isSettingsItem(item)) return null;
  if (item.kind === "run") return null;
  const info = item.info ?? null;
  if (item.kind === "play") {
    const show = episodeShow(folder, item);
    if (show) {
      const opened = openedShows.get(show.id);
      return {
        type: "episode",
        // The episode's own art is a still; the show's poster suits the watchlist better.
        save: { id: show.id, kind: "tv", name: show.name, art: opened?.art ?? info?.poster ?? item.art ?? null },
        seed: {
          ...(info ?? {}),
          kind: "episode",
          title: info?.title || cleanEpisodeTitle(item.label, show.season, show.episode, show.name) || item.label,
          show_title: info?.show_title || show.name,
          season: show.season,
          episode: show.episode,
        },
        art: item.art,
      };
    }
    // A TV item Portal cannot tie to a show has nothing to save.
    if (item.title_kind === "tv" && info?.kind !== "movie") return null;
    return movieMedia(item);
  }
  if (item.kind !== "open" || isSeasonLabel(item.label)) return null;
  const kind = info?.kind ?? item.title_kind ?? null;
  if (kind === "tv" && !item.season && !item.episode) {
    const id = item.show_path || item.path;
    const name = item.show_title || info?.show_title || info?.title || item.label;
    return {
      type: "show",
      save: { id, kind: "tv", name, art: item.art ?? null, year: yearOf(info), imdb_id: info?.imdb_id ?? null },
      seed: { ...(info ?? {}), kind: "tv", title: name },
      art: item.art,
    };
  }
  if (kind === "movie") return movieMedia(item);
  return null;
}

function showMedia(ref: ShowRef): Media {
  return {
    type: "show",
    save: {
      id: ref.id,
      kind: "tv",
      name: ref.name,
      art: ref.art ?? null,
      year: yearOf(ref.info),
      imdb_id: ref.info?.imdb_id ?? null,
    },
    seed: { ...(ref.info ?? {}), kind: "tv", title: ref.name },
    art: ref.art,
  };
}

/** The show this page belongs to: opened from a show folder, a season of one, or one show's episodes. */
function pageShow(page: BrowsePage): Media | null {
  const opened = openedShows.get(page.path);
  if (opened) return showMedia(opened);
  const parent = seasonParent(page.path);
  if (parent) return showMedia(openedShows.get(parent) ?? { id: parent, name: showName(parent) });
  const episodes = page.items
    .map((item) => mediaOf(page.path, item))
    .filter((media): media is Media => media?.type === "episode");
  if (episodes.length && episodes.every((media) => media.save.id === episodes[0].save.id)) {
    const first = episodes[0].save;
    return showMedia(openedShows.get(first.id) ?? { id: first.id, name: first.name, art: first.art });
  }
  const seasons = page.items.filter((item) => item.kind === "open" && isSeasonLabel(item.label)).length;
  const others = page.items.filter((item) => !isSettingsItem(item) && item.kind !== "run").length;
  if (seasons && seasons * 2 >= others) return showMedia({ id: page.path, name: showName(page.path) });
  return null;
}

/** A show's own page or one of its seasons, with what the library knows about it. */
type ShowView = {
  show: Media;
  library: TitleDetail | null;
  /** The season this page lists; null on the show's own page. */
  season: number | null;
  /** The page lists the show's episodes, so they show as rows. */
  episodes: boolean;
  /** The episode to resume or watch next, once something of the show was played. */
  next: { season: number; episode: number } | null;
};

const PLAY_ICON = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l10.5-6.5z" /></svg>`;

/** Library detail for shows this tab looked up; null when the library has no such show. */
const libraryShows = new Map<string, TitleDetail | null>();
/** The episode a show address meant, scrolled into view once its season paints. */
let landing: { path: string; season: number; episode: number } | null = null;
/** The show the browse page shows now, so a progress change can refresh it. */
let shownShow: { id: string; path: string } | null = null;

function seasonName(season: number): string {
  return season === 0 ? "Specials" : `Season ${season}`;
}

/** The library's show, filed so its season pages know the show and its art. */
function noteLibraryShow(title: TitleDetail): void {
  libraryShows.set(title.id, title);
  if (title.kind !== "tv") return;
  rememberShowSeasons(title.id, title.name, title.season_folders ?? []);
  const opened = openedShows.get(title.id);
  openedShows.set(title.id, {
    id: title.id,
    name: opened?.name || title.name,
    // The library keeps the latest episode's still as the show's art; a poster reads better.
    art: opened?.art ?? posterFromId(title.id) ?? title.art ?? null,
    info: opened?.info ?? { kind: "tv", title: title.name, year: title.year ?? null, imdb_id: title.imdb_id ?? null },
  });
}

/** The episode to resume or watch next, or null when nothing of the show was played. */
function whereWatching(title: TitleDetail): { season: number; episode: number } | null {
  const played = title.seasons.some((season) => season.episodes.some((episode) => episode.position > 0 || episode.finished));
  if (!played || !title.next) return null;
  return { season: title.next.season, episode: title.next.episode };
}

/** Every season folder known for the show, from the library and from this tab. */
function showSeasons(showId: string): SeasonFolder[] {
  const bySeason = new Map<number, string>();
  for (const folder of libraryShows.get(showId)?.season_folders ?? []) bySeason.set(folder.season, folder.path);
  for (const folder of seasonFoldersFor(showId)) if (!bySeason.has(folder.season)) bySeason.set(folder.season, folder.path);
  return [...bySeason.entries()].map(([season, path]) => ({ season, path })).sort((a, b) => a.season - b.season);
}

function libraryEpisode(title: TitleDetail, path: string, season: number | null, episode: number): Episode | undefined {
  let byNumber: Episode | undefined;
  for (const group of title.seasons) {
    for (const item of group.episodes) {
      if (item.path === path) return item;
      if (!byNumber && item.season === season && item.episode === episode) byNumber = item;
    }
  }
  return byNumber;
}

/** Opens a show from the library at the season being watched, or at its season list. */
async function renderShow(token: number): Promise<void> {
  const { id, season } = showRoute();
  if (!id) {
    main!.innerHTML = `<p class="error">Missing a show.</p>`;
    return;
  }
  showPage(describeLibrary("title"), 350);
  let title: TitleDetail | null = null;
  try {
    title = await api.title(id);
  } catch (err) {
    // A show the library has not filed yet still opens as the add-on lists it.
    if (!(err instanceof ApiError && err.status === 404)) throw err;
  } finally {
    if (token === renderToken) hidePage();
  }
  if (token !== renderToken) return;
  let folder = id;
  landing = null;
  if (title) {
    noteLibraryShow(title);
    const at = whereWatching(title);
    const wanted = season ?? at?.season ?? null;
    const found = wanted == null ? undefined : showSeasons(title.id).find((item) => item.season === wanted);
    if (found) folder = found.path;
    if (at && (found ? found.season === at.season : season == null)) {
      landing = { path: folder, season: at.season, episode: at.episode };
    }
  }
  // Replaced, so Back skips this address instead of opening the show again.
  location.replace(browseHref(folder));
}

function showView(page: BrowsePage, show: Media): ShowView {
  const library = libraryShows.get(show.save.id) ?? null;
  const listed = page.items.filter((item) => !isSettingsItem(item) && item.kind !== "run");
  const episodes = listed.filter((item) => item.kind === "play" && episodeShow(page.path, item));
  const seasons = new Set(
    episodes.map((item) => item.season ?? item.info?.season ?? null).filter((n): n is number => n != null),
  );
  const season =
    page.path === show.save.id && !episodes.length
      ? null
      : (seasonNumber(page.path) ??
        library?.season_folders?.find((folder) => folder.path === page.path)?.season ??
        (seasons.size === 1 ? [...seasons][0] : null));
  return {
    show,
    library,
    season,
    episodes: episodes.length > 0 && episodes.length * 2 >= listed.length,
    next: library ? whereWatching(library) : null,
  };
}

/** Resume or play the episode the show is up to, from anywhere in the show. */
function heroPlay(view: ShowView): string {
  const library = view.library;
  const at = view.next;
  if (!library || !at) return "";
  const code = episodeCode(at.season, at.episode);
  const known = libraryEpisode(library, library.next?.path ?? "", at.season, at.episode);
  const resume = known?.resumable ? resumeOf(known.position) : undefined;
  const run = act(() => (known ? playEpisode(library, known, !resume) : playTitle(library, true)));
  const label = resume ? `Resume ${code} · ${formatClock(resume)}` : `Play ${code}`;
  return `<button type="button" class="hero-play" data-act="${run}">${PLAY_ICON}<span>${esc(label)}</span></button>`;
}

function showHero(view: ShowView, page: BrowsePage, heading: string): string {
  const { show, library } = view;
  const name = show.save.name;
  const poster = posterFromId(show.save.id) || show.art || library?.art;
  const fanart = artUrl(artFromId(show.save.id, ["fanart", "tvshow.fanart", "landscape"]) ?? show.seed.fanart);
  const here = view.season != null ? seasonName(view.season) : page.path !== show.save.id ? heading : "";
  const root = browseSource
    ? `<a href="${esc(browseHref(browseSource.path))}">${esc(browseSource.name || "Browse")}</a>`
    : isRemote()
      ? `<span>Browse</span>`
      : `<a href="#/source">Source</a>`;
  const crumbs = [
    root,
    here ? `<a href="${esc(browseHref(show.save.id))}">${esc(name)}</a>` : `<span aria-current="page">${esc(name)}</span>`,
    here ? `<span aria-current="page">${esc(here)}</span>` : "",
  ].filter(Boolean);
  const facts: string[] = [];
  if (here) facts.push(here);
  const year = yearOf(show.seed);
  if (year && !here) facts.push(String(year));
  if (view.episodes) {
    const rows = page.items.filter((item) => item.kind === "play" && episodeShow(page.path, item));
    facts.push(`${rows.length} ${rows.length === 1 ? "episode" : "episodes"}`);
    if (library) {
      const watched = rows.filter((item) => {
        const numbers = episodeShow(page.path, item);
        return numbers && libraryEpisode(library, item.path, numbers.season ?? view.season, numbers.episode)?.finished;
      }).length;
      if (watched) facts.push(watched === rows.length ? "All watched" : `${watched} watched`);
    }
  } else if (library) {
    const watched = library.seasons.reduce((sum, season) => sum + season.episodes.filter((episode) => episode.finished).length, 0);
    if (watched) facts.push(`${watched} ${watched === 1 ? "episode" : "episodes"} watched`);
  }
  const buttons = mediaButtons(show, "pill", (node) => openMediaInfo(show, node, null));
  return `
    <section class="show-hero${fanart ? " has-fanart" : ""}" aria-labelledby="show-title">
      ${fanart ? `<div class="show-hero-backdrop" aria-hidden="true"><img alt="" src="${esc(fanart)}" decoding="async" /></div>` : ""}
      ${artBlock(poster, name)}
      <div class="show-hero-text">
        <nav class="crumbs" aria-label="Breadcrumb">${crumbs.join('<span class="crumb-sep" aria-hidden="true">›</span>')}</nav>
        <h1 id="show-title">${esc(name)}</h1>
        ${facts.length ? `<p class="show-hero-facts">${esc(facts.join(" · "))}</p>` : ""}
        <div class="show-hero-actions">${heroPlay(view)}${buttons}</div>
      </div>
    </section>
  `;
}

/** The show's seasons as tabs on a season page, so the next one is one click away. */
function seasonTabs(view: ShowView, path: string): string {
  if (view.season == null) return "";
  const folders = showSeasons(view.show.save.id);
  if (folders.length < 2) return "";
  // Specials go last, after the numbered seasons.
  const ordered = [...folders.filter((folder) => folder.season > 0), ...folders.filter((folder) => folder.season === 0)];
  const tabs = ordered.map((folder) => {
    const current = folder.path === path || folder.season === view.season;
    const done = seasonWatched(view.library, folder.season);
    const mark = done ? `<span class="tab-done" aria-hidden="true">✓</span><span class="sr-only">, watched</span>` : "";
    return `<a href="${esc(browseHref(folder.path))}"${current ? ` aria-current="page"` : ""}>${esc(seasonName(folder.season))}${mark}</a>`;
  });
  return `
    <nav class="season-tabs" aria-label="Seasons">
      <a href="${esc(browseHref(view.show.save.id))}">All seasons</a>
      ${tabs.join("")}
    </nav>
  `;
}

/** How far into a season the library says watching got, for its card on the show's page. */
function seasonProgress(view: ShowView, season: number): { note: string; fraction: number; current: boolean } | null {
  const episodes = view.library?.seasons.find((item) => item.number === season)?.episodes ?? [];
  const current = view.next?.season === season;
  const watched = episodes.filter((episode) => episode.finished).length;
  if (current && view.next) {
    return { note: "Up next " + episodeCode(view.next.season, view.next.episode), fraction: episodes.length ? watched / episodes.length : 0, current };
  }
  if (!watched) return null;
  const note = watched === episodes.length ? "Watched" : `${watched} of ${episodes.length} watched`;
  return { note, fraction: watched / episodes.length, current };
}

function seasonWatched(library: TitleDetail | null, season: number): boolean {
  const episodes = library?.seasons.find((item) => item.number === season)?.episodes ?? [];
  return episodes.length > 0 && episodes.every((episode) => episode.finished);
}

/** One episode as a row: its still, what the add-on says about it, and where watching left it. */
function episodeItem(folder: string, item: BrowseItem, view: ShowView): string {
  const media = mediaOf(folder, item);
  const numbers = item.kind === "play" ? episodeShow(folder, item) : null;
  if (!numbers || media?.type !== "episode") {
    const label = item.label || "Untitled";
    return `<div class="ep-other"><button type="button" class="source-row" data-act="${act(() => void openItem(folder, item))}">${esc(label)}</button></div>`;
  }
  const info = item.info ?? null;
  const season = numbers.season ?? view.season;
  const code = episodeCode(season, numbers.episode);
  const name = cleanEpisodeTitle(info?.title || item.label, season, numbers.episode, item.show_title) || item.label || code;
  const known = view.library ? libraryEpisode(view.library, item.path, season, numbers.episode) : undefined;
  const resume = known?.resumable ? resumeOf(known.position) : undefined;
  const finished = !!known?.finished;
  const fraction = !finished && known && known.duration > 0 && known.position > 0 ? known.position / known.duration : null;
  const next = !!view.next && view.next.season === season && view.next.episode === numbers.episode;
  const playFrom = (at?: number) => () => void openItem(folder, item, at);
  const go = act(playFrom(resume));
  const facts = [
    code,
    airedOn(info?.premiered),
    runtime(info?.runtime_minutes),
    typeof info?.rating === "number" && info.rating > 0 ? "★ " + Math.min(10, info.rating).toFixed(1) : "",
  ].filter(Boolean);
  const plot = info?.plot?.trim() ?? "";
  const infoId = act((node) => openMediaInfo(media, node, { label: resume ? "Resume" : "Play", run: playFrom(resume) }));
  const flag = finished
    ? `<span class="ep-flag">Watched</span>`
    : next
      ? `<span class="ep-flag is-next">${resume ? "Continue" : "Up next"}</span>`
      : "";
  const goLabel = resume ? "Resume " + formatClock(resume) : finished ? "Play again" : "Play";
  return `
    <article class="ep-row${next ? " is-next" : ""}${finished ? " is-watched" : ""}" data-ep="${esc(`${season ?? ""}-${numbers.episode}`)}">
      <button type="button" class="ep-thumb" data-act="${go}" aria-label="${esc(goLabel + ": " + [code, name].filter(Boolean).join(" · "))}">
        ${artBlock(item.art, name)}
        <span class="ep-play" aria-hidden="true">${PLAY_ICON}</span>
        ${flag}
        ${fraction != null ? bar(fraction) : ""}
      </button>
      <div class="ep-body">
        <p class="ep-facts">${esc(facts.join(" · "))}</p>
        <h3 class="ep-title">${esc(name)}</h3>
        ${plot ? `<p class="ep-plot">${esc(plot)}</p>` : ""}
        <div class="ep-actions">
          <button type="button" class="pill-btn ep-go" data-act="${go}">${PLAY_ICON}<span>${esc(goLabel)}</span></button>
          ${resume ? `<button type="button" class="pill-btn" data-act="${act(playFrom(undefined))}">Start over</button>` : ""}
          ${infoButtonHtml(infoId, name, "pill")}
        </div>
      </div>
    </article>
  `;
}

/** "Apr 4, 2005" from an add-on's YYYY-MM-DD date. */
function airedOn(value: string | null | undefined): string {
  if (!value || !/^\d{4}-\d{2}-\d{2}/.test(value)) return "";
  const date = new Date(value.slice(0, 10) + "T12:00:00");
  if (Number.isNaN(date.getTime())) return "";
  return date.toLocaleDateString(PAGE_LOCALE, { year: "numeric", month: "short", day: "numeric" });
}

/** Centers the current season tab, scrolls to the episode a show address meant, and catches up on progress. */
function afterShowPaint(page: BrowsePage, view: ShowView, token: number, checkLibrary: boolean): void {
  const tabs = main!.querySelector<HTMLElement>(".season-tabs");
  const current = tabs?.querySelector<HTMLElement>("[aria-current]");
  if (tabs && current) tabs.scrollLeft = current.offsetLeft - (tabs.clientWidth - current.offsetWidth) / 2;
  if (landing && landing.path === page.path) {
    const row = main!.querySelector<HTMLElement>(`[data-ep="${CSS.escape(`${landing.season}-${landing.episode}`)}"]`);
    if (row) {
      landing = null;
      // Under the open player, the top of the page is what stays in view.
      row.scrollIntoView({ block: document.body.classList.contains("playing") ? "start" : "center" });
    }
  }
  if (checkLibrary) void refreshLibraryShow(view.show.save.id, page.path, token);
}

/** Looks the show up in the library again and repaints its page if anything changed. */
async function refreshLibraryShow(id: string, path: string, token: number): Promise<void> {
  const seq = searchSeq;
  let fresh: TitleDetail | null = null;
  try {
    fresh = await api.title(id);
  } catch (err) {
    if (!(err instanceof ApiError && err.status === 404)) return;
  }
  if (token !== renderToken || seq !== searchSeq || shownShow?.path !== path) return;
  const before = libraryShows.get(id) ?? null;
  if (fresh) noteLibraryShow(fresh);
  else libraryShows.set(id, null);
  if (JSON.stringify(before) === JSON.stringify(fresh)) return;
  const page = browsePages.get(path);
  if (!page) return;
  keepFocus(main!, () => paintBrowse(page, browseHeading(page.path), false, "", false));
}

function mediaButtons(media: Media, variant: "icon" | "pill", info: (node: HTMLElement) => void): string {
  const watch = act(() => void toggleSaved("watchlist", media.save));
  const book = act(() => void toggleSaved("bookmarks", media.save));
  const name = media.type === "episode" ? media.seed.title || media.save.name : media.save.name;
  return (
    savedButtonHtml("watchlist", media.save, watch, variant) +
    savedButtonHtml("bookmarks", media.save, book, variant) +
    infoButtonHtml(act(info), name, variant)
  );
}

function openMediaInfo(media: Media, opener: HTMLElement, run: { label: string; run: () => void } | null): void {
  openInfo({ seed: media.seed, art: media.art, save: media.save, play: run, opener });
}

/**
 * An add-on's source list for one item, as rows to scan rather than cards
 * that all carry the same picture. A pick plays as that item, so the player,
 * history and Continue keep its own name and episode.
 */
function paintSources(page: BrowsePage, target: PlayTarget): void {
  const name = target.showTitle || target.title;
  const line = target.showTitle ? episodeLine(target.season, target.episode, target.title) : "";
  const resume = resumeOf(target.resumeSeconds);
  const rows = page.items.map((item) => {
    const label = (item.label || "Untitled").replace(/\s*\|\s*/g, " · ");
    const run = item.kind === "play"
      ? () => play({ ...target, file: item.path, manualLinks: false })
      : () => void openItem(page.path, item);
    return `<li><button type="button" class="source-row" data-act="${act(run)}">${esc(label)}</button></li>`;
  });
  main!.innerHTML = `
    <p class="muted">Choose a source</p>
    <h1>${esc(name)}</h1>
    ${line ? `<p class="muted">${esc(line)}</p>` : ""}
    <p class="muted">${rows.length} ${rows.length === 1 ? "source" : "sources"} from the add-on. Pick one to play it${resume ? " from " + esc(formatClock(resume)) : ""}.</p>
    ${rows.length ? `<ol class="source-list">${rows.join("")}</ol>` : `<p class="muted">The add-on listed no sources.</p>`}
  `;
}

function browseCard(folder: string, item: BrowseItem, view: ShowView | null = null): string {
  const media = mediaOf(folder, item);
  const season = view && item.kind === "open" ? seasonNumber(item.path) : null;
  const known = media?.type === "movie" ? yearOf(item.info) : null;
  // Add-ons often put the year in the label already.
  const year = known && !item.label.includes(String(known)) ? known : null;
  const mark =
    media?.type === "show"
      ? "Show"
      : item.kind === "open"
        ? "Folder"
        : item.kind === "run"
          ? "Action"
          : year
            ? String(year)
            : "";
  // Add-ons spell episode codes their own way ("(S01E01) : Name", "1x01. Name");
  // cards show the name, with one kind of code under it.
  const code = item.kind === "play" ? episodeCode(item.season, item.episode) : "";
  const label = (code && cleanEpisodeTitle(item.label, item.season, item.episode, item.show_title)) || item.label || "Untitled";
  const run = {
    label: item.kind === "play" ? "Play" : media?.type === "show" ? "Open show" : "Open",
    run: () => void openItem(folder, item),
  };
  const tools = media
    ? `<div class="card-actions" role="group" aria-label="${esc(label)}">${mediaButtons(media, "icon", (node) =>
        openMediaInfo(media, node, run),
      )}</div>`
    : "";
  const progress = view && season != null ? seasonProgress(view, season) : null;
  return `
    <article class="card${progress?.current ? " is-current" : ""}">
      <button type="button" class="card-play" data-act="${act(() => void openItem(folder, item))}">
        ${artBlock(item.art, label)}
        <strong>${esc(label)}</strong>
        ${progress?.note ? `<span class="muted">${esc(progress.note)}</span>` : code || mark ? `<span class="muted">${esc(code || mark)}</span>` : ""}
        ${progress?.fraction ? bar(progress.fraction) : ""}
      </button>
      ${tools}
    </article>
  `;
}

function focusBrowseSearch(): boolean {
  const input = main?.querySelector<HTMLInputElement>("form.search input[name='q']");
  if (!input) return false;
  input.focus();
  input.select();
  return true;
}

/** A link Kodi runs as an add-on, bare or inside a builtin. */
function isPluginLink(path: string): boolean {
  return path.toLowerCase().includes("plugin://");
}

/** A Run row is started the way Kodi starts it: RunPlugin, then whatever dialog opens. */
function openRunItem(item: BrowseItem): void {
  if (item.path && isPluginLink(item.path)) {
    if (isRemote()) {
      setNotice("Add-on tools run only on the PC running Portal.");
      return;
    }
    void runAddonAction(item);
    return;
  }
  setNotice("That item runs an add-on action, not a video.");
}

async function runAddonAction(item: BrowseItem): Promise<void> {
  const ticket = showPage(describeAction(item.label));
  try {
    const result = await api.runPlugin(item.path);
    if (!result.ran) {
      setNotice("That item runs an add-on action, not a video.");
      return;
    }
    setNotice("Opening…");
    const outcome = await awaitAddonPrompts((text) => setNotice(text));
    setNotice(outcome === "idle" ? "The add-on did not open a dialog." : "");
  } catch (err) {
    setNotice(detail(err));
  } finally {
    hideTicket(ticket);
  }
}

async function openItem(folder: string, item: BrowseItem, resume?: number): Promise<void> {
  const settingsId = item.settings_addon || settingsAddon(item.path);
  if (settingsId) {
    location.hash = settingsHref(settingsId);
    return;
  }
  if (!item.path) return;
  if (item.kind === "open") {
    notePathLabel(item.path, item.label);
    const media = mediaOf(folder, item);
    if (media?.type === "show") {
      openedShows.set(item.path, { id: media.save.id, name: media.save.name, art: item.art, info: item.info });
    }
    location.hash = browseHref(item.path);
    return;
  }
  if (item.kind === "run") {
    openRunItem(item);
    return;
  }
  const show = episodeShow(folder, item);
  const info = mediaOf(folder, item)?.seed ?? item.info ?? undefined;
  // A newer click wins over this one while its show name is looked up.
  const mine = ++openSeq;
  if (show) {
    const name = await progressName(show.id, item);
    if (mine !== openSeq) return;
    play({
      kind: "episode",
      id: show.id,
      showId: show.id,
      file: item.path,
      title: item.label,
      showTitle: name,
      season: show.season ?? undefined,
      episode: show.episode,
      art: item.art,
      info,
      resumeSeconds: resume,
      progressKind: "tv",
      progressId: show.id,
      progressName: name,
      trackProgress: true,
    });
    return;
  }
  if (item.title_kind === "tv") {
    play({ ...fileTarget(item), info });
    return;
  }
  play({ ...movieTarget(item.path, item.path, item.label, item.art, resume), info });
}

function fileTarget(item: BrowseItem): PlayTarget {
  return {
    kind: "file",
    id: item.path || item.id,
    file: item.path || item.id,
    title: item.label,
    art: item.art,
    progressKind: "movie",
    progressId: item.path || item.id,
    progressName: item.label,
    trackProgress: false,
  };
}

async function progressName(showId: string, item: BrowseItem): Promise<string> {
  const known = rememberedName(showId);
  if (known && !isSeasonLabel(known)) return known;
  if (item.show_title) return item.show_title;
  try {
    const title = await api.title(showId);
    if (title.name && title.name !== showId && !isSeasonLabel(title.name)) return title.name;
  } catch {
    /* The show is recorded on the next progress post. */
  }
  const label = pathLabel(showId);
  if (label && !isSeasonLabel(label)) return label;
  return folderLabel(showId);
}

async function renderTitle(token: number, id: string): Promise<void> {
  if (!id) {
    main!.innerHTML = `<p class="error">This title is not in Portal yet.</p>`;
    return;
  }
  showPage(describeLibrary("title"), 350);
  try {
  const title = await api.title(id);
  if (token !== renderToken) return;
  const resume = resumeOf(title.resume_seconds);
  const playId = act(() => void playTitle(title, true));
  const resumeId = resume ? act(() => void playTitle(title, false)) : "";
  noteSaved(title.id, { watchlist: title.watchlisted, bookmarks: title.bookmarked });
  const save: SaveTitle = {
    id: title.id,
    kind: title.kind,
    name: title.name,
    art: title.art ?? null,
    year: title.year ?? null,
    imdb_id: title.imdb_id ?? null,
  };
  const watchId = act(() => void toggleSaved("watchlist", save));
  const bookId = act(() => void toggleSaved("bookmarks", save));
  const playable = title.kind === "movie" || !!title.next;
  const infoId = act((node) =>
    openInfo({
      seed: {
        kind: title.kind,
        title: title.name,
        year: title.year ?? null,
        imdb_id: title.imdb_id ?? null,
      },
      art: title.art,
      save,
      opener: node,
      play: playable ? { label: "Play", run: () => playTitle(title, true) } : null,
    }),
  );
  const seasons = title.seasons
    .map((season) => {
      const rows = season.episodes.map((episode) => episodeRow(title, episode)).join("");
      return `<section class="season"><h2>${season.number === 0 ? "Specials" : "Season " + esc(String(season.number))}</h2>${rows}</section>`;
    })
    .join("");
  main!.innerHTML = `
    <article class="hero">
      ${artBlock(title.art, title.name)}
      <div>
        <h1>${esc(title.name)}</h1>
        <p class="muted">${title.kind === "tv" ? "Show" : "Movie"}${title.watched ? " · Watched" : ""}</p>
        <div class="row">
          ${resume ? `<button type="button" data-act="${resumeId}">Resume ${esc(formatClock(resume))}</button>` : ""}
          ${playable ? `<button type="button" data-act="${playId}">Play</button>` : ""}
          ${title.kind === "tv" ? `<button type="button" data-act="${act(() => { location.hash = showHref(title.id, title.next?.season); })}">Episodes</button>` : ""}
          ${savedButtonHtml("watchlist", save, watchId, "pill")}
          ${savedButtonHtml("bookmarks", save, bookId, "pill")}
          ${infoButtonHtml(infoId, title.name, "pill")}
        </div>
      </div>
    </article>
    ${seasons}
  `;
  } finally {
    if (token === renderToken) hidePage();
  }
}

function episodeRow(title: TitleDetail, episode: Episode): string {
  const code = episodeCode(episode.season, episode.episode);
  const label = cleanEpisodeTitle(episode.name, episode.season, episode.episode) || episode.name;
  const resume = episode.resumable ? resumeOf(episode.position) : undefined;
  const play = `<button type="button" data-act="${act(() => playEpisode(title, episode, true))}">${episode.finished ? "Play again" : "Play"}</button>`;
  const resumeButton = resume
    ? `<button type="button" data-act="${act(() => playEpisode(title, episode, false))}">Resume ${esc(formatClock(resume))}</button>`
    : "";
  const fraction = episode.finished ? 1 : episode.duration > 0 ? episode.position / episode.duration : null;
  return `
    <div class="episode has-thumb">
      <span class="episode-thumb">${artBlock(episode.art, label || code)}${fraction ? bar(fraction) : ""}</span>
      <span class="episode-name"><span class="muted">${esc(code)}${episode.finished ? " · Watched" : ""}</span><span>${esc(label)}</span></span>
      <span class="episode-actions">${resumeButton}${play}</span>
    </div>
  `;
}

function playTitle(title: TitleDetail, fromStart: boolean): void {
  if (title.kind === "movie") {
    play(
      movieTarget(
        title.id,
        title.file || title.id,
        title.name,
        title.art,
        fromStart ? undefined : resumeOf(title.resume_seconds),
      ),
    );
    return;
  }
  const next = title.next;
  if (!next) return;
  const match = title.seasons.flatMap((season) => season.episodes).find((episode) => episode.path === next.path);
  playEpisode(
    title,
    match ?? {
      season: next.season,
      episode: next.episode,
      name: next.name,
      path: next.path,
      art: next.art,
      position: 0,
      duration: 0,
      finished: false,
      resumable: false,
    },
    fromStart,
  );
}

function playEpisode(title: TitleDetail, episode: Episode, fromStart = false): void {
  play({
    kind: "episode",
    id: title.id,
    showId: title.id,
    file: episode.path,
    title: episodeLine(episode.season, episode.episode, episode.name) || episode.name,
    showTitle: title.name,
    season: episode.season,
    episode: episode.episode,
    art: episode.art ?? title.art,
    resumeSeconds: fromStart ? undefined : episode.resumable ? resumeOf(episode.position) : undefined,
    progressKind: "tv",
    progressId: title.id,
    progressName: title.name,
    trackProgress: true,
  });
}

function editStays(page: string): boolean {
  if (!editing) return false;
  if (editing.page === "history") return page === "history";
  return page === "home" || page === editing.page;
}

function listTools(page: "watchlist" | "bookmarks", card: Card): string {
  if (editing?.page === page && editing.key === card.id) {
    return renameForm({ kind: page, key: card.id, name: card.name, titleKind: card.kind, art: card.art });
  }
  const edit = act(() => {
    editing = { page, key: card.id };
    void render();
  });
  const remove = act(() => void removeSaved(page, card.id));
  return `<div class="row-actions"><button type="button" class="quiet" data-act="${edit}">Rename</button><button type="button" class="quiet" data-act="${remove}" aria-label="Remove ${esc(card.name)} from ${page === "watchlist" ? "the watchlist" : "bookmarks"}">Remove</button></div>`;
}

function historyTools(entry: HistoryEntry): string {
  if (!Number.isInteger(entry.slot) || entry.slot < 0) return "";
  historyRows.set(String(entry.slot), entry);
  const edit = act(() => {
    editing = { page: "history", key: String(entry.slot) };
    void render();
  });
  const remove = act(() => void removeHistory(entry.slot, entry));
  return `<div class="row-actions"><button type="button" class="quiet" data-act="${edit}">Rename</button><button type="button" class="quiet" data-act="${remove}" aria-label="Remove ${esc(entry.name)} from history">Remove</button></div>`;
}

function renameForm(opts: {
  kind: "watchlist" | "bookmarks" | "history";
  key: string;
  name: string;
  titleKind?: string;
  art?: string | null;
}): string {
  const cancel = act(() => {
    editing = null;
    void renderKeepingRow();
  });
  const titleKind = opts.titleKind ? ` data-title-kind="${esc(opts.titleKind)}"` : "";
  const art = opts.kind === "history" ? "" : ` data-art="${esc(opts.art ?? "")}"`;
  return `
    <form class="rename" data-kind="${esc(opts.kind)}" data-key="${esc(opts.key)}"${titleKind}${art}>
      <input type="text" name="name" value="${esc(opts.name)}" maxlength="200" aria-label="Name" required />
      <button type="submit">Save</button>
      <button type="button" data-act="${cancel}">Cancel</button>
    </form>
  `;
}

function bindRenames(root: ParentNode): void {
  root.querySelectorAll("form.rename").forEach((node) => {
    if (!(node instanceof HTMLFormElement)) return;
    node.addEventListener("submit", (event) => {
      event.preventDefault();
      const value = new FormData(node).get("name");
      const name = typeof value === "string" ? value.trim() : "";
      if (!name) {
        setNotice("Enter a name.");
        node.querySelector("input")?.focus();
        return;
      }
      void saveRename(node.dataset.kind, node.dataset.key ?? "", name, node.dataset.titleKind, node.dataset.art);
    });
    const input = node.querySelector("input");
    input?.focus();
    input?.select();
  });
}

async function saveRename(
  kind: string | undefined,
  key: string,
  name: string,
  titleKind?: string,
  art?: string,
): Promise<void> {
  try {
    if (kind === "history") {
      const slot = Number(key);
      if (!Number.isInteger(slot) || slot < 0) {
        setNotice("That history row is gone.");
        editing = null;
        await render();
        return;
      }
      await api.updateHistory(slot, name, historyRows.get(key));
    } else if (kind === "watchlist" || kind === "bookmarks") {
      if (!key.trim()) {
        setNotice("That item is gone.");
        editing = null;
        await render();
        return;
      }
      await api.putList(kind, key, {
        kind: titleKind === "tv" ? "tv" : "movie",
        name,
        art: art ? art : null,
      });
    } else {
      return;
    }
    editing = null;
    await renderKeepingRow();
  } catch (err) {
    setNotice(detail(err));
  }
}

/**
 * Re-renders a library page and puts keyboard focus back on the row that had it,
 * or the one now in its place, instead of dropping it at the top of the page.
 */
async function renderKeepingRow(): Promise<void> {
  const rows = (): HTMLElement[] =>
    main ? Array.from(main.querySelectorAll<HTMLElement>("article.card, article.history-item")) : [];
  const active = document.activeElement;
  const index = rows().findIndex((row) => row.contains(active));
  await render();
  if (index < 0 || !main) return;
  const now = rows();
  const row = now[Math.min(index, now.length - 1)];
  const target = row?.querySelector<HTMLElement>("button, a") ?? main.querySelector<HTMLElement>("h1");
  if (!target) return;
  if (!row) target.tabIndex = -1;
  target.focus();
}

async function removeSaved(name: "watchlist" | "bookmarks", id: string): Promise<void> {
  try {
    await api.deleteList(name, id);
    // The player's save button for this title shows the change too.
    noteSaved(id, { [name]: false });
    if (editing?.page === name && editing.key === id) editing = null;
    await renderKeepingRow();
  } catch (err) {
    setNotice(detail(err));
  }
}

async function removeHistory(slot: number, row: HistoryRef): Promise<void> {
  try {
    await api.deleteHistory(slot, row);
    if (editing?.page === "history" && editing.key === String(slot)) editing = null;
    await renderKeepingRow();
  } catch (err) {
    setNotice(detail(err));
  }
}

async function renderList(token: number, name: "watchlist" | "bookmarks"): Promise<void> {
  showPage(describeLibrary(name), 350);
  try {
    const body = await api.list(name);
    if (token !== renderToken) return;
    const heading = name === "watchlist" ? "Watchlist" : "Bookmarks";
    const empty =
      name === "watchlist" ? WATCHLIST_EMPTY : BOOKMARKS_EMPTY;
    if (!body.items.length && !browseSource) {
      const source = await api.source().catch(() => null);
      if (token !== renderToken) return;
      if (source?.path) browseSource = { path: source.path, name: source.name || "" };
    }
    const browse = browseSource
      ? `<p><a href="${esc(browseHref(browseSource.path))}">Browse ${esc(browseSource.name || "the add-on")}</a></p>`
      : "";
    const count = body.items.length ? ` <span class="count">${body.items.length}</span>` : "";
    const cards = body.items.length
      ? `<div class="grid">${body.items.map((card) => cardHtml(card, name)).join("")}</div>`
      : `<div class="empty-state"><p class="muted">${empty}</p>${browse}</div>`;
    main!.innerHTML = `<h1>${heading}${count}</h1>${cards}`;
    bindRenames(main!);
  } finally {
    if (token === renderToken) hidePage();
  }
}

async function renderHistory(token: number): Promise<void> {
  showPage(describeLibrary("history"), 350);
  try {
    const body = await api.history();
    if (token !== renderToken) return;
    const clearId = body.items.length && !isRemote()
      ? act(() => {
          if (!window.confirm("Clear all history?")) return;
          void (async () => {
            try {
              editing = null;
              await api.clearHistory();
              await render();
            } catch (err) {
              setNotice(detail(err));
            }
          })();
        })
      : "";
    const rows = body.items.length
      ? body.items.map(historyRow).join("")
      : `<p class="muted">Nothing has been played in Portal yet.</p>`;
    main!.innerHTML = `
      <div class="row">
        <h1>History</h1>
        ${clearId ? `<button type="button" data-act="${clearId}">Clear all</button>` : ""}
      </div>
      ${rows}
    `;
    bindRenames(main!);
  } finally {
    if (token === renderToken) hidePage();
  }
}

function historyRow(entry: HistoryEntry): string {
  const when = entry.at ? new Date(entry.at).toLocaleString(PAGE_LOCALE) : "";
  const code = episodeCode(entry.season, entry.episode);
  const state = entry.finished ? "Watched" : "";
  const resume = !entry.finished ? resumeOf(entry.position) : undefined;
  const resumeButton =
    entry.file && resume
      ? `<button type="button" data-act="${act(() => playHistory(entry, false))}">Resume ${esc(formatClock(resume))}</button>`
      : "";
  const playButton = entry.file
    ? `<button type="button" data-act="${act(() => playHistory(entry, true))}">${entry.finished || resume ? "Play again" : "Play"}</button>`
    : "";
  const key = String(entry.slot);
  const editingThis = editing?.page === "history" && editing.key === key;
  const title = editingThis
    ? renameForm({ kind: "history", key, name: entry.name })
    : `<strong>${esc(entry.name)}</strong>`;
  return `
    <article class="history-item">
      ${artBlock(entry.art, entry.name)}
      <div class="grow">
        ${title}
        <div class="muted">${esc([code, when, state].filter(Boolean).join(" · "))}</div>
      </div>
      ${entry.kind === "tv" ? `<a href="${esc(showHref(entry.id, entry.season))}">Episodes</a>` : `<a href="${esc(titleHref(entry.id))}">Details</a>`}
      ${resumeButton}
      ${playButton}
      ${editingThis ? "" : historyTools(entry)}
    </article>
  `;
}

function playHistory(entry: HistoryEntry, fromStart: boolean): void {
  if (!entry.file) {
    location.hash = entry.kind === "tv" ? showHref(entry.id, entry.season) : titleHref(entry.id);
    return;
  }
  const resume = fromStart || entry.finished ? undefined : resumeOf(entry.position);
  if (entry.kind === "tv" && entry.season != null && entry.episode) {
    play({
      kind: "episode",
      id: entry.id,
      showId: entry.id,
      file: entry.file,
      title: entry.name,
      showTitle: entry.name,
      season: entry.season,
      episode: entry.episode,
      art: entry.art,
      resumeSeconds: resume,
      progressKind: "tv",
      progressId: entry.id,
      progressName: entry.name,
      trackProgress: true,
    });
    return;
  }
  play(movieTarget(entry.id, entry.file, entry.name, entry.art, resume));
}

async function renderSource(token: number): Promise<void> {
  let open = true;
  showPage(describeSource(false));
  noteKodi(token, () => open, describeSource(true));
  try {
  const source = await api.source().catch(() => null);
  if (token !== renderToken) return;
  let addonError = "";
  let repoError = "";
  try {
    const listed = await api.addons();
    if (token !== renderToken) return;
    loadedAddons = listed.items;
  } catch (err) {
    if (token !== renderToken) return;
    loadedAddons = [];
    addonError = detail(err);
  }
  try {
    const listed = await api.repos();
    if (token !== renderToken) return;
    repoCards = listed.items;
  } catch (err) {
    if (token !== renderToken) return;
    repoCards = [];
    repoError = detail(err);
  }
  loadedSource = source;
  if (openRepoId && !repoCards.some((repo) => repo.id === openRepoId)) {
    openRepoId = "";
    openRepoName = "";
    openRepoItems = [];
  }
  paintSource(addonError, repoError);
  } finally {
    open = false;
    if (token === renderToken) hidePage();
  }
  if (openRepoId && !sourceBusy && token === renderToken) void openRepo(openRepoId);
}

function paintSource(addonError: string, repoError: string): void {
  const flash = sourceFlash;
  sourceFlash = "";
  const reopen = openRepoId;
  main!.innerHTML = `
    <h1>Source</h1>
    <p>Add a repository with a link to the repo site or its zip, and an optional name. Then install a video add-on from that repository, and any module it needs. Remove a repository or a loaded video add-on from this PC. Portal uses whichever loaded video add-on you choose.</p>
    <form id="repo-zip" class="search">
      <input id="repo-url" type="url" inputmode="url" autocomplete="off" spellcheck="false" required placeholder="Repo site or zip link" aria-label="Repo site or zip link" />
      <input id="repo-name" type="text" autocomplete="off" spellcheck="false" maxlength="80" placeholder="Name (optional)" aria-label="Name (optional)" />
      <button id="repo-install" type="submit">Install</button>
    </form>
    <p id="repo-note" class="muted" role="status"></p>
    <h2>Repositories</h2>
    <p id="repo-error" class="error"${repoError ? "" : " hidden"}></p>
    <div id="repo-rows"></div>
    <div id="repo-videos"${reopen ? "" : " hidden"}>
      <h2 id="repo-video-title"></h2>
      <div class="search">
        <input id="repo-filter" type="search" placeholder="Filter this repository" autocomplete="off" spellcheck="false" aria-label="Filter this repository" />
      </div>
      <div id="repo-video-rows"></div>
    </div>
    <h2>Loaded video add-ons</h2>
    <p id="source-current" class="muted"></p>
    <p id="loaded-error" class="error"${addonError ? "" : " hidden"}></p>
    <div class="search">
      <input id="source-filter" type="search" placeholder="Filter loaded add-ons" autocomplete="off" spellcheck="false" aria-label="Filter loaded add-ons" />
    </div>
    <div id="loaded-rows"></div>
  `;
  const repoErrorNode = document.getElementById("repo-error");
  if (repoErrorNode) repoErrorNode.textContent = repoError;
  const loadedErrorNode = document.getElementById("loaded-error");
  if (loadedErrorNode) loadedErrorNode.textContent = addonError;
  const current = document.getElementById("source-current");
  if (current) {
    current.textContent = loadedSource?.name
      ? "Current source: " + displayName(loadedSource.name, "Chosen add-on")
      : "No source chosen yet.";
  }
  if (flash) setRepoNote(flash);
  if (clearRepoUrl) {
    const field = document.getElementById("repo-url");
    if (field instanceof HTMLInputElement) field.value = "";
    const nameField = document.getElementById("repo-name");
    if (nameField instanceof HTMLInputElement) nameField.value = "";
    clearRepoUrl = false;
  }
  if (reopen) {
    const title = document.getElementById("repo-video-title");
    if (title) title.textContent = openRepoName || "Repository";
    const rows = document.getElementById("repo-video-rows");
    if (rows) rows.textContent = "Loading…";
  }
  document.getElementById("repo-zip")?.addEventListener("submit", (event) => {
    event.preventDefault();
    const field = document.getElementById("repo-url");
    if (!(field instanceof HTMLInputElement)) return;
    const nameField = document.getElementById("repo-name");
    const name = nameField instanceof HTMLInputElement ? nameField.value : "";
    void submitZip(field.value, name);
  });
  document.getElementById("source-filter")?.addEventListener("input", () => paintLoadedRows());
  document.getElementById("repo-filter")?.addEventListener("input", () => paintRepoVideoRows());
  paintRepoRows();
  paintLoadedRows();
}

function setRepoNote(text: string): void {
  const note = document.getElementById("repo-note");
  if (!note) return;
  note.textContent = text;
}

function addonIdOk(id: string): boolean {
  return /^[A-Za-z0-9._-]{1,80}$/.test(id) && !id.includes("..");
}

function displayName(value: string, fallback: string): string {
  return plainSettingLabel(value) || fallback;
}

function installNote(result: InstallResult): string {
  if (!result.wrote) return "That add-on is already installed.";
  const name = displayName(result.name, "That add-on");
  if (result.applies === "next-start") return name + " is installed. It loads after the next start.";
  if (result.kind === "repository") return name + " is installed. Open it to choose a video add-on.";
  if (result.kind === "video") return name + " is installed. Choose Use when it appears in the list.";
  if (result.kind === "module") return name + " is installed. Video add-ons that use it can now find it.";
  return name + " is installed.";
}

function removeNote(result: RemoveResult): string {
  const name = displayName(result.name, result.kind === "repository" ? "That repository" : "That add-on");
  if (result.applies === "next-start") return name + " is removed. It disappears after the next start.";
  if (result.kind === "repository") return name + " is removed. Add-ons already installed from it stay.";
  return name + " is removed.";
}

function confirmRemove(kind: "repository" | "video", name: string): boolean {
  const label = displayName(name, kind === "repository" ? "this repository" : "this add-on");
  const extra =
    kind === "repository"
      ? "Add-ons already installed from it stay."
      : "Its settings on this PC are deleted.";
  return window.confirm("Remove " + label + "? " + extra);
}

function queryValue(id: string): string {
  const field = document.getElementById(id);
  return field instanceof HTMLInputElement ? field.value.trim().toLowerCase() : "";
}

function matchesQuery(query: string, parts: string[]): boolean {
  if (!query) return true;
  return parts.join(" ").toLowerCase().includes(query);
}

function paintRepoRows(): void {
  const host = document.getElementById("repo-rows");
  if (!host) return;
  if (!repoCards.length) {
    host.innerHTML = `<p class="muted">No repositories are installed.</p>`;
    return;
  }
  const rows = repoCards
    .map((repo) => {
      const open = repo.id === openRepoId;
      const action = open
        ? `<button type="button" data-act="${act(() => closeRepo())}">Close</button>`
        : `<button type="button" data-act="${act(() => void openRepo(repo.id))}">Open</button>`;
      const remove = repo.removable
        ? `<button type="button" data-act="${act(() => void removeRepo(repo.id))}">Remove</button>`
        : "";
      return `
    <article class="addon${open ? " current" : ""}">
      <div class="grow">
        <strong>${esc(displayName(repo.name, "Repository"))}</strong>
        <div class="muted">${esc(repo.id)}</div>
      </div>
      <div class="row-actions">${action}${remove}</div>
    </article>`;
    })
    .join("");
  // Open and Close swap places in the row that was pressed.
  keepFocus(host, () => {
    host.innerHTML = rows;
  });
}

function paintRepoVideoRows(): void {
  const host = document.getElementById("repo-video-rows");
  if (!host || !openRepoId) return;
  const query = queryValue("repo-filter");
  const matching = openRepoItems.filter((item) => matchesQuery(query, [item.name, item.id, item.summary]));
  const videos = matching.filter((item) => (item.kind ?? "video") === "video");
  const modules = matching.filter((item) => item.kind === "module");
  const hasVideos = openRepoItems.some((item) => (item.kind ?? "video") === "video");
  // A repository of modules only shows them straight away.
  const showModules = openRepoModules || !hasVideos || (Boolean(query) && !videos.length);
  if (!videos.length && !modules.length) {
    const message = openRepoItems.length
      ? "No add-ons match."
      : "This repository has no video add-ons.";
    host.innerHTML = `<p class="muted">${message}</p>`;
    return;
  }
  let html = videos.map(repoAddonRow).join("");
  if (!videos.length && hasVideos) html = `<p class="muted">No video add-ons match.</p>`;
  if (modules.length) {
    const count = modules.length + (modules.length === 1 ? " module" : " modules");
    if (showModules) {
      // Hide only what the viewer opened; a filter can hold modules open.
      const fold = hasVideos && openRepoModules
        ? ` <button type="button" data-modules-toggle data-act="${act(() => toggleRepoModules(false))}">Hide</button>`
        : "";
      html += `<h3 class="repo-section">Modules${fold}</h3>
    <p class="muted">Some video add-ons rely on a module from another repository. Install one when its add-on asks for it.</p>`;
      html += modules.map(repoAddonRow).join("");
    } else {
      html += `<p><button type="button" data-modules-toggle data-act="${act(() => toggleRepoModules(true))}">Show ${count}</button></p>`;
    }
  }
  host.innerHTML = html;
}

function toggleRepoModules(show: boolean): void {
  openRepoModules = show;
  paintRepoVideoRows();
  // The pressed button was repainted; keep the keyboard on its successor.
  const next = document.querySelector<HTMLElement>("#repo-video-rows [data-modules-toggle]");
  if (next) next.focus();
  else document.querySelector<HTMLElement>("#repo-video-rows button")?.focus();
}

function repoAddonRow(item: RepoAddon): string {
  const module = item.kind === "module";
  let action: string;
  if (!item.installed) {
    action = `<button type="button" data-act="${act(() => void installListed(openRepoId, item.id))}">Install</button>`;
  } else if (module) {
    action = `<span class="muted">Installed</span>`;
  } else {
    action = `<button type="button" data-act="${act(() => void useInstalled(item))}">Use</button>`;
  }
  const remove = item.removable
    ? `<button type="button" data-act="${act(() => void removeAddon(item.id))}">Remove</button>`
    : "";
  const version = item.version ? " · " + esc(item.version) : "";
  const summary = item.summary ? `<div class="muted">${esc(item.summary)}</div>` : "";
  return `
    <article class="addon">
      <div class="grow">
        <strong>${esc(displayName(item.name, module ? "Module" : "Add-on"))}</strong>
        <div class="muted">${esc(item.id)}${version}</div>
        ${summary}
      </div>
      <div class="row-actions">${action}${remove}</div>
    </article>`;
}

function paintLoadedRows(): void {
  const host = document.getElementById("loaded-rows");
  if (!host) return;
  const query = queryValue("source-filter");
  const items = loadedAddons.filter((addon) => matchesQuery(query, [addon.name, addon.id]));
  if (!loadedAddons.length) {
    host.innerHTML = `<p class="muted">No video add-ons are enabled.</p>`;
    return;
  }
  if (!items.length) {
    host.innerHTML = `<p class="muted">No video add-ons match.</p>`;
    return;
  }
  host.innerHTML = items
    .map((addon) => addonRow(addon, loadedSource?.path === addon.path))
    .join("");
}

function addonRow(addon: AddonCard, current: boolean): string {
  const remove = addon.removable
    ? `<button type="button" data-act="${act(() => void removeAddon(addon.id))}">Remove</button>`
    : "";
  return `
    <article class="addon${current ? " current" : ""}">
      ${artBlock(addon.art, displayName(addon.name, "Add-on"))}
      <div class="grow">
        <strong>${esc(displayName(addon.name, "Add-on"))}</strong>
        <div class="muted">${esc(addon.id)}</div>
      </div>
      <div class="row-actions">
        <a href="${esc(settingsHref(addon.id))}">Settings</a>
        ${remove}
        <button type="button" data-act="${act(() => void chooseSource(addon))}">${current ? "Open" : "Use"}</button>
      </div>
    </article>
  `;
}

async function chooseSource(addon: AddonCard): Promise<void> {
  try {
    await api.setSource({ addon_id: addon.id, name: addon.name, path: addon.path });
    // Pages that name or link the add-on look it up again.
    browseSource = null;
  } catch (err) {
    setNotice(detail(err));
    return;
  }
  notePathLabel(addon.path, addon.name);
  location.hash = browseHref(addon.path);
}

async function submitZip(url: string, name: string): Promise<void> {
  if (sourceBusy) return;
  sourceBusy = true;
  setRepoNote("");
  const button = document.getElementById("repo-install");
  if (button instanceof HTMLButtonElement) button.disabled = true;
  const ticket = showPage(describeRepositoryInstall());
  let handed = false;
  try {
    const result = await api.installZip(url, name);
    sourceFlash = installNote(result);
    clearRepoUrl = true;
    if (result.kind === "repository" && result.wrote) openRepoId = "";
    handed = true;
    await render();
  } catch (err) {
    const message = detail(err);
    setRepoNote(message);
    setNotice(message);
    if (button instanceof HTMLButtonElement) button.disabled = false;
  } finally {
    sourceBusy = false;
    if (!handed) hideTicket(ticket);
    if (openRepoId && route().page === "source") void openRepo(openRepoId);
  }
}

function closeRepo(): void {
  openRepoId = "";
  openRepoName = "";
  openRepoItems = [];
  openRepoModules = false;
  const panel = document.getElementById("repo-videos");
  if (panel) panel.hidden = true;
  const filter = document.getElementById("repo-filter");
  if (filter instanceof HTMLInputElement) filter.value = "";
  const rows = document.getElementById("repo-video-rows");
  if (rows) rows.replaceChildren();
  const title = document.getElementById("repo-video-title");
  if (title) title.textContent = "";
  setRepoNote("");
  paintRepoRows();
}

async function openRepo(id: string): Promise<void> {
  if (sourceBusy) return;
  if (!addonIdOk(id)) {
    setRepoNote("Unknown add-on.");
    return;
  }
  sourceBusy = true;
  if (openRepoId !== id) openRepoModules = false;
  openRepoId = id;
  const known = repoCards.find((repo) => repo.id === id);
  openRepoName = known ? displayName(known.name, "Repository") : "";
  openRepoItems = [];
  const ticket = showPage(describeRepositoryOpen(openRepoName));
  const panel = document.getElementById("repo-videos");
  if (panel) panel.hidden = false;
  const title = document.getElementById("repo-video-title");
  if (title) title.textContent = openRepoName || "Repository";
  const rows = document.getElementById("repo-video-rows");
  if (rows) rows.textContent = "Loading…";
  paintRepoRows();
  try {
    const listed = await api.repo(id);
    if (route().page !== "source" || openRepoId !== id) return;
    openRepoName = displayName(listed.name, openRepoName || "Repository");
    openRepoItems = listed.items;
    if (title) title.textContent = openRepoName;
    paintRepoVideoRows();
  } catch (err) {
    if (route().page !== "source" || openRepoId !== id) return;
    openRepoItems = [];
    // The page may have been drawn again while this waited.
    const current = document.getElementById("repo-video-rows");
    if (current) current.innerHTML = `<p class="error">${esc(detail(err))}</p>`;
  } finally {
    sourceBusy = false;
    hideTicket(ticket);
  }
}

async function installListed(repoId: string, addonId: string): Promise<void> {
  if (sourceBusy) return;
  if (!addonIdOk(repoId) || !addonIdOk(addonId)) {
    setRepoNote("Unknown add-on.");
    return;
  }
  sourceBusy = true;
  setRepoNote("");
  const named = openRepoItems.find((item) => item.id === addonId);
  const ticket = showPage(describeAddonInstall(named ? displayName(named.name, "") : ""));
  let handed = false;
  try {
    const result = await api.installAddon(repoId, addonId);
    sourceFlash = installNote(result);
    openRepoId = repoId;
    handed = true;
    await render();
  } catch (err) {
    setRepoNote(detail(err));
  } finally {
    sourceBusy = false;
    if (!handed) hideTicket(ticket);
    if (openRepoId === repoId && route().page === "source") void openRepo(repoId);
  }
}

async function removeRepo(id: string): Promise<void> {
  if (sourceBusy) return;
  if (!addonIdOk(id)) {
    setRepoNote("Unknown add-on.");
    return;
  }
  const known = repoCards.find((repo) => repo.id === id);
  if (!known?.removable) return;
  const name = displayName(known.name, "Repository");
  if (!confirmRemove("repository", name)) return;
  sourceBusy = true;
  setRepoNote("");
  const ticket = showPage(describeRemove("repository", name));
  let handed = false;
  try {
    const result = await api.removeRepo(id);
    if (openRepoId === id) {
      openRepoId = "";
      openRepoName = "";
      openRepoItems = [];
    }
    sourceFlash = removeNote(result);
    handed = true;
    await render();
  } catch (err) {
    setRepoNote(detail(err));
  } finally {
    sourceBusy = false;
    if (!handed) hideTicket(ticket);
  }
}

async function removeAddon(id: string): Promise<void> {
  if (sourceBusy) return;
  if (!addonIdOk(id)) {
    setRepoNote("Unknown add-on.");
    return;
  }
  const loaded = loadedAddons.find((addon) => addon.id === id);
  const listed = openRepoItems.find((item) => item.id === id);
  if (loaded && !loaded.removable) return;
  if (!loaded && listed && !listed.removable) return;
  const name = displayName(loaded?.name || listed?.name || "", "Add-on");
  if (!confirmRemove("video", name)) return;
  sourceBusy = true;
  setRepoNote("");
  const ticket = showPage(describeRemove("video", name));
  let handed = false;
  try {
    const result = await api.removeAddon(id);
    if (loadedSource?.addon_id === id || loadedSource?.path === "plugin://" + id + "/") {
      browseSource = null;
    }
    sourceFlash = removeNote(result);
    openRepoId = openRepoId || "";
    handed = true;
    await render();
  } catch (err) {
    setRepoNote(detail(err));
  } finally {
    sourceBusy = false;
    if (!handed) hideTicket(ticket);
    if (openRepoId && route().page === "source") void openRepo(openRepoId);
  }
}

async function useInstalled(item: RepoAddon): Promise<void> {
  if (!addonIdOk(item.id)) {
    setRepoNote("Unknown add-on.");
    return;
  }
  await chooseSource({
    id: item.id,
    name: item.name,
    path: "plugin://" + item.id + "/",
    art: null,
  });
}

async function renderSettings(token: number, id: string): Promise<void> {
  if (!id) {
    main!.innerHTML = `<p class="error">Choose a video add-on first.</p>`;
    return;
  }
  let open = true;
  showPage(describeSettings(false));
  noteKodi(token, () => open, describeSettings(true));
  // Opened from a link rather than the Source page: fetch the add-on's name too.
  const names = loadedAddons.length ? null : api.addons().catch(() => null);
  try {
    const doc = await api.settings(id);
    const listed = await names;
    open = false;
    if (token !== renderToken) return;
    if (listed && !loadedAddons.length) loadedAddons = listed.items;
    if (categoryAddon !== doc.addon_id) {
      categoryAddon = doc.addon_id;
      categoryIndex = 0;
    }
    if (categoryIndex >= doc.categories.length) categoryIndex = 0;
    paintSettings(doc);
  } finally {
    if (token === renderToken) hidePage();
  }
}

function paintSettings(doc: SettingsDoc): void {
  const cats = doc.categories
    .map((category, index) => {
      const current = index === categoryIndex ? ` aria-current="true"` : "";
      return `<button type="button" data-act="${act(() => {
        categoryIndex = index;
        paintSettings(doc);
        // The repaint replaced the button that had focus.
        main!.querySelector<HTMLElement>(".cats button[aria-current]")?.focus();
      })}"${current}>${esc(plainSettingLabel(category.label) || "Settings")}</button>`;
    })
    .join("");
  const category = doc.categories[categoryIndex];
  const fields = category ? category.fields.map((field) => fieldHtml(doc.addon_id, field)).join("") : `<p class="muted">No settings in this category.</p>`;
  const name = plainSettingLabel(loadedAddons.find((addon) => addon.id === doc.addon_id)?.name ?? "");
  main!.innerHTML = `
    <div class="row"><h1>Settings</h1><a href="#/source">Source</a></div>
    <p class="muted">${esc(name || doc.addon_id)}</p>
    <div class="cats">${cats}</div>
    ${fields}
  `;
  wireSettings(doc);
}

function fieldHtml(addonId: string, field: SettingsField): string {
  const label = plainSettingLabel(field.label);
  if (field.kind === "header") return `<h3>${esc(label)}</h3>`;
  const id = field.id ?? "";
  // Ties each label to its control, for screen readers and for clicking the label.
  const fid = "setting-" + id.replace(/[^A-Za-z0-9_-]/g, "_");
  const disabled = field.enabled ? "" : " disabled";
  let control = "";
  if (field.kind === "bool") {
    const checked = field.value === "true" ? " checked" : "";
    control = `<label class="check"><input type="checkbox" data-setting="${esc(id)}" data-kind="bool"${checked}${disabled} /> ${esc(label)}</label>`;
  } else if (field.kind === "select") {
    const options = (field.options ?? [])
      .map((option) => {
        const selected = option.value === field.value ? " selected" : "";
        return `<option value="${esc(option.value)}"${selected}>${esc(plainSettingLabel(option.label))}</option>`;
      })
      .join("");
    control = `<label for="${fid}">${esc(label)}</label><select id="${fid}" data-setting="${esc(id)}" data-kind="select"${disabled}>${options}</select>`;
  } else if (field.kind === "action") {
    const settingsId = settingsAddon(field.action ?? "");
    if (settingsId) {
      control = `<a class="action-link" href="${esc(settingsHref(settingsId))}">${esc(label)}</a>`;
    } else if (!id && field.action) {
      control = `<button type="button" data-run="${esc(field.action)}" data-kind="action"${disabled}>${esc(label)}</button>`;
    } else {
      // What the action last stored, as Kodi shows beside the button.
      const shown = plainSettingLabel(field.value);
      const hidden = shown ? "" : " hidden";
      control = `<button type="button" data-setting="${esc(id)}" data-kind="action"${disabled}>${esc(label)}</button><span class="setting-value" data-value-for="${esc(id)}"${hidden}>${esc(shown)}</span>`;
    }
  } else if (field.kind === "secret") {
    control = `<label for="${fid}">${esc(label)}</label><input id="${fid}" type="password" data-setting="${esc(id)}" data-kind="secret" autocomplete="off" placeholder="Leave blank to keep it"${disabled} />`;
  } else if (field.kind === "number") {
    control = `<label for="${fid}">${esc(label)}</label><input id="${fid}" type="number" data-setting="${esc(id)}" data-kind="number" value="${esc(field.value)}"${disabled} />`;
  } else {
    control = `<label for="${fid}">${esc(label)}</label><input id="${fid}" type="text" data-setting="${esc(id)}" data-kind="text" value="${esc(field.value)}"${disabled} />`;
  }
  return `<div class="setting" data-addon="${esc(addonId)}">${control}<div class="note" role="status"></div></div>`;
}

function wireSettings(doc: SettingsDoc): void {
  const addonId = doc.addon_id;
  main!.querySelectorAll<HTMLButtonElement>("[data-run]").forEach((node) => {
    const target = node.dataset.run ?? "";
    const note = node.closest(".setting")?.querySelector<HTMLElement>(".note") ?? null;
    node.addEventListener("click", () => {
      if (node.disabled || !target) return;
      node.disabled = true;
      void runAddonAction({ id: target, label: node.textContent || "", path: target, kind: "run" }).finally(() => {
        if (node.isConnected) node.disabled = false;
        void refreshSettings(doc);
      });
      if (note) note.textContent = "";
    });
  });
  main!.querySelectorAll<HTMLElement>("[data-setting]").forEach((node) => {
    const settingId = node.dataset.setting ?? "";
    const kind = node.dataset.kind ?? "";
    const note = node.closest(".setting")?.querySelector<HTMLElement>(".note") ?? null;
    if (kind === "action" && node instanceof HTMLButtonElement) {
      node.addEventListener("click", () => {
        // An add-on action can take a while; a second click would run it twice.
        if (node.disabled) return;
        node.disabled = true;
        void runAction(addonId, settingId, note, node.textContent || "").finally(() => {
          node.disabled = false;
          void refreshSettings(doc);
        });
      });
      return;
    }
    node.addEventListener("change", () => {
      if (!(node instanceof HTMLInputElement || node instanceof HTMLSelectElement)) return;
      if (kind === "secret" && !node.value) return;
      const value = node instanceof HTMLInputElement && node.type === "checkbox" ? (node.checked ? "true" : "false") : node.value;
      // Categories repaint from the loaded settings, so they carry the new value
      // (a saved secret is never shown, so it is not kept).
      const before = kind === "secret" ? null : noteSetting(doc, settingId, value);
      void saveSetting(addonId, settingId, value, note).then((saved) => {
        // A later change that already replaced this value keeps its own.
        if (saved || before === null) return;
        noteSetting(doc, settingId, before, value);
        if (!node.isConnected) return;
        const showing =
          node instanceof HTMLInputElement && node.type === "checkbox" ? (node.checked ? "true" : "false") : node.value;
        if (showing !== value) return;
        if (node instanceof HTMLInputElement && node.type === "checkbox") node.checked = before === "true";
        else node.value = before;
      });
    });
  });
}

/**
 * An action is the add-on changing its own settings, so the form is read
 * again and shows what it stored. A field being typed in is left alone.
 */
async function refreshSettings(doc: SettingsDoc): Promise<void> {
  let fresh: SettingsDoc;
  try {
    fresh = await api.settings(doc.addon_id);
  } catch {
    return;
  }
  for (const category of fresh.categories) {
    for (const field of category.fields) {
      if (!field.id || field.kind === "secret" || field.kind === "header") continue;
      noteSetting(doc, field.id, field.value);
      paintSetting(doc.addon_id, field.id, field.value);
    }
  }
}

function paintSetting(addonId: string, settingId: string, value: string): void {
  main?.querySelectorAll<HTMLElement>(".setting").forEach((row) => {
    if (row.dataset.addon !== addonId) return;
    row.querySelectorAll<HTMLElement>("[data-value-for], [data-setting]").forEach((node) => {
      if (node.dataset.valueFor === settingId) {
        const shown = plainSettingLabel(value);
        node.textContent = shown;
        node.hidden = !shown;
        return;
      }
      if (node.dataset.setting !== settingId || node === document.activeElement) return;
      if (node instanceof HTMLInputElement && node.type === "checkbox") {
        node.checked = value === "true";
      } else if (node instanceof HTMLInputElement || node instanceof HTMLSelectElement) {
        node.value = value;
      }
    });
  });
}

/** Sets a field's value in the loaded settings (only over `from`, when given); answers the value it had. */
function noteSetting(doc: SettingsDoc, settingId: string, value: string, from?: string): string | null {
  let before: string | null = null;
  for (const category of doc.categories) {
    for (const field of category.fields) {
      if (field.id !== settingId || (from !== undefined && field.value !== from)) continue;
      if (before === null) before = field.value;
      field.value = value;
    }
  }
  return before;
}

async function saveSetting(addonId: string, settingId: string, value: string, note: HTMLElement | null): Promise<boolean> {
  if (note) note.textContent = "Saving…";
  try {
    await api.writeSetting(addonId, settingId, value);
    if (note) note.textContent = "Saved";
    return true;
  } catch (err) {
    if (note) note.textContent = detail(err);
    return false;
  }
}

async function runAction(addonId: string, settingId: string, note: HTMLElement | null, name = ""): Promise<void> {
  const ticket = showPage(describeAction(name));
  let handed = false;
  const here = location.hash;
  try {
    const result = await api.action(addonId, settingId);
    if (result.settings_addon) {
      setNotice("");
      handed = true;
      location.hash = settingsHref(result.settings_addon);
      return;
    }
    if (result.browse_path) {
      handed = true;
      location.hash = browseHref(result.browse_path);
      return;
    }
    if (result.ran) {
      if (note) note.textContent = "Opening…";
      const outcome = await awaitAddonPrompts((text, prompt) => {
        if (note) paintActionNote(note, text, prompt);
      });
      // Settings for another add-on opened: that page takes over. The same
      // add-on's settings are this page, which stays.
      if (outcome === "settings" && location.hash !== here) {
        handed = true;
        return;
      }
      if (note) {
        note.textContent =
          outcome === "idle" ? "The add-on did not open a dialog." : "Done";
      }
      return;
    }
    if (note) note.textContent = "Done";
  } catch (err) {
    if (note) note.textContent = detail(err);
  } finally {
    if (!handed) hideTicket(ticket);
  }
}

/**
 * What the add-on's dialog says, as Kodi shows it. A sign-in dialog names a
 * web address and a code, so https addresses become links, and a dialog
 * Kodi lets the user cancel gets a Cancel button.
 */
function paintActionNote(note: HTMLElement, text: string, prompt: Prompt | null): void {
  note.replaceChildren();
  const parts = text.split(/(https:\/\/[^\s<>"']+)/);
  parts.forEach((part, index) => {
    if (index % 2 === 1) {
      const link = document.createElement("a");
      link.href = part.replace(/[.,;:!?)]+$/, "");
      link.textContent = part;
      link.target = "_blank";
      link.rel = "noopener noreferrer";
      note.append(link);
    } else if (part) {
      note.append(document.createTextNode(part));
    }
  });
  if (prompt?.kind === "progress" && prompt.cancellable) {
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    cancel.addEventListener("click", () => {
      cancel.disabled = true;
      void api.answerPrompt({ id: prompt.id, action: "cancel" }).catch((err) => {
        note.textContent = detail(err);
      });
    });
    note.append(document.createElement("br"), cancel);
  }
}

type ServerCast = {
  lan: boolean;
  url: string;
  restartNeeded: boolean;
};

let serverCast: ServerCast | null = null;
let serverMeta: MetadataSettings | null = null;
let serverMetaError = "";
let serverMetaNote = "";
let metaBusy = false;
let serverCastError = "";
let castLanBusy = false;

function castPageUrl(url: string): string {
  const trimmed = url.trim();
  if (!/^https?:\/\//i.test(trimmed)) return "";
  if (/#\/cast\/?$/.test(trimmed)) return trimmed;
  return trimmed.replace(/\/+$/, "") + "/#/cast";
}

function restartFlag(row: Record<string, unknown>): boolean | null {
  if (!("restart_needed" in row)) return null;
  return row.restart_needed === true;
}

function castUrl(row: Record<string, unknown>, fallback: string): string {
  if (!("url" in row)) return fallback;
  return typeof row.url === "string" ? row.url : "";
}

function parseCast(body: unknown, fallback: ServerCast | null, requestedLan?: boolean): ServerCast {
  const row = body && typeof body === "object" ? (body as Record<string, unknown>) : {};
  const nested = row.cast && typeof row.cast === "object" ? (row.cast as Record<string, unknown>) : row;
  const lan = typeof nested.lan === "boolean" ? nested.lan : requestedLan ?? fallback?.lan ?? false;
  const url = castUrl(nested, castUrl(row, fallback?.url ?? ""));
  const restartNeeded = restartFlag(nested) ?? restartFlag(row) ?? fallback?.restartNeeded ?? false;
  return { lan, url, restartNeeded };
}

async function refreshServerCast(): Promise<void> {
  try {
    serverCast = parseCast(await api.castInfo(), serverCast);
    serverCastError = "";
  } catch (err) {
    serverCastError = detail(err);
  }
}

async function saveCastLan(lan: boolean): Promise<void> {
  serverCast = parseCast(await api.setCastLan(lan), serverCast, lan);
  serverCastError = "";
}

function castSection(status: Status): string {
  const on = serverCast?.lan === true;
  const href = castPageUrl(serverCast?.url ?? "");
  const link = href
    ? `<p class="cast-url">Open this address on a TV: <a href="${esc(href)}">${esc(href)}</a></p>`
    : on
      ? `<p class="muted cast-url">The cast link appears when Portal reports an address.</p>`
      : "";
  const restart = serverCast?.restartNeeded
    ? `<div class="row cast-restart"><span class="muted">${esc(CAST_RESTART_TEXT)}</span><button type="button" data-act="${act(() => void restartForCast(status))}"${castLanBusy ? " disabled" : ""}>Restart Portal</button></div>`
    : "";
  const error = serverCastError ? `<p class="error">${esc(serverCastError)}</p>` : "";
  return `
    <section class="server-card" id="server-cast">
      <h2>Cast</h2>
      <div class="row">
        <span id="server-cast-label">Allow TVs on this network</span>
        <button type="button" role="switch" aria-checked="${on ? "true" : "false"}" aria-labelledby="server-cast-label" data-act="${act(() => void toggleCastLan(status))}">${on ? "On" : "Off"}</button>
      </div>
      ${link}
      ${restart}
      ${error}
    </section>
  `;
}

const CAST_RESTART_TEXT = "Portal needs to restart before this takes effect.";

/** Restarts Portal so the TV setting applies, as the Portal menu does. */
async function restartForCast(status: Status): Promise<void> {
  if (castLanBusy || !window.confirm(LAN_TEXT)) return;
  castLanBusy = true;
  setNotice("Restarting Portal…");
  try {
    await restartPortal();
  } catch (err) {
    setNotice("");
    serverCastError = detail(err);
  } finally {
    castLanBusy = false;
  }
  // Only reached when the restart failed: a restart reloads the page.
  keepFocus(main!, () => paintServer(status, ""));
}

async function toggleCastLan(status: Status): Promise<void> {
  if (castLanBusy) return;
  const nextLan = serverCast?.lan !== true;
  const token = renderToken;
  castLanBusy = true;
  try {
    await saveCastLan(nextLan);
    if (token !== renderToken) return;
    setNotice("");
    keepFocus(main!, () => paintServer(status, ""));
  } catch (err) {
    serverCastError = detail(err);
    if (token !== renderToken) return;
    keepFocus(main!, () => paintServer(status, ""));
  } finally {
    castLanBusy = false;
  }
}

async function renderServer(token: number): Promise<void> {
  const [status] = await Promise.all([api.status(), refreshUpdate()]);
  if (token !== renderToken) return;
  await Promise.all([refreshServerCast(), refreshMeta(), refreshSubs(), refreshDevices()]);
  if (token !== renderToken) return;
  paintServer(status, "");
}

function paintServer(status: Status, message: string): void {
  const lines = [
    ["State", status.state.charAt(0).toUpperCase() + status.state.slice(1)],
    ["Version", status.version || ""],
    ["Process ID", status.pid ? String(status.pid) : ""],
    ["Started by Portal", status.started_by_us ? "Yes" : "No"],
    ["Window", status.window_visible ? "Shown" : "Hidden"],
    ["Web server", status.webserver ? "On" : "Off"],
  ];
  const facts = lines.map(([label, value]) => `<div class="row fact"><span class="muted">${esc(label)}</span><span>${esc(value)}</span></div>`).join("");
  const error = status.error || message;
  main!.innerHTML = `
    <h1>Server</h1>
    <section class="server-card" id="server-engine">
      <h2>Kodi engine</h2>
      ${facts}
      ${error ? `<p class="error">${esc(error)}</p>` : ""}
      <div class="row">
        <button type="button" data-act="${act(() => void serverCall("window", status))}">${status.window_visible ? "Hide Kodi window" : "Show Kodi window"}</button>
        <button type="button" data-act="${act(() => void serverCall("restart", status))}">Restart Kodi engine</button>
        <button type="button" data-act="${act(() => void serverCall("quit", status))}">Close Kodi engine</button>
      </div>
    </section>
    ${castSection(status)}
    ${devicesSection()}
    ${mediaSection()}
    ${subsSection()}
    ${updateSection()}
  `;
  wireMedia();
  wireSubs();
  wireDevices();
}

async function refreshDevices(): Promise<void> {
  if (!pairInfo?.this_pc) return;
  try {
    serverDevices = await api.devices();
    serverDevicesError = "";
  } catch (err) {
    serverDevicesError = detail(err);
  }
}

/** Pairing other devices happens on this PC only, so other screens skip it. */
function devicesSection(): string {
  if (!pairInfo?.this_pc) return "";
  const busy = pairBusy ? " disabled" : "";
  const rows = serverDevices
    .map((device) => {
      const seen = device.last_seen ? new Date(device.last_seen).toLocaleString(PAGE_LOCALE) : "";
      const facts = [device.address, seen ? `last seen ${seen}` : ""].filter(Boolean).join(" · ");
      return `<div class="row"><span>${esc(device.name)} <span class="muted">${esc(facts)}</span></span><button type="button" data-act="${act(() => void removeDevice(device))}"${busy}>Remove</button></div>`;
    })
    .join("");
  return `
    <section class="server-card" id="server-devices">
      <h2>Devices</h2>
      <p class="muted">A phone, tablet or TV that opens Portal shows a number. Type it here to let that device in.</p>
      <form id="pair-form" class="media-key">
        <label for="pair-code">Number shown on the device</label>
        <div class="search">
          <input id="pair-code" inputmode="numeric" autocomplete="off" spellcheck="false" maxlength="7" placeholder="123 456" />
          <button type="submit"${busy}>Pair</button>
        </div>
        ${serverDevicesNote ? `<p class="muted" role="status">${esc(serverDevicesNote)}</p>` : ""}
        ${serverDevicesError ? `<p class="error" role="alert">${esc(serverDevicesError)}</p>` : ""}
      </form>
      ${rows || `<p class="muted">No devices are paired.</p>`}
    </section>
  `;
}

function wireDevices(): void {
  const form = document.getElementById("pair-form");
  if (!(form instanceof HTMLFormElement)) return;
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const field = document.getElementById("pair-code");
    const code = field instanceof HTMLInputElement ? field.value.replace(/\D/g, "") : "";
    if (code.length !== 6) {
      serverDevicesError = "Type the 6-digit number the device shows.";
      serverDevicesNote = "";
      repaintDevices("pair-code");
      return;
    }
    void pairDevice(code);
  });
}

function repaintDevices(focusId = ""): void {
  const node = document.getElementById("server-devices");
  if (!node) return;
  node.outerHTML = devicesSection();
  wireDevices();
  if (focusId) document.getElementById(focusId)?.focus();
}

async function pairDevice(code: string): Promise<void> {
  pairBusy = true;
  repaintDevices();
  try {
    const result = await api.pairApprove(code);
    serverDevicesNote = `Paired: ${result.name}. It opens Portal in a moment.`;
    serverDevicesError = "";
    await refreshDevices();
  } catch (err) {
    serverDevicesError = detail(err);
    serverDevicesNote = "";
  } finally {
    pairBusy = false;
    repaintDevices("pair-code");
  }
}

async function removeDevice(device: PairedDevice): Promise<void> {
  pairBusy = true;
  repaintDevices();
  try {
    await api.removeDevice(device.id);
    serverDevicesNote = `Removed ${device.name}. It needs a new number to come back.`;
    serverDevicesError = "";
    await refreshDevices();
  } catch (err) {
    serverDevicesError = detail(err);
  } finally {
    pairBusy = false;
    repaintDevices();
  }
}

async function refreshMeta(): Promise<void> {
  try {
    serverMeta = await api.metadata();
    serverMetaError = "";
  } catch (err) {
    serverMeta = null;
    serverMetaError = detail(err);
  }
  serverMetaNote = "";
}

function mediaSection(): string {
  const meta = serverMeta;
  if (!meta) {
    return `
    <section class="server-card" id="server-media">
      <h2>Media info</h2>
      <p class="muted">Media info settings are not available.</p>
      ${serverMetaError ? `<p class="error">${esc(serverMetaError)}</p>` : ""}
    </section>
  `;
  }
  const on = meta.online;
  const busy = metaBusy ? " disabled" : "";
  const remove = meta.tmdb
    ? `<button type="button" data-act="${act(() => void saveTmdb(null))}"${busy}>Remove</button>`
    : "";
  const state = meta.tmdb ? "Key saved" : "No key saved";
  return `
    <section class="server-card" id="server-media">
      <h2>Media info</h2>
      <div class="row">
        <span id="server-online-label">Look up extra info online (TVmaze, Wikipedia)</span>
        <button type="button" id="server-online" role="switch" aria-checked="${on ? "true" : "false"}" aria-labelledby="server-online-label" data-act="${act(() => void toggleOnline())}"${busy}>${on ? "On" : "Off"}</button>
      </div>
      <p class="muted">Looking up info sends the title and year of what you open to TVmaze and Wikipedia, and to TMDB when a key is saved.</p>
      <form id="tmdb-form" class="media-key">
        <label for="tmdb-key">TMDB API key (optional, for richer info)</label>
        <div class="search">
          <input id="tmdb-key" type="password" autocomplete="off" spellcheck="false" placeholder="${meta.tmdb ? "Paste a new key" : "Paste a TMDB API key"}" aria-describedby="tmdb-state tmdb-hint${serverMetaError ? " tmdb-error" : ""}" />
          <button type="submit"${busy}>Save</button>
          ${remove}
        </div>
        <p id="tmdb-state" class="${meta.tmdb ? "key-saved" : "muted"}">${esc(state)}</p>
        <p id="tmdb-hint" class="muted">A free key comes from your account on themoviedb.org → Settings → API.</p>
      </form>
      ${serverMetaNote ? `<p class="muted">${esc(serverMetaNote)}</p>` : ""}
      ${serverMetaError ? `<p class="error" id="tmdb-error">${esc(serverMetaError)}</p>` : ""}
    </section>
  `;
}

function wireMedia(): void {
  const form = document.getElementById("tmdb-form");
  if (!(form instanceof HTMLFormElement)) return;
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const field = document.getElementById("tmdb-key");
    const value = field instanceof HTMLInputElement ? field.value.trim() : "";
    if (!value) {
      serverMetaError = "Enter a TMDB API key.";
      serverMetaNote = "";
      repaintMedia("tmdb-key");
      return;
    }
    void saveTmdb(value);
  });
}

function repaintMedia(focusId = "", keepKey = false): void {
  const node = document.getElementById("server-media");
  if (!node) return;
  const old = document.getElementById("tmdb-key");
  const typed = keepKey && old instanceof HTMLInputElement ? old.value : "";
  node.outerHTML = mediaSection();
  wireMedia();
  const field = document.getElementById("tmdb-key");
  if (typed && field instanceof HTMLInputElement) field.value = typed;
  if (focusId) document.getElementById(focusId)?.focus();
}

async function toggleOnline(): Promise<void> {
  if (metaBusy || !serverMeta) return;
  const token = renderToken;
  metaBusy = true;
  try {
    serverMeta = await api.setMetadata({ online: !serverMeta.online });
    serverMetaError = "";
    serverMetaNote = "";
  } catch (err) {
    serverMetaError = detail(err);
  } finally {
    metaBusy = false;
  }
  if (token === renderToken) repaintMedia("server-online");
}

async function saveTmdb(key: string | null): Promise<void> {
  if (metaBusy) return;
  const token = renderToken;
  metaBusy = true;
  let focus = "";
  try {
    serverMeta = await api.setMetadata({ tmdb_key: key });
    serverMetaError = "";
    serverMetaNote = key ? "TMDB key saved." : "TMDB key removed.";
  } catch (err) {
    serverMetaError = detail(err);
    serverMetaNote = "";
    focus = "tmdb-key";
  } finally {
    metaBusy = false;
  }
  if (token === renderToken) repaintMedia(focus, !!focus);
}

let serverSubs: SubtitleSettings | null = null;
let serverSubsError = "";
let serverSubsNote = "";
let subsBusy = false;

async function refreshSubs(): Promise<void> {
  try {
    serverSubs = cleanSubtitleSettings(await api.subtitleSettings());
    serverSubsError = "";
  } catch (err) {
    serverSubs = null;
    serverSubsError = detail(err);
  }
  serverSubsNote = "";
}

function languageName(code: string): string {
  return COMMON_LANGUAGES.find(([value]) => value === code)?.[1] ?? code;
}

const MAX_SUB_LANGUAGES = 4;

/** The other subtitle services that work with the user's own free API key. */
type SubKeySource = { id: "subdl" | "subsource"; name: string; hint: string };

const SUB_KEY_SOURCES: SubKeySource[] = [
  { id: "subdl", name: "SubDL", hint: "A free key comes from your account on subdl.com → API." },
  { id: "subsource", name: "SubSource", hint: "A free key comes from your profile on subsource.net → API key." },
];

function subKeyPut(source: SubKeySource, value: string | null): SubtitleSettingsPut {
  return source.id === "subdl" ? { subdl_key: value } : { subsource_key: value };
}

/** "A and B", or "A, B and C". */
function spokenList(items: string[]): string {
  if (items.length < 3) return items.join(" and ");
  return items.slice(0, -1).join(", ") + " and " + items[items.length - 1];
}

function subKeyForm(source: SubKeySource, saved: boolean, busy: string): string {
  const field = `subs-${source.id}-key`;
  const remove = saved
    ? `<button type="button" data-act="${act(() => void saveSubs(subKeyPut(source, null), source.name + " key removed."))}"${busy}>Remove</button>`
    : "";
  return `
      <form id="${field}-form" class="media-key">
        <label for="${field}">${esc(source.name)} API key (optional, adds ${esc(source.name)} results)</label>
        <div class="search">
          <input id="${field}" type="password" autocomplete="off" spellcheck="false" placeholder="${saved ? "Paste a new key" : "Paste a " + esc(source.name) + " API key"}" aria-describedby="${field}-state ${field}-hint${serverSubsError ? " subs-error" : ""}" />
          <button type="submit"${busy}>Save</button>
          ${remove}
        </div>
        <p id="${field}-state" class="${saved ? "key-saved" : "muted"}">${saved ? "Key saved" : "No key saved"}</p>
        <p id="${field}-hint" class="muted">${esc(source.hint)}</p>
      </form>`;
}

function subsSection(): string {
  const subs = serverSubs;
  if (!subs) {
    return `
    <section class="server-card" id="server-subs">
      <h2>Subtitles</h2>
      <p class="muted">Subtitle settings are not available.</p>
      ${serverSubsError ? `<p class="error">${esc(serverSubsError)}</p>` : ""}
    </section>
  `;
  }
  const busy = subsBusy ? " disabled" : "";
  const saved = subs.languages.length > 0;
  const languages = saved ? subs.languages : browserLanguages();
  const chips = languages
    .map((code, index) => {
      const name = languageName(code);
      const remove = `<button type="button" class="chip-remove" aria-label="Remove ${esc(name)}" data-act="${act(() => {
        const current = subs.languages.length ? subs.languages : browserLanguages();
        void saveSubs({ languages: current.filter((_, at) => at !== index) }, name + " removed.");
      })}"${busy}>×</button>`;
      return `<li class="lang-chip"><span>${esc(name)}</span>${remove}</li>`;
    })
    .join("");
  const addable = COMMON_LANGUAGES.filter(([code]) => !languages.includes(code));
  const full = languages.length >= MAX_SUB_LANGUAGES;
  const options = addable.map(([code, name]) => `<option value="${esc(code)}">${esc(name)}</option>`).join("");
  const hearing = subs.hearing_impaired;
  const keyState = subs.opensubtitles ? "Key saved" : "No key saved";
  const userState = subs.opensubtitles_user ? "Signed in" : "Not signed in";
  const removeKey = subs.opensubtitles
    ? `<button type="button" data-act="${act(() => void saveSubs({ opensubtitles_key: null, opensubtitles_username: null, opensubtitles_password: null }, "OpenSubtitles key removed."))}"${busy}>Remove</button>`
    : "";
  const removeUser = subs.opensubtitles_user
    ? `<button type="button" data-act="${act(() => void saveSubs({ opensubtitles_username: null, opensubtitles_password: null }, "Signed out of OpenSubtitles."))}"${busy}>Sign out</button>`
    : "";
  const login = subs.opensubtitles
    ? `
      <form id="subs-login-form" class="media-key">
        <label for="subs-user">OpenSubtitles account (optional, for more downloads a day)</label>
        <div class="search">
          <input id="subs-user" type="text" autocomplete="username" spellcheck="false" placeholder="Username" aria-describedby="subs-user-state" />
          <input id="subs-pass" type="password" autocomplete="current-password" placeholder="Password" aria-label="OpenSubtitles password" />
          <button type="submit"${busy}>Sign in</button>
          ${removeUser}
        </div>
        <p id="subs-user-state" class="${subs.opensubtitles_user ? "key-saved" : "muted"}">${esc(userState)}</p>
      </form>`
    : "";
  const keyForms = SUB_KEY_SOURCES.map((source) => subKeyForm(source, subs[source.id], busy)).join("");
  const services = ["Gestdown", "TVsubtitles", "Subtitle Cat"];
  if (subs.opensubtitles) services.push("OpenSubtitles");
  for (const source of SUB_KEY_SOURCES) if (subs[source.id]) services.push(source.name);
  return `
    <section class="server-card" id="server-subs">
      <h2>Subtitles</h2>
      <p class="muted">The CC button finds subtitles for what you play. Gestdown and TVsubtitles cover TV shows, Subtitle Cat covers films and TV shows (machine translations are marked), and OpenSubtitles, SubDL and SubSource join in once their keys are saved. Searching sends the title, year, season and episode to ${esc(spokenList(services))}.</p>
      <div class="subs-langs">
        <span id="subs-langs-label">Preferred languages${saved ? "" : " (from your browser)"}</span>
        <ol class="lang-chips" aria-labelledby="subs-langs-label">${chips}</ol>
        <label class="sr-only" for="subs-add-lang">Add a language</label>
        <select id="subs-add-lang"${full || subsBusy ? " disabled" : ""}>
          <option value="">${full ? "Up to " + MAX_SUB_LANGUAGES + " languages" : "Add a language…"}</option>
          ${options}
        </select>
      </div>
      <div class="row">
        <span id="subs-hi-label">Prefer subtitles for the hearing impaired</span>
        <button type="button" id="subs-hi" role="switch" aria-checked="${hearing ? "true" : "false"}" aria-labelledby="subs-hi-label" data-act="${act(() => void saveSubs({ hearing_impaired: !hearing }, ""))}"${busy}>${hearing ? "On" : "Off"}</button>
      </div>
      <form id="subs-key-form" class="media-key">
        <label for="subs-key">OpenSubtitles API key (optional, adds OpenSubtitles results)</label>
        <div class="search">
          <input id="subs-key" type="password" autocomplete="off" spellcheck="false" placeholder="${subs.opensubtitles ? "Paste a new key" : "Paste an OpenSubtitles API key"}" aria-describedby="subs-key-state subs-key-hint${serverSubsError ? " subs-error" : ""}" />
          <button type="submit"${busy}>Save</button>
          ${removeKey}
        </div>
        <p id="subs-key-state" class="${subs.opensubtitles ? "key-saved" : "muted"}">${esc(keyState)}</p>
        <p id="subs-key-hint" class="muted">A free key comes from your account on opensubtitles.com → API consumers.</p>
      </form>
      ${login}
      ${keyForms}
      ${serverSubsNote ? `<p class="muted" role="status">${esc(serverSubsNote)}</p>` : ""}
      ${serverSubsError ? `<p class="error" id="subs-error" role="alert">${esc(serverSubsError)}</p>` : ""}
    </section>
  `;
}

function wireSubs(): void {
  const add = document.getElementById("subs-add-lang");
  if (add instanceof HTMLSelectElement) {
    add.addEventListener("change", () => {
      const code = add.value;
      if (!code || !serverSubs) return;
      const current = serverSubs.languages.length ? serverSubs.languages : browserLanguages();
      void saveSubs({ languages: [...current, code].slice(0, MAX_SUB_LANGUAGES) }, languageName(code) + " added.", "subs-add-lang");
    });
  }
  const keyForm = document.getElementById("subs-key-form");
  if (keyForm instanceof HTMLFormElement) {
    keyForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const field = document.getElementById("subs-key");
      const value = field instanceof HTMLInputElement ? field.value.trim() : "";
      if (!value) {
        serverSubsError = "Enter an OpenSubtitles API key.";
        serverSubsNote = "";
        repaintSubs("subs-key");
        return;
      }
      void saveSubs({ opensubtitles_key: value }, "OpenSubtitles key saved.", "", "subs-key");
    });
  }
  for (const source of SUB_KEY_SOURCES) {
    const form = document.getElementById(`subs-${source.id}-key-form`);
    if (!(form instanceof HTMLFormElement)) continue;
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const fieldId = `subs-${source.id}-key`;
      const field = document.getElementById(fieldId);
      const value = field instanceof HTMLInputElement ? field.value.trim() : "";
      if (!value) {
        serverSubsError = `Enter a ${source.name} API key.`;
        serverSubsNote = "";
        repaintSubs(fieldId);
        return;
      }
      void saveSubs(subKeyPut(source, value), source.name + " key saved.", "", fieldId);
    });
  }
  const loginForm = document.getElementById("subs-login-form");
  if (loginForm instanceof HTMLFormElement) {
    loginForm.addEventListener("submit", (event) => {
      event.preventDefault();
      const user = document.getElementById("subs-user");
      const pass = document.getElementById("subs-pass");
      const username = user instanceof HTMLInputElement ? user.value.trim() : "";
      const password = pass instanceof HTMLInputElement ? pass.value : "";
      if (!username || !password) {
        serverSubsError = "Enter your OpenSubtitles username and password.";
        serverSubsNote = "";
        repaintSubs(username ? "subs-pass" : "subs-user");
        return;
      }
      void saveSubs(
        { opensubtitles_username: username, opensubtitles_password: password },
        "Signed in to OpenSubtitles.",
        "",
        "subs-user",
      );
    });
  }
}

function repaintSubs(focusId = "", keepTyped = false): void {
  const node = document.getElementById("server-subs");
  if (!node) return;
  const typed = new Map<string, string>();
  if (keepTyped) {
    for (const id of ["subs-key", "subs-user", "subs-subdl-key", "subs-subsource-key"]) {
      const field = document.getElementById(id);
      if (field instanceof HTMLInputElement && field.value) typed.set(id, field.value);
    }
  }
  node.outerHTML = subsSection();
  wireSubs();
  for (const [id, value] of typed) {
    const field = document.getElementById(id);
    if (field instanceof HTMLInputElement) field.value = value;
  }
  if (focusId) document.getElementById(focusId)?.focus();
}

async function saveSubs(put: SubtitleSettingsPut, note: string, focusAfter = "", focusOnError = ""): Promise<void> {
  if (subsBusy) return;
  const token = renderToken;
  subsBusy = true;
  let focus = focusAfter;
  let failed = false;
  try {
    serverSubs = cleanSubtitleSettings(await api.setSubtitleSettings(put));
    rememberSubtitleSettings(serverSubs);
    serverSubsError = "";
    serverSubsNote = note;
  } catch (err) {
    serverSubsError = detail(err);
    serverSubsNote = "";
    focus = focusOnError || focusAfter;
    failed = true;
  } finally {
    subsBusy = false;
  }
  if (token === renderToken) repaintSubs(focus || (put.hearing_impaired !== undefined ? "subs-hi" : ""), failed);
}

async function refreshUpdate(): Promise<void> {
  if (isRemote()) return;
  try {
    noteUpdate(await api.update());
  } catch {
    /* Portal is restarting, or it predates updates. */
  }
}

function noteUpdate(info: UpdateInfo): void {
  updateInfo = info;
  updateFetchedAt = Date.now();
  if (info.state === "downloading" || info.state === "installing") watchUpdate(info.current);
  paintUpdatePill();
  repaintUpdate();
}

function paintUpdatePill(): void {
  const pill = document.getElementById("update-pill");
  if (!pill) return;
  const info = updateInfo;
  const installing = info?.state === "installing";
  pill.hidden = !(info?.available || installing);
  pill.textContent = updatePillText(info);
  pill.title = info?.latest ? "Portal " + info.latest : "";
}

function updatePillText(info: UpdateInfo | null): string {
  if (info?.state === "downloading") {
    const percent = updatePercent(info);
    return percent === null ? "Downloading…" : `Downloading ${percent}%`;
  }
  if (info?.state === "ready") return "Update downloaded";
  if (info?.state === "installing") return "Updating…";
  return "Update available";
}

function updatePercent(info: UpdateInfo): number | null {
  if (typeof info.progress !== "number" || !Number.isFinite(info.progress)) return null;
  return Math.round(Math.min(1, Math.max(0, info.progress)) * 100);
}

function updateLine(info: UpdateInfo): string {
  const latest = info.latest ?? "";
  if (info.state === "checking") return "Checking for updates…";
  if (info.state === "downloading") {
    const percent = updatePercent(info);
    return `Downloading Portal ${latest}` + (percent === null ? "…" : ` (${percent}%)`);
  }
  if (info.state === "ready") return `Portal ${latest} is downloaded.`;
  if (info.state === "installing") return `Installing Portal ${latest}. Portal will close and reopen.`;
  if (info.available) return `Portal ${latest} is available.`;
  if (info.state === "failed" || info.error) return "";
  return info.latest ? "Portal is up to date." : "No update found.";
}

function releaseLink(url: string | null): string {
  if (!url || !/^https:\/\/github\.com\//.test(url)) return "";
  return `<a class="action-link" href="${esc(url)}" target="_blank" rel="noopener noreferrer">Release notes</a>`;
}

function updateSection(): string {
  const info = updateInfo;
  if (!info) {
    return `
    <section class="server-card update-card" id="server-update">
      <h2>Updates</h2>
      <p class="muted">Update status is not available.</p>
    </section>
  `;
  }
  const working = updateBusy || info.state === "checking" || info.state === "downloading" || info.state === "installing";
  const percent = updatePercent(info);
  const meter =
    info.state === "downloading"
      ? `<div class="activity-meter update-meter${percent === null ? " indeterminate" : ""}" role="progressbar" aria-label="Download" aria-valuemin="0" aria-valuemax="100"${percent === null ? "" : ` aria-valuenow="${percent}"`}><span${percent === null ? "" : ` style="width: ${percent}%"`}></span></div>`
      : "";
  const line = updateLine(info);
  const problem = updateError || info.error || "";
  const problemClass = updateError || info.state === "failed" ? "error" : "muted";
  const install =
    info.available && info.latest
      ? `<button type="button" class="update-install" data-act="${act(() => void installUpdate())}"${working ? " disabled" : ""}>Install ${esc(info.latest)}</button>`
      : "";
  return `
    <section class="server-card update-card" id="server-update">
      <h2>Updates</h2>
      <div class="row fact"><span class="muted">Portal version</span><span>${esc(info.current)}</span></div>
      ${line ? `<p class="update-line">${esc(line)}</p>` : ""}
      ${meter}
      ${problem ? `<p class="${problemClass}">${esc(problem)}</p>` : ""}
      <div class="row">
        <button type="button" data-act="${act(() => void checkUpdate())}"${working ? " disabled" : ""}>Check for updates</button>
        ${install}
        ${releaseLink(info.release_url)}
      </div>
    </section>
  `;
}

function repaintUpdate(): void {
  const node = document.getElementById("server-update");
  if (!node) return;
  // The card is redrawn often while a download runs; keep focus on the same button.
  const active = document.activeElement;
  const focused = active instanceof HTMLElement && node.contains(active) ? active.textContent?.trim() : "";
  node.outerHTML = updateSection();
  if (!focused) return;
  const controls = document.getElementById("server-update")?.querySelectorAll<HTMLElement>("button, a");
  Array.from(controls ?? []).find((control) => control.textContent?.trim() === focused)?.focus();
}

async function checkUpdate(): Promise<void> {
  if (updateBusy) return;
  updateBusy = true;
  updateError = "";
  if (updateInfo) updateInfo = { ...updateInfo, state: "checking", error: null };
  repaintUpdate();
  try {
    noteUpdate(await api.checkUpdate());
  } catch (err) {
    updateError = detail(err);
    await refreshUpdate();
  } finally {
    updateBusy = false;
    repaintUpdate();
  }
}

async function installUpdate(): Promise<void> {
  const info = updateInfo;
  if (updateBusy || !info?.available) return;
  if (!window.confirm("Portal will close, update, and reopen. Anything playing will stop.")) return;
  updateBusy = true;
  updateError = "";
  updateInfo = { ...info, state: "downloading", progress: null, error: null };
  watchUpdate(info.current);
  repaintUpdate();
  try {
    noteUpdate(await api.installUpdate());
  } catch (err) {
    if (err instanceof ApiError) {
      updateError = detail(err);
      await refreshUpdate();
    } else {
      // No answer at all: the installer is most likely stopping Portal already.
      updateInfo = { ...info, state: "installing", progress: null, error: null };
    }
  } finally {
    updateBusy = false;
    paintUpdatePill();
    repaintUpdate();
  }
}

/** Follows a download or install; reloads once Portal is back as a new version. */
function watchUpdate(fromVersion: string): void {
  if (updateTimer !== undefined) return;
  updateFromVersion = fromVersion;
  const started = Date.now();
  updateTimer = window.setInterval(() => void pollUpdate(started), UPDATE_WATCH_MS);
}

function stopUpdateWatch(): void {
  if (updateTimer === undefined) return;
  window.clearInterval(updateTimer);
  updateTimer = undefined;
}

async function pollUpdate(started: number): Promise<void> {
  if (Date.now() - started > UPDATE_WATCH_LIMIT_MS) {
    stopUpdateWatch();
    return;
  }
  if (updatePolling) return;
  updatePolling = true;
  try {
    const info = await api.update();
    if (updateFromVersion && info.current !== updateFromVersion) {
      stopUpdateWatch();
      location.reload();
      return;
    }
    if (!updateBusy && info.state !== "downloading" && info.state !== "installing") stopUpdateWatch();
    noteUpdate(info);
  } catch {
    /* Portal is down while the installer runs. */
  } finally {
    updatePolling = false;
  }
}

let serverBusy = false;

async function serverCall(action: "window" | "restart" | "quit", status: Status): Promise<void> {
  // One engine action at a time; a second click while Kodi restarts would start another.
  if (serverBusy) return;
  if (action === "quit" && !window.confirm("Close the Kodi engine? Anything playing stops.")) return;
  serverBusy = true;
  const ticket = action === "window" ? 0 : showPage(describeServer(action));
  const token = renderToken;
  try {
    const next =
      action === "window" ? await api.window(!status.window_visible) : action === "restart" ? await api.restart() : await api.quit();
    void refreshChip();
    if (token !== renderToken) return;
    keepFocus(main!, () => paintServer(next, ""));
  } catch (err) {
    if (token !== renderToken) return;
    keepFocus(main!, () => paintServer(status, detail(err)));
  } finally {
    serverBusy = false;
    if (ticket) hideTicket(ticket);
  }
}

// Started last, once every declaration above exists: the first render reads
// module state synchronously, and a `let` below this point would not exist yet.
// A picture's real shape decides how it sits in its poster frame: a wide still
// or a square icon shows whole instead of being cut to fit.
document.addEventListener(
  "load",
  (event) => {
    const img = event.target;
    if (!(img instanceof HTMLImageElement) || !img.classList.contains("art")) return;
    const ratio = img.naturalWidth / Math.max(1, img.naturalHeight);
    img.classList.toggle("art-wide", ratio > 1.2);
    img.classList.toggle("art-square", ratio >= 0.85 && ratio <= 1.2);
  },
  true,
);
// A picture that fails (a stale link, or Kodi stopped) shows the letter frame instead of an empty box.
document.addEventListener(
  "error",
  (event) => {
    const img = event.target;
    if (!(img instanceof HTMLImageElement) || !img.classList.contains("art") || !img.dataset.letter) return;
    const frame = document.createElement("div");
    frame.className = "art empty";
    frame.setAttribute("aria-hidden", "true");
    frame.textContent = img.dataset.letter;
    img.replaceWith(frame);
  },
  true,
);
mountThemes();
window.addEventListener("portal:pair", () => showPairing());
void (async () => {
  // A Portal that is still starting may not answer yet; ask a few times
  // before treating this screen as a device that only watches.
  for (let attempt = 0; attempt < 5 && !pairInfo; attempt += 1) {
    try {
      pairInfo = await api.pairMe();
    } catch {
      await new Promise((resolve) => window.setTimeout(resolve, 1000 * (attempt + 1)));
    }
  }
  if (!pairInfo) pairInfo = { paired: true, this_pc: false };
  if (pairInfo && !pairInfo.paired) {
    showPairing();
    return;
  }
  startApp();
})();

/** True on a phone, tablet or TV: anything but the PC running Portal. */
function isRemote(): boolean {
  return pairInfo !== null && !pairInfo.this_pc;
}

function startApp(): void {
  appStarted = true;
  wireAppMenu(() => !isRemote());
  if (isRemote()) {
    // Settings live on the PC; this device only watches and casts.
    document.body.classList.add("remote");
    document
      .querySelectorAll<HTMLAnchorElement>('a[href="#/source"], a[href="#/server"], #update-pill')
      .forEach((link) => link.remove());
  }
  void render();
  void refreshChip();
  void refreshUpdate();
  window.setInterval(() => {
    if (!document.hidden) void refreshChip();
  }, 4000);
  window.setInterval(() => {
    if (!document.hidden) void refreshUpdate();
  }, UPDATE_POLL_MS);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) return;
    void refreshChip();
    if (Date.now() - updateFetchedAt > UPDATE_POLL_MS) void refreshUpdate();
  });
}
