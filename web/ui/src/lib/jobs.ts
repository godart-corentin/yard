import type { Job, JobAction, JobState } from "../api";
import { formatDuration, shortRevision } from "./format";
import type { Tone } from "./tone";

const ACTION: Record<JobAction, string> = {
  deploy: "Deploy",
  backup: "Backup",
  restore: "Restore",
  "images-prune": "Prune images",
  "host-collect": "Collect host snapshot",
};

const STATE: Record<JobState, { label: string; tone: Tone }> = {
  queued: { label: "Queued", tone: "unk" },
  running: { label: "Running", tone: "run" },
  succeeded: { label: "Succeeded", tone: "ok" },
  failed: { label: "Failed", tone: "bad" },
};

export const jobTitle = (job: Job): string =>
  job.project ? `${ACTION[job.action] ?? job.action} ${job.project}` : (ACTION[job.action] ?? job.action);

export const jobState = (job: Job): { label: string; tone: Tone } =>
  STATE[job.state] ?? { label: job.state, tone: "unk" };

/** Revision, failure reason and run time, whichever the job has. */
export function jobDetail(job: Job): string {
  const parts: string[] = [];
  if (job.revision) parts.push(shortRevision(job.revision));
  if (job.state === "failed" && job.error) parts.push(job.error);
  if (job.started_at_unix != null && job.finished_at_unix != null) {
    parts.push(formatDuration(job.finished_at_unix - job.started_at_unix));
  }
  return parts.join(" · ");
}
