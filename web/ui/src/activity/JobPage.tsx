import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Link } from "@tanstack/react-router";

import { YardApiError, type Job } from "../api";
import { useJob } from "../api/jobQueries";
import { Badge } from "../components/Badge";
import { Button, ButtonLink } from "../components/Button";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { formatDuration, shortRevision } from "../lib/format";
import { jobState, jobTitle } from "../lib/jobs";
import { useNow } from "../shell/useNow";
import styles from "./JobPage.module.css";
import { deriveSteps, progress } from "./steps";
import { useJobOutput, type StreamStatus } from "./useJobOutput";

const MAX_LINES = 2000;

/** Seconds a job has run, or took. */
export function jobSeconds(job: Job, nowMs: number): number | null {
  if (job.started_at_unix == null) return null;
  return (job.finished_at_unix ?? Math.floor(nowMs / 1000)) - job.started_at_unix;
}

function Check() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      <path d="m5 12 5 5 9-10" />
    </svg>
  );
}

function Terminal({ text }: { text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  const stick = useRef(true);
  const all = text.split("\n");
  const hidden = Math.max(0, all.length - MAX_LINES);
  const lines = hidden ? all.slice(hidden) : all;

  useLayoutEffect(() => {
    const node = ref.current;
    if (node && stick.current) node.scrollTop = node.scrollHeight;
  }, [text]);

  return (
    <pre
      ref={ref}
      className={styles.term}
      role="log"
      aria-label="Job output"
      tabIndex={0}
      onScroll={(event) => {
        const node = event.currentTarget;
        stick.current = node.scrollHeight - node.scrollTop - node.clientHeight < 40;
      }}
    >
      {hidden > 0 && <span className={styles.hint}>{`… ${hidden} earlier lines hidden (Copy copies everything)\n`}</span>}
      {lines.map((line, index) => (
        <span key={index} className={line.startsWith("✓") ? styles.ok : undefined}>
          {line}
          {index < lines.length - 1 ? "\n" : ""}
        </span>
      ))}
    </pre>
  );
}

const STREAM_LABEL: Record<StreamStatus, string> = {
  connecting: "Connecting",
  streaming: "Streaming",
  reconnecting: "Reconnecting",
  ended: "Complete",
  lost: "Disconnected",
};

export function JobPage({ jobId }: { jobId: string }) {
  const query = useJob(jobId);
  const stream = useJobOutput(jobId, query.isSuccess);
  const now = useNow();
  const [copied, setCopied] = useState(false);

  // The stream's state events are fresher than the polled query.
  const job = stream.job ?? query.data ?? null;
  const steps = useMemo(() => (job ? deriveSteps(job, stream.text) : []), [job, stream.text]);

  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(id);
  }, [copied]);

  if (!job) {
    const missing = query.error instanceof YardApiError && query.error.status === 404;
    return (
      <>
        <PageHeader title={`Job ${jobId}`} />
        <Card>
          <EmptyState>
            {query.isPending
              ? "Loading job…"
              : missing
                ? "This job does not exist (it may have been removed after 90 days)."
                : "The job could not be loaded."}{" "}
            <Link to="/activity">Back to Activity</Link>
          </EmptyState>
        </Card>
      </>
    );
  }

  const state = jobState(job);
  const seconds = jobSeconds(job, now);
  const running = job.state === "running" || job.state === "queued";
  const { done, total } = progress(steps);
  const title = jobTitle(job);
  const meta = [`Started by ${job.actor} from Web`, job.revision ? shortRevision(job.revision) : null];

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(stream.text);
      setCopied(true);
    } catch {
      // Clipboard access can be denied; the text stays selectable in the terminal.
    }
  };

  return (
    <>
      <div className={styles.head}>
        <div>
          <div className={styles.title}>
            <h1>{title}</h1>
            <Badge tone={state.tone}>{seconds != null ? `${state.label} · ${formatDuration(seconds)}` : state.label}</Badge>
          </div>
          <p className="muted" style={{ margin: "4px 0 0", fontSize: 13 }}>
            {meta[0]}
            {meta[1] && (
              <>
                {" · "}
                <span className="mono">{meta[1]}</span>
              </>
            )}
            {job.state === "failed" && job.error && <> · {job.error}</>}
          </p>
        </div>
        <div className={styles.tools}>
          {running && <span className={styles.note}>This job can't be cancelled once started.</span>}
          {job.project && (
            <ButtonLink size="sm" to="/projects/$name/overview" params={{ name: job.project }}>
              Open project
            </ButtonLink>
          )}
        </div>
      </div>

      <div
        className={styles.progress}
        role="progressbar"
        aria-label="Progress"
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={done}
        data-tone={job.state === "failed" ? "bad" : job.state === "succeeded" ? "ok" : "run"}
      >
        <span style={{ width: `${total ? (done / total) * 100 : 0}%` }} />
      </div>

      <div className={styles.cols}>
        <Card as="section" className={styles.steps} aria-labelledby="steps-title">
          <CardHeader title="Steps" titleId="steps-title">
            <span className="muted" style={{ fontSize: 12 }}>
              {done} of {total}
            </span>
          </CardHeader>
          <ol className={styles.stepsList}>
            {steps.map((step) => (
              <li key={step.label} className={styles.step} data-state={step.state}>
                <span className={styles.ico} data-state={step.state}>
                  {step.state === "done" && <Check />}
                  {step.state === "now" && <span className={styles.dot} />}
                  {step.state === "failed" && "!"}
                </span>
                <span>{step.label}</span>
                {step.note && <span className={styles.stepNote}>{step.note}</span>}
              </li>
            ))}
          </ol>
        </Card>

        <Card as="section" className={styles.output} aria-labelledby="out-title">
          <CardHeader title="Output" titleId="out-title">
            <div className={styles.headRight}>
              <Badge tone={stream.status === "ended" ? "unk" : stream.status === "streaming" ? "run" : "warn"}>
                {STREAM_LABEL[stream.status]}
              </Badge>
              <Button size="sm" onClick={() => void copy()}>
                {copied ? "Copied" : "Copy"}
              </Button>
            </div>
          </CardHeader>
          {stream.status === "lost" && (
            <p className={styles.retry} role="alert">
              The output stream was interrupted.{" "}
              <Button size="sm" onClick={stream.retry}>
                Reconnect
              </Button>
            </p>
          )}
          <Terminal text={stream.text} />
        </Card>
      </div>
    </>
  );
}
