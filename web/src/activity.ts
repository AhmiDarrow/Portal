export type ActivityInput = {
  title: string;
  detail: string;
  /** 0..1 from Kodi. Omit when the amount of work is unknown. */
  progress?: number | null;
};

export type ActivityView = {
  title: string;
  detail: string;
  status: string;
  percent: number | null;
  meter: "determinate" | "indeterminate";
};

export type PlayPhase = "starting" | "opening" | "waiting" | "preparing" | "working" | "answering" | "skipping";

type ActivityOptions = {
  delayMs?: number;
  onCancel?: (() => void) | null;
  cancelDisabled?: boolean;
};

type Pending = { ticket: number; timer: number };

let seq = 0;
let shownTicket = 0;
let shownOwner = "";
let pending: Pending | null = null;

/** Playback keeps the panel until it ends, so a folder refresh does not cover the add-on's work. */
export function beginActivity(owner: "page" | "play", input: ActivityInput, options?: ActivityOptions): number {
  const ticket = ++seq;
  const delay = options?.delayMs ?? 0;
  if (pending) {
    window.clearTimeout(pending.timer);
    pending = null;
  }
  const start = () => {
    pending = null;
    if (shownOwner === "play" && owner !== "play") return;
    shownTicket = ticket;
    shownOwner = owner;
    paint(input, options);
  };
  if (delay > 0) {
    pending = { ticket, timer: window.setTimeout(start, delay) };
    return ticket;
  }
  start();
  return ticket;
}

export function endActivity(ticket: number): void {
  if (pending?.ticket === ticket) {
    window.clearTimeout(pending.timer);
    pending = null;
  }
  if (shownTicket !== ticket) return;
  shownTicket = 0;
  shownOwner = "";
  const root = document.getElementById("activity");
  if (!root) return;
  root.hidden = true;
}

export function setNotice(text: string): void {
  const node = document.getElementById("notice");
  if (!node) return;
  // Shown before the text changes, so screen readers hear the status region's new text.
  node.hidden = !text;
  node.textContent = text;
}

export function activityView(input: ActivityInput): ActivityView {
  const title = input.title.trim() || "Working";
  const detail = input.detail.trim() || "Working.";
  const raw = input.progress;
  const status = detail.split("\n").find((part) => part.trim())?.trim() || detail;
  if (typeof raw === "number" && Number.isFinite(raw)) {
    const percent = Math.max(0, Math.min(100, Math.round(raw * 100)));
    return { title, detail, status, percent, meter: "determinate" };
  }
  return { title, detail, status, percent: null, meter: "indeterminate" };
}

export function describePlay(input: {
  item: string;
  phase: PlayPhase;
  heading?: string;
  message?: string;
  progress?: number | null;
  at?: string;
  canceling?: boolean;
  /** Source-list / Links flow — never claim a stream is resolving. */
  links?: boolean;
}): ActivityInput {
  const item = visibleText(input.item) || "Playback";
  if (input.phase === "starting") {
    return {
      title: item,
      detail: input.links ? "Starting Kodi for the source list." : "Starting Kodi.",
    };
  }
  if (input.links) {
    if (input.phase === "opening") return { title: item, detail: "Opening the add-on source list." };
    if (input.phase === "waiting") return { title: item, detail: "Waiting for the source list." };
    if (input.phase === "preparing") return { title: item, detail: "Opening the source list." };
  } else {
    if (input.phase === "opening") return { title: item, detail: "Asking the add-on to play this." };
    if (input.phase === "waiting") return { title: item, detail: "Waiting for a playable stream." };
    if (input.phase === "preparing") return { title: item, detail: "Preparing the video." };
  }
  if (input.phase === "skipping") {
    const at = visibleText(input.at || "");
    return { title: item, detail: at ? "Skipping to " + at + "." : "Skipping to the place you chose." };
  }
  if (input.phase === "answering") {
    const heading = visibleText(input.heading || "") || item;
    return {
      title: heading,
      detail: input.links ? "Waiting for a source-list answer." : "Waiting for an answer.",
    };
  }
  const head = visibleText(input.heading || "");
  const message = visibleDetail(input.message || "");
  const lines: string[] = [];
  if (head && head.toLowerCase() !== item.toLowerCase() && head.toLowerCase() !== "working") lines.push(head);
  if (message) lines.push(message);
  const idle = input.links ? "The add-on is opening the source list." : "The add-on is finding sources.";
  return {
    title: item,
    detail: input.canceling ? "Canceling." : lines.join("\n") || idle,
    progress: input.progress,
  };
}

export function describeBrowse(name: string, mode: "open" | "refresh" | "starting"): ActivityInput {
  const label = visibleText(name) || "This folder";
  const searchFolder = /\bsearch\b/i.test(label);
  if (mode === "refresh") {
    return {
      title: "Refreshing",
      detail: searchFolder ? "Checking this search folder for changes." : "Checking this folder for changes.",
    };
  }
  if (mode === "starting") {
    return {
      title: "Opening " + label,
      detail: searchFolder
        ? "Starting Kodi, then opening this search folder."
        : "Starting Kodi, then opening this folder.",
    };
  }
  return {
    title: "Opening " + label,
    detail: searchFolder ? "Opening this search folder from the add-on." : "Opening this folder from the add-on.",
  };
}

export function describeSearch(query: string, starting: boolean): ActivityInput {
  const q = clip(visibleText(query), 80);
  return {
    title: q ? "Searching for " + q : "Searching",
    detail: starting ? "Starting Kodi, then searching the add-on." : "Searching the add-on.",
  };
}

export function describeSource(starting: boolean): ActivityInput {
  return {
    title: "Loading add-ons",
    detail: starting ? "Starting Kodi, then reading installed video add-ons." : "Reading installed video add-ons.",
  };
}

export function describeSettings(starting: boolean): ActivityInput {
  return {
    title: "Loading settings",
    detail: starting ? "Starting Kodi, then reading this add-on's settings." : "Reading this add-on's settings.",
  };
}

export function describeRepositoryInstall(): ActivityInput {
  return { title: "Installing the repository", detail: "Downloading it from the link and writing the add-on files." };
}

export function describeAddonInstall(name: string): ActivityInput {
  const label = visibleText(name) || "this add-on";
  return { title: "Installing " + label, detail: "Downloading it from the repository and writing the files." };
}

export function describeRemove(kind: "repository" | "video", name: string): ActivityInput {
  const label = visibleText(name) || (kind === "repository" ? "the repository" : "the add-on");
  return {
    title: "Removing " + label,
    detail:
      kind === "repository"
        ? "Deleting the repository from this PC."
        : "Deleting the add-on and its settings from this PC.",
  };
}

export function describeRepositoryOpen(name: string): ActivityInput {
  const label = visibleText(name) || "this repository";
  return { title: "Opening " + label, detail: "Reading its video add-ons." };
}

export function describeAction(name?: string): ActivityInput {
  const label = visibleText(name || "");
  return {
    title: label ? "Running " + label : "Running that action",
    detail: "Asking the add-on to run this action.",
  };
}

export function describeServer(action: "restart" | "quit"): ActivityInput {
  if (action === "quit") return { title: "Stopping Kodi", detail: "Asking Kodi to quit." };
  return { title: "Restarting Kodi", detail: "Stopping Kodi and starting it again." };
}

export function describeLibrary(kind: "home" | "title" | "watchlist" | "bookmarks" | "history"): ActivityInput {
  if (kind === "home") return { title: "Home", detail: "Loading your library." };
  if (kind === "title") return { title: "Title", detail: "Loading this title." };
  if (kind === "watchlist") return { title: "Watchlist", detail: "Loading the watchlist." };
  if (kind === "bookmarks") return { title: "Bookmarks", detail: "Loading bookmarks." };
  return { title: "History", detail: "Loading history." };
}

function paint(input: ActivityInput, options?: ActivityOptions): void {
  const root = document.getElementById("activity");
  const title = document.getElementById("activity-title");
  const detail = document.getElementById("activity-detail");
  const percent = document.getElementById("activity-percent");
  const meter = document.getElementById("activity-meter");
  const fill = document.getElementById("activity-fill");
  const cancel = document.getElementById("activity-cancel");
  if (!root || !title || !detail || !percent || !meter || !fill || !(cancel instanceof HTMLButtonElement)) return;
  const view = activityView(input);
  if (title.textContent !== view.title) title.textContent = view.title;
  if (detail.textContent !== view.detail) detail.textContent = view.detail;
  if (view.percent == null) {
    percent.hidden = true;
    percent.textContent = "";
    fill.style.removeProperty("width");
    meter.removeAttribute("aria-valuenow");
    meter.setAttribute("aria-busy", "true");
  } else {
    const label = view.percent + "%";
    percent.hidden = false;
    if (percent.textContent !== label) percent.textContent = label;
    fill.style.width = label;
    meter.setAttribute("aria-valuenow", String(view.percent));
    meter.setAttribute("aria-busy", "false");
  }
  if (meter.dataset.mode !== view.meter) {
    meter.dataset.mode = view.meter;
    meter.classList.toggle("indeterminate", view.meter === "indeterminate");
    meter.classList.toggle("determinate", view.meter === "determinate");
  }
  meter.setAttribute("aria-label", view.title);
  const showCancel = !!options?.onCancel || !!options?.cancelDisabled;
  cancel.hidden = !showCancel;
  cancel.disabled = !options?.onCancel || !!options?.cancelDisabled;
  cancel.onclick = options?.onCancel ? () => options.onCancel?.() : null;
  root.hidden = false;
}

function visibleText(text: string): string {
  return safeCopy(text.replace(/\s+/g, " ").trim());
}

function visibleDetail(text: string): string {
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.replace(/[ \t]+/g, " ").trim())
    .filter(Boolean);
  return safeCopy(lines.join("\n"));
}

function safeCopy(value: string): string {
  if (!value) return "";
  const flat = value.toLowerCase();
  if (flat.includes("plugin://")) return "";
  if (flat.includes("token") || flat.includes("password") || flat.includes("api key") || flat.includes("apikey")) return "";
  if (/[a-z][a-z0-9+.-]*:\/\/[^\s]*@/i.test(value)) return "";
  return value;
}

function clip(text: string, max: number): string {
  if (text.length <= max) return text;
  return text.slice(0, max - 1).trimEnd() + "…";
}
