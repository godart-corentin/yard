import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";

import { jobOutputUrl, type Job, type OutputChunk } from "../api";

export type StreamStatus = "connecting" | "streaming" | "reconnecting" | "ended" | "lost";

const MAX_RETRIES = 6;

/**
 * Streams a job's output over SSE. `EventSource` resumes on its own after a network
 * blip (Last-Event-ID); when the browser gives up (HTTP error), we reopen at the last
 * `next_offset` ourselves. After `end` we never reconnect.
 */
export function useJobOutput(jobId: string, enabled: boolean) {
  const client = useQueryClient();
  const [text, setText] = useState("");
  const [job, setJob] = useState<Job | null>(null);
  const [status, setStatus] = useState<StreamStatus>("connecting");
  const [attempt, setAttempt] = useState(0);
  const offset = useRef(0);

  useEffect(() => {
    offset.current = 0;
    setText("");
    setJob(null);
    setStatus("connecting");
  }, [jobId]);

  useEffect(() => {
    if (!enabled) return;
    let source: EventSource | null = null;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let retries = 0;
    let closed = false;

    const open = () => {
      source = new EventSource(jobOutputUrl(jobId, offset.current));
      source.addEventListener("open", () => {
        retries = 0;
        setStatus("streaming");
      });
      source.addEventListener("state", (event) => setJob(JSON.parse((event as MessageEvent<string>).data) as Job));
      source.addEventListener("output", (event) => {
        const chunk = JSON.parse((event as MessageEvent<string>).data) as OutputChunk;
        // A replay after reconnecting may repeat bytes we already have.
        if (chunk.next_offset <= offset.current) return;
        offset.current = chunk.next_offset;
        setText((previous) => previous + chunk.text);
      });
      source.addEventListener("end", (event) => {
        closed = true;
        source?.close();
        setJob(JSON.parse((event as MessageEvent<string>).data) as Job);
        setStatus("ended");
        void client.invalidateQueries({ queryKey: ["jobs"] });
        void client.invalidateQueries({ queryKey: ["status"] });
      });
      source.addEventListener("error", () => {
        if (closed) return;
        if (source?.readyState === 2 /* CLOSED: the browser will not retry */) {
          source.close();
          if (retries >= MAX_RETRIES) {
            setStatus("lost");
            return;
          }
          retries += 1;
          setStatus("reconnecting");
          timer = setTimeout(open, Math.min(1000 * 2 ** retries, 15_000));
        } else {
          setStatus("reconnecting");
        }
      });
    };

    open();
    return () => {
      closed = true;
      source?.close();
      clearTimeout(timer);
    };
  }, [jobId, enabled, attempt, client]);

  /** Starts over from the current offset after the stream was given up on. */
  const retry = useCallback(() => {
    setStatus("connecting");
    setAttempt((n) => n + 1);
  }, []);

  return { text, job, status, retry };
}
