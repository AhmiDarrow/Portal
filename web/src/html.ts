/**
 * The locale for dates, numbers and language names. The page is in English
 * (`<html lang="en">`), so they are too: the viewer's own English (en-GB keeps
 * its day-month order), or plain English when the browser prefers another
 * language, so no text comes out in a language the page does not declare.
 */
export const PAGE_LOCALE: string = (() => {
  const page = (document.documentElement.lang || "en").toLowerCase();
  const own = (navigator.languages ?? [navigator.language]).find(
    (tag) => typeof tag === "string" && tag.toLowerCase().split("-")[0] === page,
  );
  return own ?? page;
})();

/** Escapes text for use inside HTML markup and double-quoted attributes. */
export function esc(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Only http(s) addresses become links or images from online sources. */
export function webUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  const text = value.trim();
  if (!/^https?:\/\//i.test(text)) return null;
  try {
    const url = new URL(text);
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
}
