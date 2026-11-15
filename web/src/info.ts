/** The Info dialog: shows what is known at once, then what an online lookup adds. */
import { api, artUrl, type MediaInfo, type MediaLink } from "./api";
import { webUrl } from "./html";
import { savedButton, type SaveTitle } from "./saved";
import { episodeCode } from "./titles";

export type InfoRequest = {
  /** What the page already knows; it is shown at once and sent to the lookup. */
  seed: MediaInfo;
  /** Server art for the title, used when the info has no poster. */
  art?: string | null;
  /** What Watchlist and Bookmark save; an episode saves its show. */
  save?: SaveTitle | null;
  /** A primary action such as Play. */
  play?: { label: string; run: () => void } | null;
  /** Gets focus back when the dialog closes. */
  opener?: HTMLElement | null;
};

type Lookup = "loading" | "done" | "failed";

const CAST_LIMIT = 12;

let layer: HTMLElement | null = null;
let dialog: HTMLElement | null = null;
let content: HTMLElement | null = null;
let statusLine: HTMLElement | null = null;
let sourcesLine: HTMLElement | null = null;
let session = 0;
let opener: HTMLElement | null = null;
let current: { req: InfoRequest; info: MediaInfo; lookup: Lookup } | null = null;
let inerted: HTMLElement[] = [];
let downOnLayer = false;

export function infoOpen(): boolean {
  return !!layer && !layer.hidden;
}

function node<K extends keyof HTMLElementTagNameMap>(tag: K, className = "", text = ""): HTMLElementTagNameMap[K] {
  const made = document.createElement(tag);
  if (className) made.className = className;
  if (text) made.textContent = text;
  return made;
}

function build(): void {
  if (layer) return;
  layer = node("div", "info-layer");
  layer.hidden = true;
  dialog = node("div", "info-dialog");
  dialog.setAttribute("role", "dialog");
  dialog.setAttribute("aria-modal", "true");
  dialog.setAttribute("aria-labelledby", "info-title");
  dialog.tabIndex = -1;
  const top = node("div", "info-top");
  const close = node("button", "icon-btn info-close");
  close.type = "button";
  close.dataset.key = "close";
  close.setAttribute("aria-label", "Close");
  close.title = "Close (Esc)";
  close.innerHTML = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M6 6l12 12M18 6L6 18" /></svg>`;
  close.addEventListener("click", () => closeInfo());
  top.append(close);
  content = node("div", "info-content");
  const foot = node("div", "info-foot");
  statusLine = node("p", "info-status");
  statusLine.setAttribute("role", "status");
  sourcesLine = node("p", "info-sources");
  foot.append(statusLine, sourcesLine);
  dialog.append(top, content, foot);
  layer.append(dialog);
  document.body.append(layer);
  layer.addEventListener("mousedown", (event) => {
    downOnLayer = event.target === layer;
  });
  layer.addEventListener("click", (event) => {
    if (event.target === layer && downOnLayer) closeInfo();
    downOnLayer = false;
  });
  window.addEventListener("hashchange", () => {
    if (infoOpen()) closeInfo(false);
  });
}

function focusables(): HTMLElement[] {
  if (!dialog) return [];
  return [...dialog.querySelectorAll<HTMLElement>("a[href], button:not(:disabled), [tabindex]:not([tabindex='-1'])")].filter(
    (item) => item.offsetParent !== null || item === document.activeElement,
  );
}

/** Keys while the dialog is open: Esc closes, Tab stays inside, nothing reaches the player's shortcuts. */
function onKey(event: KeyboardEvent): void {
  if (!infoOpen() || !dialog) return;
  if (event.key === "Escape") {
    event.preventDefault();
    event.stopPropagation();
    closeInfo();
    return;
  }
  if (event.key === "Tab") {
    const items = focusables();
    const active = document.activeElement;
    if (!items.length) {
      event.preventDefault();
      dialog.focus();
    } else if (!(active instanceof Node) || !dialog.contains(active)) {
      event.preventDefault();
      items[0].focus();
    } else if (event.shiftKey && (active === items[0] || active === dialog)) {
      event.preventDefault();
      items[items.length - 1].focus();
    } else if (!event.shiftKey && active === items[items.length - 1]) {
      event.preventDefault();
      items[0].focus();
    }
  }
  event.stopPropagation();
}

function setInert(on: boolean): void {
  if (on) {
    inerted = [];
    for (const child of [...document.body.children]) {
      if (child === layer || !(child instanceof HTMLElement) || child.inert) continue;
      child.inert = true;
      inerted.push(child);
    }
    return;
  }
  for (const child of inerted) child.inert = false;
  inerted = [];
}

function present(value: unknown): boolean {
  if (value === null || value === undefined) return false;
  if (typeof value === "string") return value.trim() !== "";
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.length > 0;
  return true;
}

/** Fields the server sends replace the ones the page knew; empty ones do not. */
function merge(base: MediaInfo, extra: MediaInfo | null | undefined): MediaInfo {
  const out: Record<string, unknown> = { ...base };
  if (extra && typeof extra === "object") {
    for (const [key, value] of Object.entries(extra)) {
      if (present(value)) out[key] = value;
    }
  }
  return out as MediaInfo;
}

function clean(seed: MediaInfo): MediaInfo {
  return merge({}, seed);
}

export function openInfo(req: InfoRequest): void {
  build();
  if (!layer || !dialog) return;
  session += 1;
  const mine = session;
  if (!infoOpen()) {
    opener = req.opener ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    setInert(true);
    layer.hidden = false;
    document.body.classList.add("info-open");
    document.addEventListener("keydown", onKey, true);
  } else if (req.opener) {
    opener = req.opener;
  }
  const seed = clean(req.seed);
  current = { req, info: seed, lookup: "loading" };
  paint(false);
  dialog.scrollTop = 0;
  dialog.focus();
  void lookup(mine, seed);
}

async function lookup(mine: number, seed: MediaInfo): Promise<void> {
  try {
    const found = await api.info(seed);
    if (mine !== session || !current) return;
    current.info = merge(current.info, found);
    current.lookup = "done";
  } catch {
    if (mine !== session || !current) return;
    current.lookup = "failed";
  }
  paint(true);
}

export function closeInfo(restoreFocus = true): void {
  if (!layer || layer.hidden) return;
  session += 1;
  current = null;
  layer.hidden = true;
  document.body.classList.remove("info-open");
  document.removeEventListener("keydown", onKey, true);
  setInert(false);
  content?.replaceChildren();
  const back = opener;
  opener = null;
  if (restoreFocus && back && back.isConnected) back.focus();
}

function image(value: string | null | undefined): string | null {
  return artUrl(value);
}

function list(values: string[] | null | undefined, limit = 4): string {
  if (!Array.isArray(values)) return "";
  return values
    .filter((value) => typeof value === "string" && value.trim())
    .slice(0, limit)
    .join(", ");
}

/** More than one name, counting names an add-on joined with commas. */
function several(values: string[] | null | undefined): boolean {
  if (!Array.isArray(values)) return false;
  const named = values.filter((value) => typeof value === "string" && value.trim());
  return named.length > 1 || named.some((value) => value.includes(", "));
}

export function runtime(minutes: number | null | undefined): string {
  if (typeof minutes !== "number" || !Number.isFinite(minutes) || minutes <= 0) return "";
  const total = Math.round(minutes);
  const hours = Math.floor(total / 60);
  const rest = total % 60;
  if (!hours) return `${rest}m`;
  return rest ? `${hours}h ${rest}m` : `${hours}h`;
}

function yearOf(info: MediaInfo): string {
  if (typeof info.year === "number" && info.year > 1800) return String(info.year);
  const match = /^(\d{4})/.exec(info.premiered ?? "");
  return match ? match[1] : "";
}

function aired(value: string | null | undefined): string {
  if (!value || !/^\d{4}-\d{2}-\d{2}/.test(value)) return "";
  const date = new Date(value.slice(0, 10) + "T12:00:00");
  if (Number.isNaN(date.getTime())) return "";
  return "Aired " + date.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
}

function votes(count: number | null | undefined): string {
  if (typeof count !== "number" || !Number.isFinite(count) || count <= 0) return "";
  const text = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 }).format(count);
  return `${text} ${count === 1 ? "vote" : "votes"}`;
}

function kicker(info: MediaInfo): string {
  if (info.kind === "episode") {
    return [info.show_title?.trim() ?? "", episodeCode(info.season, info.episode)].filter(Boolean).join(" · ") || "Episode";
  }
  if (info.kind === "tv") return "Show";
  if (info.kind === "movie") return "Movie";
  return "";
}

function links(info: MediaInfo): MediaLink[] {
  const out: MediaLink[] = [];
  const seen = new Set<string>();
  const add = (label: string, url: string | null) => {
    const clean = label.trim();
    if (!clean || !url || seen.has(clean.toLowerCase())) return;
    seen.add(clean.toLowerCase());
    out.push({ label: clean, url });
  };
  for (const link of info.links ?? []) {
    if (link && typeof link.label === "string" && typeof link.url === "string") add(link.label, webUrl(link.url));
  }
  if (info.imdb_id && /^tt\d{5,12}$/.test(info.imdb_id)) {
    add("IMDb", `https://www.imdb.com/title/${info.imdb_id}/`);
  }
  if (info.tmdb_id && /^\d{1,12}$/.test(info.tmdb_id) && (info.kind === "movie" || info.kind === "tv")) {
    add("TMDB", `https://www.themoviedb.org/${info.kind}/${info.tmdb_id}`);
  }
  add("Trailer", webUrl(info.trailer));
  return out;
}

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  const letters = (parts[0]?.charAt(0) ?? "") + (parts.length > 1 ? parts[parts.length - 1].charAt(0) : "");
  return letters.toUpperCase() || "?";
}

function picture(url: string, className: string, onFail: () => void): HTMLImageElement {
  const img = node("img", className);
  img.alt = "";
  img.decoding = "async";
  img.addEventListener("error", () => {
    img.remove();
    onFail();
  });
  img.src = url;
  return img;
}

/** Rebuilds the dialog's content; keeps focus and scroll when it is a refresh. */
function paint(refresh: boolean): void {
  if (!current || !content || !dialog || !statusLine || !sourcesLine) return;
  const { req, info, lookup } = current;
  const focusKey = refresh && document.activeElement instanceof HTMLElement ? document.activeElement.dataset.key ?? "" : "";
  const scroll = dialog.scrollTop;
  content.replaceChildren();

  const title = info.title?.trim() || info.show_title?.trim() || "Untitled";
  const fanart = image(info.fanart);
  // An episode shows its own still; the poster the add-on gives it is the season's or the show's.
  const still = info.kind === "episode" ? image(req.art) : null;
  const poster = still ?? image(info.poster) ?? image(req.art);

  const hero = node("div", "info-hero");
  if (fanart) {
    hero.classList.add("has-fanart");
    const backdrop = node("div", "info-backdrop");
    backdrop.setAttribute("aria-hidden", "true");
    backdrop.append(picture(fanart, "", () => {
      backdrop.remove();
      hero.classList.remove("has-fanart");
    }));
    hero.append(backdrop);
  }
  const head = node("div", "info-head");
  if (poster) {
    const frame = node("div", still ? "info-poster is-still" : "info-poster");
    frame.append(picture(poster, "", () => frame.remove()));
    head.append(frame);
  }
  const heading = node("div", "info-heading");
  const kick = kicker(info);
  if (kick) heading.append(node("p", "info-kicker", kick));
  const h2 = node("h2", "", title);
  h2.id = "info-title";
  heading.append(h2);
  const original = info.original_title?.trim();
  if (original && original.toLowerCase() !== title.toLowerCase()) heading.append(node("p", "info-original", original));
  const facts = [
    info.kind === "episode" ? aired(info.premiered) || yearOf(info) : yearOf(info),
    runtime(info.runtime_minutes),
    info.mpaa?.trim() ?? "",
    list(info.genres, 4),
  ].filter(Boolean);
  if (facts.length) heading.append(node("p", "info-facts", facts.join(" · ")));
  if (typeof info.rating === "number" && Number.isFinite(info.rating) && info.rating > 0) {
    const rating = node("p", "info-rating");
    const score = Math.min(10, info.rating).toFixed(1);
    rating.setAttribute("aria-label", `Rated ${score} out of 10${info.votes ? ", " + votes(info.votes) : ""}`);
    const star = node("span", "info-star", "★");
    star.setAttribute("aria-hidden", "true");
    const value = node("strong", "", score);
    value.setAttribute("aria-hidden", "true");
    const scale = node("span", "info-scale", "/10");
    scale.setAttribute("aria-hidden", "true");
    rating.append(star, value, scale);
    const count = votes(info.votes);
    if (count) {
      const small = node("span", "info-votes", " · " + count);
      small.setAttribute("aria-hidden", "true");
      rating.append(small);
    }
    heading.append(rating);
  }
  const actions = node("div", "info-actions");
  if (req.play) {
    const play = node("button", "primary");
    play.type = "button";
    play.dataset.key = "play";
    play.innerHTML = `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l10.5-6.5z" /></svg>`;
    play.append(node("span", "", req.play.label));
    const run = req.play.run;
    play.addEventListener("click", () => {
      // Focus goes back to what opened Info first, so closing the player returns there.
      closeInfo();
      run();
    });
    actions.append(play);
  }
  if (req.save) {
    const watch = savedButton("watchlist", req.save, "pill");
    watch.dataset.key = "watchlist";
    const book = savedButton("bookmarks", req.save, "pill");
    book.dataset.key = "bookmarks";
    actions.append(watch, book);
  }
  if (actions.childElementCount) heading.append(actions);
  head.append(heading);
  hero.append(head);
  content.append(hero);

  const body = node("div", "info-body");
  const tagline = info.tagline?.trim();
  if (tagline) body.append(node("p", "info-tagline", tagline));
  const plot = info.plot?.trim();
  if (plot) body.append(node("p", "info-plot", plot));
  else if (lookup !== "loading") body.append(node("p", "info-plot muted", "No summary is available for this title."));

  const credits: [string, string][] = [
    [several(info.directors) ? "Directors" : "Director", list(info.directors)],
    [several(info.writers) ? "Writers" : "Writer", list(info.writers)],
    [several(info.studios) ? "Studios" : "Studio", list(info.studios, 3)],
    [several(info.countries) ? "Countries" : "Country", list(info.countries, 3)],
  ];
  const shown = credits.filter(([, value]) => value);
  if (shown.length) {
    const dl = node("dl", "info-credits");
    for (const [label, value] of shown) dl.append(node("dt", "", label), node("dd", "", value));
    body.append(dl);
  }

  const cast = (info.cast ?? []).filter((member) => member && typeof member.name === "string" && member.name.trim()).slice(0, CAST_LIMIT);
  if (cast.length) {
    const section = node("section", "info-section");
    const castHeading = node("h3", "", "Cast");
    castHeading.id = "info-cast-title";
    section.append(castHeading);
    const ul = node("ul", "info-cast");
    ul.setAttribute("aria-labelledby", "info-cast-title");
    for (const member of cast) {
      const li = node("li", "cast-chip");
      const face = node("span", "cast-face");
      face.setAttribute("aria-hidden", "true");
      face.textContent = initials(member.name);
      const thumb = image(member.thumb);
      if (thumb) {
        face.classList.add("has-photo");
        face.append(picture(thumb, "", () => face.classList.remove("has-photo")));
      }
      const words = node("span", "cast-words");
      words.append(node("strong", "", member.name.trim()));
      const role = member.role?.trim();
      if (role) words.append(node("span", "cast-role", role));
      li.append(face, words);
      ul.append(li);
    }
    section.append(ul);
    body.append(section);
  }

  const found = links(info);
  if (found.length) {
    const row = node("div", "info-links");
    for (const link of found) {
      const a = node("a", "link-btn");
      a.href = link.url;
      a.target = "_blank";
      a.rel = "noopener noreferrer";
      a.dataset.key = "link-" + link.label.toLowerCase();
      a.append(node("span", "", link.label));
      a.insertAdjacentHTML(
        "beforeend",
        `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path class="line" d="M14 5h5v5M19 5l-8 8M17 14v4.5a.5.5 0 0 1-.5.5h-11a.5.5 0 0 1-.5-.5v-11a.5.5 0 0 1 .5-.5H10" /></svg>`,
      );
      a.append(node("span", "sr-only", " (opens in a new tab)"));
      row.append(a);
    }
    body.append(row);
  }
  content.append(body);

  statusLine.textContent =
    lookup === "loading" ? "Looking up more…" : lookup === "failed" ? "Couldn't look up more info right now." : "";
  statusLine.classList.toggle("is-loading", lookup === "loading");
  const sources = (info.sources ?? []).filter((source) => typeof source === "string" && source.trim());
  sourcesLine.textContent = sources.length ? "Info from: " + sources.join(", ") : "";
  // The dialog is never aria-busy: screen readers may hold back a busy dialog's
  // content, and what it shows works during the lookup the status line names.

  if (refresh) {
    dialog.scrollTop = scroll;
    if (focusKey) dialog.querySelector<HTMLElement>(`[data-key="${CSS.escape(focusKey)}"]`)?.focus();
  }
}
