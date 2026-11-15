/** Clock text for the player bar. Hours appear only after the first hour. */
export function formatClock(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
  const whole = Math.floor(seconds);
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const secs = whole % 60;
  const ss = String(secs).padStart(2, "0");
  if (hours > 0) return hours + ":" + String(minutes).padStart(2, "0") + ":" + ss;
  return minutes + ":" + ss;
}

/** What a screen reader says for the seek bar, "12:34 of 45:00", rather than a count of seconds. */
export function seekValueText(position: number, duration: number): string {
  if (!Number.isFinite(duration) || duration <= 0) return formatClock(position);
  return formatClock(position) + " of " + formatClock(duration);
}

/** A skip target on the full timeline, including the first minute. */
export function clampSkip(seconds: number, duration: number): number {
  if (!Number.isFinite(seconds) || seconds < 0) return 0;
  const at = Math.floor(seconds);
  if (Number.isFinite(duration) && duration > 0) return Math.min(at, Math.floor(duration));
  return at;
}
