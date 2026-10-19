/** "12 s", "4 m", "2 h", "3 d": one coarse unit, for ages. */
export function formatAge(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} m`;
  if (s < 86400) return `${Math.floor(s / 3600)} h`;
  return `${Math.floor(s / 86400)} d`;
}

/** "4 m 12 s", "2 m", "2 h 5 m", "38 s": exact durations without zero tails. */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) return `${s} s`;
  if (s < 3600) return s % 60 ? `${Math.floor(s / 60)} m ${s % 60} s` : `${s / 60} m`;
  const minutes = Math.floor((s % 3600) / 60);
  return minutes ? `${Math.floor(s / 3600)} h ${minutes} m` : `${s / 3600} h`;
}

/** Relative time for a unix timestamp; "yesterday" reads better than "1 d ago". */
export function formatWhen(unix: number, nowMs: number): string {
  const seconds = Math.floor(nowMs / 1000) - unix;
  if (seconds < 60) return "just now";
  if (seconds >= 86400 && seconds < 2 * 86400) return "yesterday";
  return `${formatAge(seconds)} ago`;
}

const clock = new Intl.DateTimeFormat("en-GB", { hour: "2-digit", minute: "2-digit", hour12: false });
const calendar = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short" });

const startOfDay = (date: Date) => new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();

/** "Today 03:00", "Yesterday 03:00", "17 Oct 03:00". */
export function formatDayTime(unix: number, nowMs: number): string {
  const date = new Date(unix * 1000);
  const days = Math.round((startOfDay(new Date(nowMs)) - startOfDay(date)) / 86_400_000);
  const time = clock.format(date);
  if (days === 0) return `Today ${time}`;
  if (days === 1) return `Yesterday ${time}`;
  return `${calendar.format(date)} ${time}`;
}

export const shortRevision = (value: string | null | undefined): string =>
  value ? value.slice(0, 12) : "—";

export const formatPercent = (value: number): string => `${Math.round(value)} %`;
