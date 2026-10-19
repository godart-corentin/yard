/** Parsing of raw `docker compose logs` lines. Only what is really present is extracted. */

export type Level = "info" | "warn" | "error";

export interface ParsedLine {
  /** Text without ANSI escapes and without the compose `name |` prefix. */
  message: string;
  /** Compose service, when the line carries the `<project>-<service>-<n> |` prefix. */
  service: string | null;
  /** `HH:MM:SS` as printed by the application, when the message starts with a timestamp. */
  time: string | null;
  /** Full timestamp usable for `since` (only when it carries a time zone). */
  sinceStamp: string | null;
  level: Level | null;
}

// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;?]*[ -/]*[@-~]/g;
const PREFIX = /^([A-Za-z0-9][A-Za-z0-9_.-]*)\s+\|\s?/;
const STAMP = /^\[?(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2}:\d{2})([.,]\d+)?(Z|[+-]\d{2}:?\d{2})?\]?\s*/;
const LEVEL_KEY = /\blevel[=:]\s*"?(\w+)/i;
const LEVEL_LEAD = /^[[(<]?(error|err|fatal|critical|panic|warn|warning|info|debug|trace)\b/i;

export function stripAnsi(text: string): string {
  return text.replace(ANSI, "");
}

function levelOf(word: string): Level | null {
  switch (word.toLowerCase()) {
    case "error":
    case "err":
    case "fatal":
    case "critical":
    case "panic":
      return "error";
    case "warn":
    case "warning":
      return "warn";
    case "info":
      return "info";
    default:
      // debug/trace and unknown words are not coloured.
      return null;
  }
}

function serviceName(container: string, project: string): string {
  let name = container.replace(/-\d+$/, "");
  if (name.startsWith(`${project}-`)) name = name.slice(project.length + 1);
  return name;
}

export function parseLine(raw: string, project: string): ParsedLine {
  let text = stripAnsi(raw);
  let service: string | null = null;
  const prefix = PREFIX.exec(text);
  if (prefix) {
    service = serviceName(prefix[1] ?? "", project);
    text = text.slice(prefix[0].length);
  }
  let time: string | null = null;
  let sinceStamp: string | null = null;
  const stamp = STAMP.exec(text);
  if (stamp) {
    time = stamp[2] ?? null;
    if (stamp[4]) sinceStamp = `${stamp[1]}T${stamp[2]}${(stamp[3] ?? "").replace(",", ".")}${stamp[4]}`;
    text = text.slice(stamp[0].length);
  }
  const word = LEVEL_KEY.exec(text)?.[1] ?? LEVEL_LEAD.exec(text)?.[1];
  return { message: text, service, time, sinceStamp, level: word ? levelOf(word) : null };
}

/** Same rule as the server (`valid_since`): empty means no `since`. */
export function validSince(value: string): boolean {
  return value === "" || (value.length <= 40 && /^[0-9TZ+\-:.smhd]+$/.test(value));
}
