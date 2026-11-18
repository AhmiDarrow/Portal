/**
 * The typefaces the page is set in: one for text and one for headings. Like
 * the theme, the choice belongs to each browser (localStorage), so a phone
 * and a TV can read differently. Every face is built into Portal; a browser
 * downloads only the ones in use.
 */

export type FontId = "remedy-sans" | "remedy-sans-ui" | "ahmi-pixel" | "ahmi-print" | "system";

export type FontChoice = {
  id: FontId;
  name: string;
  /** The CSS font-family list, ending in the system faces as a fallback. */
  stack: string;
};

const SYSTEM = `"Segoe UI", system-ui, sans-serif`;

export const FONTS: FontChoice[] = [
  { id: "remedy-sans", name: "Remedy Sans", stack: `"Remedy Sans", ${SYSTEM}` },
  { id: "remedy-sans-ui", name: "Remedy Sans UI", stack: `"Remedy Sans UI", ${SYSTEM}` },
  { id: "ahmi-pixel", name: "Ahmi Pixel", stack: `"Ahmi Pixel", "Remedy Sans", ${SYSTEM}` },
  { id: "ahmi-print", name: "AhmiPrint", stack: `"AhmiPrint", Georgia, serif` },
  { id: "system", name: "System default", stack: SYSTEM },
];

export type Fonts = { text: FontId; headings: FontId };

export const DEFAULT_FONTS: Fonts = { text: "remedy-sans", headings: "ahmi-pixel" };

const KEY = "portal-fonts";
const IDS = new Set<string>(FONTS.map((font) => font.id));

function isFont(value: unknown): value is FontId {
  return typeof value === "string" && IDS.has(value);
}

function stack(id: FontId): string {
  return FONTS.find((font) => font.id === id)?.stack ?? SYSTEM;
}

function read(): Fonts {
  try {
    const parsed = JSON.parse(localStorage.getItem(KEY) ?? "null") as Record<string, unknown> | null;
    if (!parsed) return { ...DEFAULT_FONTS };
    return {
      text: isFont(parsed.text) ? parsed.text : DEFAULT_FONTS.text,
      headings: isFont(parsed.headings) ? parsed.headings : DEFAULT_FONTS.headings,
    };
  } catch {
    return { ...DEFAULT_FONTS };
  }
}

let current: Fonts = DEFAULT_FONTS;

function paint(): void {
  const root = document.documentElement;
  // The defaults are the stylesheet's own, so an untouched page keeps it.
  if (current.text === DEFAULT_FONTS.text) root.style.removeProperty("--ui");
  else root.style.setProperty("--ui", stack(current.text));
  if (current.headings === DEFAULT_FONTS.headings) root.style.removeProperty("--display");
  else root.style.setProperty("--display", stack(current.headings));
  // Ahmi Pixel is sharp only at multiples of 8px; other faces size freely.
  root.dataset.headingFont = current.headings;
}

export function fonts(): Fonts {
  return current;
}

export function setFonts(patch: Partial<Fonts>): void {
  current = { ...current, ...patch };
  try {
    localStorage.setItem(KEY, JSON.stringify(current));
  } catch {
    /* Private mode can reject localStorage. The choice still applies for this view. */
  }
  paint();
}

/** Sets the page in this browser's chosen fonts. Called once at start-up. */
export function applyFonts(): void {
  current = read();
  paint();
}
