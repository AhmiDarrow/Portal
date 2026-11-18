/** The theme menu, mapped onto Portal's page tokens. */

export type ThemeId =
  | "portal"
  | "system"
  | "dark"
  | "neutral"
  | "light"
  | "forest"
  | "green"
  | "purple"
  | "alien"
  | "orange"
  | "cyan"
  | "deutan"
  | "protan"
  | "tritan";

type Kind = "dark" | "light";

type Palette = {
  id: Exclude<ThemeId, "portal" | "system">;
  name: string;
  kind: Kind;
  bg: string;
  raised: string;
  well: string;
  line: string;
  accent: string;
  text: string;
  muted: string;
  error: string;
};

const STORAGE_KEY = "portal-theme";

const VARS = ["--bg", "--raised", "--well", "--line", "--text", "--muted", "--gold", "--error", "--track"] as const;

function palette(
  id: Palette["id"],
  name: string,
  kind: Kind,
  bg: string,
  raised: string,
  well: string,
  line: string,
  accent: string,
  text: string,
  muted: string,
  error: string,
): Palette {
  return { id, name, kind, bg, raised, well, line, accent, text, muted, error };
}

const PALETTES: Record<Palette["id"], Palette> = {
  dark: palette("dark", "Dark", "dark", "#0c0c14", "#14141f", "#1c1c2a", "#2a2a3d", "#8b5cf6", "#f0f0f5", "#a8a8c0", "#f87171"),
  neutral: palette("neutral", "Neutral Dark", "dark", "#0f1115", "#161a20", "#1e2430", "#2e3644", "#94a3b8", "#e2e8f0", "#94a3b8", "#f87171"),
  light: palette("light", "Light", "light", "#f4f4f8", "#ffffff", "#e8e8ef", "#a8a8b8", "#7c3aed", "#14141f", "#3d3d52", "#b91c1c"),
  forest: palette("forest", "Dark Forest", "dark", "#0a0e0b", "#121812", "#1a221c", "#2a352c", "#4d7a5a", "#e6ebe7", "#9aa89e", "#b87a7a"),
  green: palette("green", "Emerald", "dark", "#0a1410", "#0f1f18", "#163026", "#1f4a38", "#34d399", "#d8ebe2", "#8fb8a4", "#fca5a5"),
  purple: palette("purple", "Amethyst", "dark", "#100a16", "#1a1224", "#261a36", "#3d2a55", "#c084fc", "#e4dff0", "#b0a0c8", "#fca5a5"),
  alien: palette("alien", "Dark Purple", "dark", "#07040f", "#0f081a", "#1a0f2e", "#3b1d66", "#b026ff", "#f2e8ff", "#c4a0e8", "#ff6b9d"),
  orange: palette("orange", "Amber", "dark", "#14100a", "#1f1810", "#2e2418", "#4a3a24", "#fb923c", "#efe6da", "#c4a88a", "#fca5a5"),
  cyan: palette("cyan", "Ocean", "dark", "#0a1216", "#0f1c22", "#163038", "#1e4a55", "#22d3ee", "#d8eef2", "#8fb8c2", "#fca5a5"),
  // The next three keep the accent and the error colour far apart, in hue
  // and in brightness, for one kind of colour vision each: weak green (blue
  // against orange), weak red (blue against yellow, since red reads as dark)
  // and weak blue (teal against rose). Both colours hold at least 4.5:1
  // contrast on every surface.
  deutan: palette("deutan", "Azure", "dark", "#0b0f14", "#121821", "#1a2330", "#34465a", "#56b4e9", "#e9f0f6", "#a9b9c7", "#f5a13c"),
  protan: palette("protan", "Skyline", "dark", "#0d1013", "#151a1f", "#1e252c", "#384551", "#6cb6ff", "#eef1f4", "#adb8c2", "#ffc233"),
  tritan: palette("tritan", "Lagoon", "dark", "#12100f", "#1b1817", "#262120", "#453b39", "#2fd0c0", "#f3efee", "#c0b6b3", "#ff6f7d"),
};

export type ThemeChoice = {
  id: ThemeId;
  name: string;
  swatch: string;
};

/**
 * Portal stays the untouched page until another theme is chosen. The menu
 * lists System, Light, names that do not start with Dark, then Dark with
 * Neutral Dark directly under it.
 */
export const themeMenu: ThemeChoice[] = [
  { id: "portal", name: "Portal", swatch: "#e2b15a" },
  { id: "system", name: "System", swatch: "linear-gradient(90deg, #0c0c14 50%, #f4f4f8 50%)" },
  { id: "light", name: "Light", swatch: PALETTES.light.accent },
  { id: "orange", name: "Amber", swatch: PALETTES.orange.accent },
  { id: "purple", name: "Amethyst", swatch: PALETTES.purple.accent },
  { id: "deutan", name: "Azure", swatch: PALETTES.deutan.accent },
  { id: "green", name: "Emerald", swatch: PALETTES.green.accent },
  { id: "tritan", name: "Lagoon", swatch: PALETTES.tritan.accent },
  { id: "cyan", name: "Ocean", swatch: PALETTES.cyan.accent },
  { id: "protan", name: "Skyline", swatch: PALETTES.protan.accent },
  { id: "dark", name: "Dark", swatch: PALETTES.dark.accent },
  { id: "neutral", name: "Neutral Dark", swatch: PALETTES.neutral.accent },
  { id: "forest", name: "Dark Forest", swatch: PALETTES.forest.accent },
  { id: "alien", name: "Dark Purple", swatch: PALETTES.alien.accent },
];

const MENU_IDS = new Set<string>(themeMenu.map((item) => item.id));

function prefersLight(): boolean {
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

function resolve(id: ThemeId): Palette | null {
  if (id === "portal") return null;
  if (id === "system") return prefersLight() ? PALETTES.light : PALETTES.dark;
  return PALETTES[id];
}

function paint(id: ThemeId): void {
  const root = document.documentElement;
  const palette = resolve(id);
  if (!palette) {
    for (const name of VARS) root.style.removeProperty(name);
    root.style.colorScheme = "";
    root.removeAttribute("data-theme");
    root.dataset.themeChoice = "portal";
    paintBrowserChrome("#14120e", "dark");
    return;
  }
  root.dataset.theme = palette.id;
  root.dataset.themeChoice = id;
  root.style.colorScheme = palette.kind;
  root.style.setProperty("--bg", palette.bg);
  root.style.setProperty("--raised", palette.raised);
  root.style.setProperty("--well", palette.well);
  root.style.setProperty("--line", palette.line);
  root.style.setProperty("--text", palette.text);
  root.style.setProperty("--muted", palette.muted);
  root.style.setProperty("--gold", palette.accent);
  root.style.setProperty("--error", palette.error);
  root.style.setProperty("--track", palette.line);
  paintBrowserChrome(palette.bg, palette.kind);
}

function paintBrowserChrome(color: string, scheme: string): void {
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", color);
  document.querySelector('meta[name="color-scheme"]')?.setAttribute("content", scheme);
}

function storedChoice(): ThemeId {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && MENU_IDS.has(stored)) return stored as ThemeId;
  } catch {
    /* Private mode can reject localStorage. */
  }
  return "portal";
}

function remember(id: ThemeId): void {
  try {
    localStorage.setItem(STORAGE_KEY, id);
  } catch {
    /* The choice still applies for this page view. */
  }
}

let current: ThemeId = "portal";
/** The header's menu repaints its ticks when Settings changes the theme. */
let onChange: (() => void) | null = null;

/** The theme this browser uses now. */
export function currentTheme(): ThemeId {
  return current;
}

/** Changes and remembers the theme, as the header's Theme menu does. */
export function setTheme(id: ThemeId): void {
  if (!MENU_IDS.has(id)) return;
  current = id;
  remember(id);
  paint(id);
  onChange?.();
}

export function mountThemes(): void {
  current = storedChoice();
  const button = document.querySelector("#theme-button");
  const menu = document.querySelector("#theme-menu");
  if (!(button instanceof HTMLButtonElement) || !(menu instanceof HTMLElement)) {
    paint(current);
    return;
  }

  let focusIndex = Math.max(0, themeMenu.findIndex((item) => item.id === current));

  const choiceName = (id: ThemeId) => themeMenu.find((item) => item.id === id)?.name ?? "Portal";

  const mark = () => {
    button.textContent = "Theme";
    button.setAttribute("aria-label", `Theme, ${choiceName(current)}`);
    const options = menu.querySelectorAll<HTMLButtonElement>("[data-theme-id]");
    options.forEach((option, index) => {
      const selected = option.dataset.themeId === current;
      option.setAttribute("aria-selected", selected ? "true" : "false");
      option.classList.toggle("is-focus", index === focusIndex);
    });
  };

  const choose = (id: ThemeId) => {
    setTheme(id);
    close();
    button.focus();
  };

  const open = () => {
    focusIndex = Math.max(0, themeMenu.findIndex((item) => item.id === current));
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    mark();
    menu.querySelector<HTMLButtonElement>(`[data-theme-id="${current}"]`)?.focus();
  };

  const close = () => {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
  };

  menu.innerHTML = themeMenu
    .map(
      (item) => `
        <button type="button" class="theme-option" role="option" data-theme-id="${item.id}" aria-selected="false">
          <span class="swatch" style="background: ${item.swatch}"></span>
          <span>${item.name}${item.id === "system" ? " · OS" : ""}</span>
        </button>
      `,
    )
    .join("");

  paint(current);
  mark();
  onChange = mark;

  button.addEventListener("click", () => {
    if (menu.hidden) open();
    else close();
  });

  menu.addEventListener("click", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const option = target.closest<HTMLElement>("[data-theme-id]");
    const id = option?.dataset.themeId;
    if (!id || !MENU_IDS.has(id)) return;
    choose(id as ThemeId);
  });

  menu.addEventListener("mousemove", (event) => {
    const target = event.target;
    if (!(target instanceof Element)) return;
    const option = target.closest<HTMLElement>("[data-theme-id]");
    if (!option) return;
    const index = themeMenu.findIndex((item) => item.id === option.dataset.themeId);
    if (index < 0 || index === focusIndex) return;
    focusIndex = index;
    mark();
  });

  menu.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      close();
      button.focus();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      focusIndex = (focusIndex + 1) % themeMenu.length;
      mark();
      menu.querySelector<HTMLButtonElement>(`[data-theme-id="${themeMenu[focusIndex]?.id}"]`)?.focus();
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      focusIndex = (focusIndex - 1 + themeMenu.length) % themeMenu.length;
      mark();
      menu.querySelector<HTMLButtonElement>(`[data-theme-id="${themeMenu[focusIndex]?.id}"]`)?.focus();
      return;
    }
    if (event.key === "Home") {
      event.preventDefault();
      focusIndex = 0;
      mark();
      menu.querySelector<HTMLButtonElement>("[data-theme-id]")?.focus();
      return;
    }
    if (event.key === "End") {
      event.preventDefault();
      focusIndex = themeMenu.length - 1;
      mark();
      menu.querySelector<HTMLButtonElement>(`[data-theme-id="${themeMenu[focusIndex]?.id}"]`)?.focus();
    }
  });

  // Tab out of the menu closes it. A null relatedTarget (a click) is left to mousedown below.
  menu.addEventListener("focusout", (event) => {
    const next = event.relatedTarget;
    if (menu.hidden || !(next instanceof Node)) return;
    if (menu.contains(next) || button.contains(next)) return;
    close();
  });

  document.addEventListener("mousedown", (event) => {
    if (menu.hidden) return;
    const target = event.target;
    if (!(target instanceof Node)) return;
    if (menu.contains(target) || button.contains(target)) return;
    close();
  });

  const media = window.matchMedia("(prefers-color-scheme: light)");
  media.addEventListener("change", () => {
    if (current === "system") paint("system");
  });
}
