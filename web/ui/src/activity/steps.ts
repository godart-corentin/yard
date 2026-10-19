import type { Job } from "../api";

export type StepState = "done" | "now" | "todo" | "failed";

export interface Step {
  label: string;
  state: StepState;
  /** Extra remark, e.g. a step that never ran. */
  note?: string;
}

interface StepDef {
  label: string;
  /** Text the CLI prints when the step completes (see src/deploy.rs). */
  marker: string;
  /** Not every project has this step. */
  optional?: boolean;
}

// Markers printed by `yard deploy`: "✓ Git update", "✓ Backup", "✓ Build",
// "✓ Migrations" (only with a migration service) and the final "Healthy: ...".
const DEPLOY: StepDef[] = [
  { label: "Git update", marker: "✓ Git update" },
  { label: "Backup", marker: "✓ Backup" },
  { label: "Build images", marker: "✓ Build" },
  { label: "Migrations", marker: "✓ Migrations", optional: true },
  { label: "Start, service gates, health check", marker: "Healthy:" },
];

const SINGLE_LABEL: Record<string, string> = {
  backup: "Run backup",
  restore: "Restore release",
  "images-prune": "Remove unused images",
  "host-collect": "Collect host snapshot",
};

/**
 * Steps of a job, derived from its output. Only `deploy` prints per-step markers;
 * the other actions are one step whose state follows the job.
 */
export function deriveSteps(job: Job, output: string): Step[] {
  if (job.action !== "deploy") {
    const state: StepState =
      job.state === "succeeded" ? "done" : job.state === "failed" ? "failed" : job.state === "running" ? "now" : "todo";
    return [{ label: SINGLE_LABEL[job.action] ?? job.action, state }];
  }

  const seen = DEPLOY.map((def) => output.includes(def.marker));
  // A later marker proves the earlier ones happened even if a line was missed.
  const last = seen.lastIndexOf(true);
  let active = true;
  return DEPLOY.map((def, index) => {
    const done = job.state === "succeeded" ? true : index <= last;
    if (done) {
      const skipped = def.optional && !seen[index];
      return { label: def.label, state: "done", note: skipped ? "not configured" : undefined } as Step;
    }
    if (active) {
      active = false;
      if (job.state === "failed") return { label: def.label, state: "failed" } as Step;
      if (job.state === "running") return { label: def.label, state: "now" } as Step;
    }
    return { label: def.label, state: "todo" } as Step;
  });
}

export function progress(steps: Step[]): { done: number; total: number } {
  return { done: steps.filter((s) => s.state === "done").length, total: steps.length };
}
