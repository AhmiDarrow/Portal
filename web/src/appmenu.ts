/**
 * The Portal menu: what the tray icon's menu offers, on the dashboard of the
 * PC running Portal. On any other device the name stays a link to Home.
 */

import { api, ApiError, type AppInfo, type CastInfo, type Status, type UpdateInfo } from "./api";

type Loaded = {
  status: Status | null;
  update: UpdateInfo | null;
  cast: CastInfo | null;
  app: AppInfo | null;
};

const RESTART_TEXT = "Restart the Portal server? Anything playing will stop.";
const LAN_TEXT = "Portal restarts to change who can reach it. Anything playing will stop. Continue?";
const QUIT_TEXT = "Quit Portal? Anything playing or casting stops, and Kodi closes if Portal started it.";

let brand: HTMLAnchorElement | null = null;
let menu: HTMLElement | null = null;
let busy = false;
let loaded: Loaded = { status: null, update: null, cast: null, app: null };
let note = "";

function detail(err: unknown): string {
  return err instanceof ApiError || err instanceof Error ? err.message : "That did not work.";
}

function esc(text: string): string {
  return text.replace(/[&<>"']/g, (ch) => `&#${ch.charCodeAt(0)};`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((done) => setTimeout(done, ms));
}

async function quiet<T>(ask: () => Promise<T>): Promise<T | null> {
  try {
    return await ask();
  } catch {
    return null;
  }
}

/** Each answer is shown as it arrives: the engine's can take a moment. */
async function load(): Promise<void> {
  const show = <K extends keyof Loaded>(key: K, ask: () => Promise<Loaded[K]>): Promise<void> =>
    quiet(ask).then((value) => {
      loaded[key] = value;
      if (isOpen() && !busy) paint();
    });
  await Promise.all([
    show("status", api.status),
    show("update", api.update),
    show("cast", api.castInfo),
    show("app", api.app),
  ]);
}

function item(act: string, label: string, enabled = true, checked?: boolean): string {
  const role = checked === undefined ? "menuitem" : "menuitemcheckbox";
  const state = checked === undefined ? "" : ` aria-checked="${checked ? "true" : "false"}"`;
  const off = enabled && !busy ? "" : " disabled";
  const mark = checked === undefined ? "" : `<span class="app-check" aria-hidden="true">${checked ? "✓" : ""}</span>`;
  return `<button type="button" class="app-item" role="${role}"${state} data-app="${act}"${off}>${mark}<span>${esc(label)}</span></button>`;
}

function line(text: string): string {
  return `<p class="app-status">${esc(text)}</p>`;
}

function paint(): void {
  if (!menu) return;
  const { status, update, cast, app } = loaded;
  const port = location.port || (location.protocol === "https:" ? "443" : "80");
  const tray = app?.tray === true;
  const parts: string[] = [item("home", "Open Portal")];
  if (update?.available && update.latest) parts.push(item("server", `Install update ${update.latest}…`));
  parts.push(`<p class="app-head">Server</p>`);
  parts.push(line(`Server: running on port ${port}`));
  if (status) parts.push(line(`Kodi engine: ${status.state}${status.version ? ` (${status.version})` : ""}`));
  if (update) parts.push(line(`Version ${update.current}`));
  parts.push(`<hr />`);
  parts.push(item("lan", "Allow TVs on this network", cast !== null && tray, cast?.lan === true));
  parts.push(item("engine", "Restart Kodi engine"));
  parts.push(item("restart", "Restart Portal server", tray));
  parts.push(item("check", "Check for updates", update !== null));
  parts.push(`<hr />`);
  parts.push(item("server", "Open server page"));
  parts.push(item("logs", "Open log folder"));
  parts.push(`<hr />`);
  parts.push(item("autostart", "Start with Windows", tray && typeof app?.autostart === "boolean", app?.autostart === true));
  parts.push(item("quit", "Quit Portal", tray));
  parts.push(`<p class="app-note" role="status">${esc(note)}</p>`);
  menu.innerHTML = parts.join("");
}

function say(text: string): void {
  note = text;
  paint();
}

function isOpen(): boolean {
  return menu !== null && !menu.hidden;
}

function close(): void {
  if (!menu || menu.hidden) return;
  menu.hidden = true;
  brand?.setAttribute("aria-expanded", "false");
}

async function open(): Promise<void> {
  if (!menu) return;
  note = "";
  menu.hidden = false;
  brand?.setAttribute("aria-expanded", "true");
  paint();
  menu.querySelector<HTMLButtonElement>(".app-item:not(:disabled)")?.focus();
  await load();
  if (isOpen()) paint();
}

/** Waits for the server to go away and answer again, then loads the page fresh. */
async function reloadWhenBack(): Promise<void> {
  const until = Date.now() + 90_000;
  let gone = false;
  while (Date.now() < until) {
    await sleep(700);
    const up = (await quiet(api.status)) !== null;
    if (!up) gone = true;
    else if (gone || Date.now() > until - 80_000) break;
  }
  location.reload();
}

async function run(act: string): Promise<void> {
  if (busy) return;
  if (act === "home" || act === "server") {
    close();
    location.hash = act === "home" ? "#/" : "#/server";
    return;
  }
  busy = true;
  try {
    if (act === "engine") {
      say("Restarting the Kodi engine…");
      loaded.status = await api.restart();
      say("The Kodi engine restarted.");
    } else if (act === "check") {
      say("Checking for updates…");
      loaded.update = await api.checkUpdate();
      const found = loaded.update.available && loaded.update.latest;
      say(found ? `Portal ${loaded.update.latest} is available.` : "Portal is up to date.");
    } else if (act === "logs") {
      await api.appLogs();
      say("The log folder is open on this PC.");
    } else if (act === "autostart") {
      loaded.app = await api.appAutostart(loaded.app?.autostart !== true);
      say(loaded.app.autostart ? "Portal starts with Windows." : "Portal no longer starts with Windows.");
    } else if (act === "lan") {
      if (!confirm(LAN_TEXT)) return;
      loaded.cast = await api.setCastLan(loaded.cast?.lan !== true);
      say("Restarting Portal…");
      await api.appRestart();
      await reloadWhenBack();
    } else if (act === "restart") {
      if (!confirm(RESTART_TEXT)) return;
      say("Restarting Portal…");
      await api.appRestart();
      await reloadWhenBack();
    } else if (act === "quit") {
      if (!confirm(QUIT_TEXT)) return;
      await api.appQuit();
      say("Portal has quit. Start it again from its shortcut.");
    }
  } catch (err) {
    say(detail(err));
  } finally {
    busy = false;
    if (isOpen()) paint();
  }
}

/** Turns the Portal name into the menu's button when `local()` says this is the PC running Portal. */
export function wireAppMenu(local: () => boolean): void {
  brand = document.querySelector<HTMLAnchorElement>("a.brand");
  menu = document.querySelector<HTMLElement>("#app-menu");
  if (!brand || !menu) return;
  brand.addEventListener("click", (event) => {
    if (!local()) return;
    event.preventDefault();
    if (isOpen()) close();
    else void open();
  });
  menu.addEventListener("click", (event) => {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-app]");
    if (target && !target.disabled) void run(target.dataset.app ?? "");
  });
  document.addEventListener("click", (event) => {
    if (!isOpen()) return;
    // The path, not the target: a click on an item repaints the menu, and
    // the item that was clicked is no longer inside it.
    const path = event.composedPath();
    if (!(menu && path.includes(menu)) && !(brand && path.includes(brand))) close();
  });
  document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape" || !isOpen()) return;
    close();
    brand?.focus();
  });
  window.addEventListener("hashchange", close);
}
