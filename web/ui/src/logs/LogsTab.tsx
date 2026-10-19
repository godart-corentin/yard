import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useStatus } from "../api/queries";
import { Badge } from "../components/Badge";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import type { Tone } from "../lib/tone";
import styles from "./LogsTab.module.css";
import { validSince } from "./parse";
import { MAX_LINES, useLogStream, type LogEntry, type StreamState } from "./useLogStream";

const DEFAULT_SINCE = "1h";
const SINCE_DEBOUNCE_MS = 400;
const BOTTOM_SLACK_PX = 24;

const LINE_CLASS = { info: styles.lvInfo, warn: styles.lvWarn, error: styles.lvError } as const;

const Row = memo(function Row({ entry, showTime, showService }: { entry: LogEntry; showTime: boolean; showService: boolean }) {
  if (entry.note) {
    return (
      <div className={`${styles.line} ${styles.note}`}>
        <span>{entry.message}</span>
      </div>
    );
  }
  return (
    <div className={styles.line}>
      {showTime && <span className={styles.ts}>{entry.time ?? ""}</span>}
      {showService && <span className={styles.svc}>{entry.service ?? ""}</span>}
      <span className={entry.level ? LINE_CLASS[entry.level] : undefined}>{entry.message}</span>
    </div>
  );
});

function badge(state: StreamState): { tone: Tone; label: string } {
  switch (state.kind) {
    case "live":
      return { tone: "run", label: "Live" };
    case "paused":
      return { tone: "unk", label: "Paused" };
    case "connecting":
      return { tone: "unk", label: "Connecting" };
    case "reconnecting":
      return { tone: "warn", label: "Reconnecting" };
    case "error":
      return { tone: "bad", label: state.status === 503 ? "Unavailable" : "Not streaming" };
  }
}

function errorText(status: 400 | 429 | 503 | null): string {
  switch (status) {
    case 400:
      return "The request was refused. Check the service and the Since value, or that the project configuration is valid.";
    case 429:
      return "Too many live log streams are open on this Web instance (limit 4). Close another logs tab, then retry.";
    case 503:
      return "The host executor is unavailable, so live logs cannot start. Retrying automatically.";
    default:
      return "The log stream could not be started.";
  }
}

function download(project: string, service: string | null, lines: LogEntry[]) {
  const text = lines.filter((line) => !line.note).map((line) => line.raw).join("\n") + "\n";
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = `${project}${service ? `-${service}` : ""}-logs.txt`;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}

export function LogsTab({ project }: { project: string }) {
  const status = useStatus();
  const services = useMemo(
    () => status.data?.projects.find((item) => item.name === project)?.application_services ?? [],
    [status.data, project],
  );
  const [service, setService] = useState<string | null>(null);
  const [sinceText, setSinceText] = useState(DEFAULT_SINCE);
  const [since, setSince] = useState(DEFAULT_SINCE);
  const [filter, setFilter] = useState("");
  const [paused, setPaused] = useState(false);
  const sinceValid = validSince(sinceText.trim());

  useEffect(() => {
    setService(null);
  }, [project]);

  useEffect(() => {
    const value = sinceText.trim();
    if (!validSince(value)) return;
    const id = setTimeout(() => setSince(value), SINCE_DEBOUNCE_MS);
    return () => clearTimeout(id);
  }, [sinceText]);

  const { lines, state, retryNow } = useLogStream({ project, service, since, paused });

  const needle = filter.trim().toLowerCase();
  const visible = useMemo(
    () => (needle ? lines.filter((line) => !line.note && line.message.toLowerCase().includes(needle)) : lines),
    [lines, needle],
  );
  const showTime = useMemo(() => lines.some((line) => line.time), [lines]);
  const showService = service === null && lines.some((line) => line.service);

  // Follow the end only while the reader is at the end.
  const term = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  const [away, setAway] = useState(false);
  const onScroll = useCallback(() => {
    const el = term.current;
    if (!el) return;
    atBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight <= BOTTOM_SLACK_PX;
    setAway(!atBottom.current);
  }, []);
  useLayoutEffect(() => {
    const el = term.current;
    if (el && atBottom.current) el.scrollTop = el.scrollHeight;
  }, [visible]);
  const jump = () => {
    const el = term.current;
    if (el) el.scrollTop = el.scrollHeight;
    atBottom.current = true;
    setAway(false);
  };

  const { tone, label } = badge(state);
  const command = `yard logs ${project} --follow${service ? ` --service ${service}` : ""}${since ? ` --since ${since}` : ""}`;
  const options: Array<[string | null, string]> = [[null, "All"], ...services.map((name): [string, string] => [name, name])];

  return (
    <>
      <Card as="section" aria-label="Log stream">
        <div className={styles.toolbar}>
          <div className={styles.field}>
            <span id="logs-service">Service</span>
            <div className={styles.seg} role="group" aria-labelledby="logs-service">
              {options.map(([key, text]) => (
                <button
                  key={key ?? "all"}
                  type="button"
                  className={`${styles.segbtn} ${key === service ? styles.on : ""}`}
                  aria-pressed={key === service}
                  onClick={() => setService(key)}
                >
                  {text}
                </button>
              ))}
            </div>
          </div>
          <label className={styles.field}>
            Since
            <input
              className={styles.input}
              style={{ width: 110 }}
              value={sinceText}
              onChange={(event) => setSinceText(event.target.value)}
              placeholder="e.g. 1h"
              aria-invalid={!sinceValid}
              aria-describedby={sinceValid ? undefined : "logs-since-error"}
              spellCheck={false}
            />
          </label>
          <label className={styles.field}>
            Filter
            <input
              className={styles.input}
              style={{ width: 200 }}
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
              placeholder="Text in line"
              spellCheck={false}
            />
          </label>
          <div className={styles.actions}>
            <Badge tone={tone}>{label}</Badge>
            <Button size="sm" onClick={() => setPaused((value) => !value)}>
              {paused ? "Resume" : "Pause"}
            </Button>
            <Button size="sm" onClick={() => download(project, service, lines)} disabled={lines.length === 0}>
              Download
            </Button>
          </div>
        </div>
        {!sinceValid && (
          <p id="logs-since-error" className={styles.fieldError} role="alert">
            Since accepts a duration like 10m, 1h or 2d, or a timestamp like 2026-10-19T10:00:00Z (digits and T Z + - : . s m h d, up to 40
            characters).
          </p>
        )}
        <div className={styles.termWrap}>
          <div className={styles.term} role="log" aria-live="off" aria-label="Log output" ref={term} onScroll={onScroll}>
            {state.kind === "error" && (
              <div className={styles.problem} role="alert">
                <span>{errorText(state.status)}</span>
                {state.status !== 503 && (
                  <Button size="sm" onClick={retryNow}>
                    Retry
                  </Button>
                )}
              </div>
            )}
            {visible.map((entry) => (
              <Row key={entry.id} entry={entry} showTime={showTime} showService={showService} />
            ))}
            {lines.length > 0 && visible.length === 0 && (
              <div className={`${styles.line} ${styles.note}`}>No buffered line matches the filter.</div>
            )}
            {state.kind === "live" && (
              <div className={styles.line}>
                <span className={styles.waiting}>{lines.length === 0 ? "▍ waiting for lines…" : "▍ waiting for new lines…"}</span>
              </div>
            )}
          </div>
          {away && (
            <Button size="sm" className={styles.jump} onClick={jump}>
              Jump to latest
            </Button>
          )}
        </div>
      </Card>
      <p className={styles.foot}>
        Streams <span className={styles.mono}>{command}</span> · keeps the last {MAX_LINES.toLocaleString("en")} lines in the browser ·
        closing the tab stops the stream.
      </p>
    </>
  );
}
