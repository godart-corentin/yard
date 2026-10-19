import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { api, type ProjectStatus } from "../api";
import { useStatus } from "../api/queries";
import { formatWhen, shortRevision } from "../lib/format";
import { JobDialog } from "./JobDialog";
import styles from "./JobDialog.module.css";
import { formatBytes, parsePrunePlan } from "./prunePlan";

interface DialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function DeployDialog({ project, ...dialog }: DialogProps & { project: ProjectStatus }) {
  const branch = project.cli?.branch ?? null;
  const current = project.release.tag ?? project.release.revision;
  const services = project.application_services;
  return (
    <JobDialog
      {...dialog}
      kicker={`yard deploy ${project.name}`}
      title={`Deploy ${project.name}?`}
      confirmLabel="Deploy"
      request={{ action: "deploy", project: project.name }}
    >
      <div className={`${styles.box} ${styles.fromTo}`}>
        <span>
          <span className="muted">From</span>
          <br />
          <span className="mono">{current ? shortRevision(current) : "no release recorded"}</span>
        </span>
        <span aria-hidden="true">→</span>
        <span style={{ textAlign: "right" }}>
          <span className="muted">To</span>
          <br />
          <span className="mono">{branch ? `latest of ${branch}` : "latest of the tracked branch"}</span>
        </span>
      </div>
      <span className={styles.note}>Yard will run, in order:</span>
      <ol className={styles.steps}>
        {[
          <>Git update{branch && <> on <span className="mono">{branch}</span></>}</>,
          <>Backup (configured command)</>,
          <>Build images{services.length > 0 && ` ${services.join(", ")}`}</>,
          <>Run the migration service, if the project has one</>,
          <>Start, service gates, health check</>,
        ].map((step, index) => (
          <li key={index}>
            <span className={styles.num}>{index + 1}</span>
            <span>{step}</span>
          </li>
        ))}
      </ol>
      <p className={styles.note}>On failure, application images roll back automatically. Data is never restored.</p>
    </JobDialog>
  );
}

export function BackupDialog({ project, ...dialog }: DialogProps & { project: string }) {
  return (
    <JobDialog
      {...dialog}
      kicker={`yard backup ${project}`}
      title={`Back up ${project}?`}
      confirmLabel="Back up"
      request={{ action: "backup", project }}
    >
      <p className={styles.note}>
        Runs the project's configured backup command now. Backups can take a while; the output streams on the next page.
      </p>
    </JobDialog>
  );
}

export function RestoreDialog({ project, revision, ...dialog }: DialogProps & { project: string; revision: string }) {
  const status = useStatus();
  const [typed, setTyped] = useState("");
  const record = status.data?.projects.find((p) => p.name === project);
  const current = record?.release.tag ?? record?.release.revision ?? null;
  const point = record?.restore_points.find((p) => p.id === revision);
  return (
    <JobDialog
      {...dialog}
      onOpenChange={(open) => {
        if (!open) setTyped("");
        dialog.onOpenChange(open);
      }}
      kicker={`yard restore ${project} … --yes`}
      title={`Restore release ${shortRevision(revision)}?`}
      confirmLabel="Restore"
      variant="danger"
      ready={typed === project}
      request={{ action: "restore", project, revision, confirm: true }}
    >
      <div className={styles.box}>
        <div className={styles.row}>
          <span className="muted">Current</span>
          <span className="mono">{current ? shortRevision(current) : "unknown"}</span>
        </div>
        <div className={styles.row}>
          <span className="muted">Restore to</span>
          <span className="mono">{shortRevision(revision)}</span>
        </div>
        {point && (
          <div className={styles.row}>
            <span className="muted">Deployed</span>
            <span>{formatWhen(point.deployed_at_unix, Date.now())}</span>
          </div>
        )}
      </div>
      <div className={styles.warnbox}>
        Application images only. Database and volumes stay as they are: migrations already applied are not reverted.
        Service gates are bypassed.
      </div>
      <label className={styles.field}>
        <span>
          Type <strong className="mono">{project}</strong> to confirm
        </span>
        <input
          className={styles.input}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoComplete="off"
          spellCheck={false}
        />
      </label>
    </JobDialog>
  );
}

export function PruneDialog(dialog: DialogProps) {
  const plan = useQuery({
    queryKey: ["images-prune-plan"],
    queryFn: () => api.read({ op: "images-prune-plan" }),
    enabled: dialog.open,
    gcTime: 0,
    staleTime: 0,
    retry: false,
  });
  const output = plan.data?.output ?? "";
  const parsed = plan.data ? parsePrunePlan(output) : null;
  const count = parsed?.candidates.length ?? 0;
  const empty = parsed !== null && count === 0;
  return (
    <JobDialog
      {...dialog}
      kicker="yard images --prune  →  --prune --yes"
      title={parsed ? `Prune ${count} image revision${count === 1 ? "" : "s"}?` : "Prune unused image revisions?"}
      confirmLabel={parsed ? `Prune ${count} revision${count === 1 ? "" : "s"}` : "Prune images"}
      variant="danger"
      ready={plan.isSuccess && !empty}
      request={{ action: "images-prune", confirm: true }}
    >
      {plan.isPending && <p className={styles.note}>Running the simulation…</p>}
      {plan.isError && (
        <p className={styles.error} role="alert">
          The simulation could not run, so nothing can be pruned. Check that the read service is available, then reopen
          this dialog.
        </p>
      )}
      {plan.isSuccess && parsed && (
        <>
          <span className={styles.note}>Simulation result</span>
          <div className={styles.box}>
            {empty && <div className={styles.row}>Nothing to prune: no unused revision is eligible.</div>}
            {parsed.candidates.map((candidate) => (
              <div key={candidate.reference} className={styles.row}>
                <span className="mono">{candidate.reference}</span>
                <span className="muted">{formatBytes(candidate.bytes)}</span>
              </div>
            ))}
            <div className={styles.row}>
              <strong>Reclaimable (estimate)</strong>
              <strong>{formatBytes(parsed.reclaimableBytes)}</strong>
            </div>
          </div>
        </>
      )}
      {plan.isSuccess && !parsed && (
        <>
          <span className={styles.note}>Yard answered in a format this page does not recognize. Raw simulation output:</span>
          <div className={styles.box}>
            <pre className={styles.raw}>{output || "(empty)"}</pre>
          </div>
        </>
      )}
      <p className={styles.note}>
        Removes every eligible candidate, never a selection. Current and previous releases and running containers stay
        protected.
      </p>
    </JobDialog>
  );
}

export function HostCollectDialog(dialog: DialogProps) {
  return (
    <JobDialog
      {...dialog}
      kicker="yard host"
      title="Collect host metrics?"
      confirmLabel="Collect"
      request={{ action: "host-collect" }}
    >
      <p className={styles.note}>Takes a host snapshot now. It is quick and changes nothing on the host.</p>
    </JobDialog>
  );
}
