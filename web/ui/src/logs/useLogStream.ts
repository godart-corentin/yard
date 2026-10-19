import { useCallback, useEffect, useRef, useState } from "react";

import { logsStreamUrl } from "../api/client";
import { parseLine, type ParsedLine } from "./parse";

/** Ring buffer size: older lines are dropped. */
export const MAX_LINES = 5000;
const FLUSH_MS = 30;
const BACKOFF_MS = [1000, 2000, 4000, 8000, 15000, 30000];

export interface LogEntry extends ParsedLine {
  id: number;
  /** Raw text as sent by the server (no ANSI stripping), for download. */
  raw: string;
  /** Local notes (reconnects) are not log output. */
  note?: boolean;
}

export type StreamState =
  | { kind: "connecting" }
  | { kind: "live" }
  | { kind: "paused" }
  | { kind: "reconnecting" }
  | { kind: "error"; status: 400 | 429 | 503 | null };

interface Options {
  project: string;
  service: string | null;
  /** Validated; empty or undefined means no `since`. */
  since: string;
  paused: boolean;
}

async function probeStatus(url: string, signal: AbortSignal): Promise<number | null> {
  try {
    const response = await fetch(url, { headers: { Accept: "application/json" }, signal, credentials: "same-origin" });
    void response.body?.cancel();
    return response.status;
  } catch {
    return null;
  }
}

/**
 * Follows `/api/logs/stream` with an `EventSource`. Reconnects with backoff on unexpected ends,
 * resumes from the last parseable timestamp (else tail), and keeps at most MAX_LINES lines.
 */
export function useLogStream({ project, service, since, paused }: Options) {
  const [lines, setLines] = useState<LogEntry[]>([]);
  const [state, setState] = useState<StreamState>({ kind: "connecting" });
  const [attempt, setAttempt] = useState(0);
  const buffer = useRef<LogEntry[]>([]);
  const pending = useRef<LogEntry[]>([]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const nextId = useRef(0);
  const lastStamp = useRef<string | null>(null);
  const everConnected = useRef(false);

  const flush = useCallback(() => {
    timer.current = null;
    if (pending.current.length === 0) return;
    buffer.current = [...buffer.current, ...pending.current].slice(-MAX_LINES);
    pending.current = [];
    setLines(buffer.current);
  }, []);

  const push = useCallback(
    (entries: LogEntry[]) => {
      pending.current.push(...entries);
      timer.current ??= setTimeout(flush, FLUSH_MS);
    },
    [flush],
  );

  // A different query is a different stream: start from an empty buffer.
  useEffect(() => {
    buffer.current = [];
    pending.current = [];
    lastStamp.current = null;
    everConnected.current = false;
    setLines([]);
    setState({ kind: "connecting" });
  }, [project, service, since]);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  useEffect(() => {
    if (paused) {
      setState({ kind: "paused" });
      return;
    }
    let disposed = false;
    let source: EventSource | null = null;
    let retry: ReturnType<typeof setTimeout> | null = null;
    const probe = new AbortController();
    let failures = 0;

    const note = (text: string) =>
      push([{ id: nextId.current++, raw: text, note: true, message: text, service: null, time: null, sinceStamp: null, level: null }]);

    const schedule = (delay: number) => {
      setState({ kind: "reconnecting" });
      retry = setTimeout(connect, delay);
    };
    const backoff = () => BACKOFF_MS[Math.min(failures++, BACKOFF_MS.length - 1)] ?? 30000;

    function connect() {
      if (disposed) return;
      const resumeFrom = lastStamp.current;
      const params = {
        project,
        service: service ?? undefined,
        since: resumeFrom ?? (since || undefined),
        tail: resumeFrom || since ? 1000 : 200,
      };
      if (everConnected.current) {
        note(
          resumeFrom
            ? `Reconnected, resuming after ${resumeFrom}`
            : "Reconnected without a timestamp to resume from: the last lines may appear twice",
        );
      }
      const url = logsStreamUrl(params);
      const es = new EventSource(url);
      source = es;
      let opened = false;
      const stop = () => {
        es.close();
        if (source === es) source = null;
      };
      es.addEventListener("open", () => {
        opened = true;
        everConnected.current = true;
        setState({ kind: "live" });
      });
      es.addEventListener("line", (event) => {
        let text: string;
        try {
          text = (JSON.parse((event as MessageEvent<string>).data) as { text: string }).text;
        } catch {
          return;
        }
        failures = 0;
        const parsed = parseLine(text, project);
        if (parsed.sinceStamp) lastStamp.current = parsed.sinceStamp;
        push([{ ...parsed, id: nextId.current++, raw: text }]);
      });
      es.addEventListener("end", (event) => {
        stop();
        let reason = "error";
        try {
          reason = (JSON.parse((event as MessageEvent<string>).data) as { reason: string }).reason;
        } catch {
          // Treated as an error end.
        }
        // `closed` is the server time limit: resume right away. `error` backs off.
        schedule(reason === "closed" ? 250 : backoff());
      });
      es.addEventListener("error", () => {
        stop();
        if (opened) {
          schedule(backoff());
          return;
        }
        // EventSource hides the HTTP status: ask once to tell a refusal from an outage.
        void probeStatus(url, probe.signal).then((status) => {
          if (disposed) return;
          if (status === 400 || status === 429 || status === 503) {
            setState({ kind: "error", status });
            // The executor may come back; a refused or over-limit request will not fix itself.
            if (status === 503) retry = setTimeout(connect, backoff());
          } else {
            schedule(backoff());
          }
        });
      });
    }

    setState((current) => (current.kind === "paused" ? { kind: "connecting" } : current));
    connect();
    return () => {
      disposed = true;
      source?.close();
      if (retry) clearTimeout(retry);
      probe.abort();
    };
  }, [project, service, since, paused, attempt, push]);

  const retryNow = useCallback(() => setAttempt((n) => n + 1), []);
  return { lines, state, retryNow };
}
