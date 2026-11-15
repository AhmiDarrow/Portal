/** Watchlist and bookmark state shared by browse cards, the details page, the info dialog, and the player. */
import { api, type ListPut, type SavedState, type TitleKind } from "./api";
import { setNotice } from "./activity";
import { esc } from "./html";

export type SaveList = "watchlist" | "bookmarks";

/** What a toggle saves: a movie, or a show (an episode saves its show). */
export type SaveTitle = {
  id: string;
  kind: TitleKind;
  name: string;
  art?: string | null;
  year?: number | null;
  imdb_id?: string | null;
};

export const ICONS = {
  info: `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><circle class="line" cx="12" cy="12" r="8.6" /><path class="line" d="M12 11v5.2" /><circle cx="12" cy="7.9" r="1.15" /></svg>`,
  watchlist: `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path class="line when-off" d="M4.5 7h11M4.5 12h11M4.5 17h6.5M17.5 14v7M14 17.5h7" /><path class="line when-on" d="M4.5 7h15M4.5 12h15M4.5 17h6.5M14 17.6l2.4 2.4 4.1-4.6" /></svg>`,
  bookmarks: `<svg class="ico" viewBox="0 0 24 24" aria-hidden="true"><path class="line when-off" d="M7 4.5h10v15.2l-5-3.7-5 3.7z" /><path class="fill when-on" d="M7 4.5h10v15.2l-5-3.7-5 3.7z" /></svg>`,
} as const;

const LIST_NAME: Record<SaveList, string> = { watchlist: "Watchlist", bookmarks: "Bookmark" };

const states = new Map<string, SavedState>();
/** Sequence number of the last local change per title; older server answers do not overwrite it. */
const touched = new Map<string, number>();
const busy = new Set<string>();
let seq = 0;

export function savedState(id: string): SavedState {
  return states.get(id) ?? { watchlist: false, bookmarks: false };
}

/** Records state the page already knows (e.g. from the details page). */
export function noteSaved(id: string, state: Partial<SavedState>): void {
  if (!id) return;
  states.set(id, { ...savedState(id), ...state });
}

/** Fetches state for the given titles with one request, then repaints every toggle. */
export async function loadSaved(ids: string[], wanted: () => boolean = () => true): Promise<void> {
  const unique = [...new Set(ids.filter((id) => !!id))];
  if (!unique.length) return;
  const started = seq;
  let items: unknown;
  try {
    items = (await api.saved(unique)).items;
  } catch {
    return;
  }
  if (!wanted()) return;
  if (!items || typeof items !== "object" || Array.isArray(items)) return;
  const rows = items as Record<string, unknown>;
  for (const id of unique) {
    const row = rows[id];
    if (!row || typeof row !== "object") continue;
    if ((touched.get(id) ?? 0) > started) continue;
    const value = row as Record<string, unknown>;
    states.set(id, { watchlist: value.watchlist === true, bookmarks: value.bookmarks === true });
  }
  paintSaved();
}

function tip(list: SaveList, on: boolean, name: string): string {
  const quoted = name ? `“${name}”` : "this title";
  if (list === "watchlist") return on ? `Remove ${quoted} from the watchlist` : `Add ${quoted} to the watchlist`;
  return on ? `Remove the bookmark for ${quoted}` : `Bookmark ${quoted}`;
}

/** Brings every toggle on the page in line with the known state. */
export function paintSaved(root: ParentNode = document): void {
  root.querySelectorAll<HTMLElement>("[data-saved-list]").forEach(paintNode);
}

function paintNode(node: HTMLElement): void {
  const list = node.dataset.savedList === "bookmarks" ? "bookmarks" : "watchlist";
  const id = node.dataset.savedId ?? "";
  const on = !!id && savedState(id)[list];
  node.classList.toggle("is-on", on);
  node.setAttribute("aria-pressed", on ? "true" : "false");
  node.title = tip(list, on, node.dataset.savedName ?? "");
  const text = node.querySelector<HTMLElement>(".save-text");
  if (text) text.textContent = on ? (list === "watchlist" ? "In watchlist" : "Bookmarked") : LIST_NAME[list];
}

function bodyFor(title: SaveTitle): ListPut {
  const body: ListPut = { kind: title.kind, name: title.name, art: title.art ?? null };
  if (typeof title.year === "number" && Number.isFinite(title.year)) body.year = title.year;
  if (title.imdb_id) body.imdb_id = title.imdb_id;
  return body;
}

function message(err: unknown): string {
  return err instanceof Error && err.message.trim() ? err.message : "Portal could not save that.";
}

/** Flips one list for a title right away, then undoes it with a notice if the service refuses. */
export async function toggleSaved(list: SaveList, title: SaveTitle): Promise<void> {
  if (!title.id) return;
  const key = list + "\n" + title.id;
  if (busy.has(key)) return;
  busy.add(key);
  const was = savedState(title.id)[list];
  const set = (on: boolean) => {
    states.set(title.id, { ...savedState(title.id), [list]: on });
    touched.set(title.id, ++seq);
    paintSaved();
  };
  set(!was);
  try {
    if (was) await api.deleteList(list, title.id);
    else await api.putList(list, title.id, bodyFor(title));
  } catch (err) {
    set(was);
    const what = list === "watchlist" ? "the watchlist" : "bookmarks";
    setNotice(`Couldn't update ${what}. ${message(err)}`);
  } finally {
    busy.delete(key);
  }
}

/**
 * Markup for a toggle. `variant` "icon" is a round icon button; "pill" adds a text label.
 * The caller wires the click (main.ts through data-act).
 */
export function savedButtonHtml(list: SaveList, title: SaveTitle, act: string, variant: "icon" | "pill"): string {
  const on = savedState(title.id)[list];
  const cls = variant === "icon" ? "icon-btn save-btn" : "pill-btn save-btn";
  const text = variant === "pill" ? `<span class="save-text">${esc(on ? (list === "watchlist" ? "In watchlist" : "Bookmarked") : LIST_NAME[list])}</span>` : "";
  // The name stays put while aria-pressed carries the state; the visible text contains it.
  const label = ` aria-label="${LIST_NAME[list]}"`;
  return `<button type="button" class="${cls}${on ? " is-on" : ""}" data-act="${esc(act)}" data-saved-list="${list}" data-saved-id="${esc(title.id)}" data-saved-name="${esc(title.name)}" aria-pressed="${on ? "true" : "false"}"${label} title="${esc(tip(list, on, title.name))}">${ICONS[list]}${text}</button>`;
}

/** Markup for an Info button. */
export function infoButtonHtml(act: string, name: string, variant: "icon" | "pill"): string {
  const cls = variant === "icon" ? "icon-btn info-btn" : "pill-btn info-btn";
  const text = variant === "pill" ? `<span>Info</span>` : "";
  const label = variant === "icon" ? ` aria-label="Info"` : "";
  const title = name ? `Info about “${name}”` : "Info";
  return `<button type="button" class="${cls}" data-act="${esc(act)}" aria-haspopup="dialog"${label} title="${esc(title)}">${ICONS.info}${text}</button>`;
}

const bound = new WeakMap<HTMLElement, SaveTitle>();

/** Makes a toggle that is not rendered through main.ts's click registry flip its list. */
export function wireSavedButton(node: HTMLButtonElement, list: SaveList): void {
  node.addEventListener("click", () => {
    const title = bound.get(node);
    if (title) void toggleSaved(list, title);
  });
}

/** A ready-wired toggle element (used by the info dialog). */
export function savedButton(list: SaveList, title: SaveTitle, variant: "icon" | "pill"): HTMLButtonElement {
  const template = document.createElement("template");
  template.innerHTML = savedButtonHtml(list, title, "", variant);
  const node = template.content.firstElementChild as HTMLButtonElement;
  node.removeAttribute("data-act");
  wireSavedButton(node, list);
  bindSavedButton(node, title);
  return node;
}

/** Points an existing toggle at a title (or disables it when there is none). */
export function bindSavedButton(node: HTMLButtonElement, title: SaveTitle | null): void {
  if (title) bound.set(node, title);
  else bound.delete(node);
  node.dataset.savedId = title?.id ?? "";
  node.dataset.savedName = title?.name ?? "";
  node.disabled = !title;
  paintNode(node);
}
