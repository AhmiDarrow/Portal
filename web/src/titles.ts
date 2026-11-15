/** Episode titles as the player, the TV page, and the library show them. */

export function episodeCode(season?: number | null, episode?: number | null): string {
  // Season 0 is specials: a real season.
  if (season == null || !episode) return "";
  return "S" + String(season).padStart(2, "0") + "E" + String(episode).padStart(2, "0");
}

/** Drop a leading show name, then a leading S01E05 / (S01E05) : / 1x05. / Season N Episode M. */
export function cleanEpisodeTitle(
  name: string,
  season?: number | null,
  episode?: number | null,
  show?: string | null,
): string {
  let text = name.trim();
  if (!text) return "";
  const series = show?.trim();
  if (series && text.toLowerCase().startsWith(series.toLowerCase()) && text.length > series.length) {
    text = text.slice(series.length).replace(/^[\s:.\-–—·|]+/, "");
  }
  text = text.replace(/^\(\s*S?\d{1,2}\s*[Ex]\s*\d{1,3}\s*\)\s*[:.\-–—·|]?\s*/i, "");
  text = text.replace(/^S\d{1,2}E\d{1,3}\s*[:.\-–—·|]?\s*/i, "");
  text = text.replace(/^\d{1,2}\s*[x×]\s*\d{1,3}\s*[.:,\-–—·|]?\s*/i, "");
  text = text.replace(/^Season\s+\d+\s+Episode\s+\d+\s*[:.\-–—]?\s*/i, "");
  const code = episodeCode(season, episode);
  if (code) {
    const wrapped = new RegExp("^\\(\\s*" + code + "\\s*\\)\\s*[:.\\-–—]?\\s*", "i");
    text = text.replace(wrapped, "");
    if (text.toUpperCase().startsWith(code)) {
      text = text.slice(code.length).replace(/^[\s:.\-–—·|]+/, "");
    }
  }
  return text.trim();
}

/** "S01E05 · Name", or whichever half is known. */
export function displayMediaTitle(
  raw: string,
  season?: number | null,
  episode?: number | null,
  show?: string | null,
): string {
  const cleaned = cleanEpisodeTitle(raw, season, episode, show);
  const code = episodeCode(season, episode);
  if (code && cleaned) return code + " · " + cleaned;
  if (code && !cleaned) return code;
  return cleaned || raw.trim();
}

/** The show on top and the episode below it; a movie is one line. */
export function splitTitle(
  raw: string,
  season?: number | null,
  episode?: number | null,
  show?: string | null,
): { heading: string; detail: string } {
  const series = show?.trim() || "";
  if (season != null && episode) {
    const described = displayMediaTitle(raw, season, episode, series);
    return series ? { heading: series, detail: described } : { heading: described, detail: "" };
  }
  return { heading: raw.trim(), detail: "" };
}
