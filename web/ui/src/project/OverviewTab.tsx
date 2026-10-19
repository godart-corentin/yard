import type { ReactNode } from "react";

import { BackupButton } from "../actions/BackupButton";
import { RestoreButton } from "../actions/RestoreButton";
import type { ProjectStatus } from "../api";
import { Badge } from "../components/Badge";
import { ButtonLink } from "../components/Button";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { Table, Td, Th } from "../components/Table";
import { formatDayTime, formatWhen, shortRevision } from "../lib/format";
import { jobDetail, jobState, jobTitle } from "../lib/jobs";
import styles from "./ProjectLayout.module.css";
import { backupLine, isInvalidState, releaseName, serviceRows } from "./model";
import { useProject, useProjectJobs } from "./queries";

const Row = ({ label, children }: { label: string; children: ReactNode }) => (
  <div className={styles.kv}>
    <span>{label}</span>
    <span>{children}</span>
  </div>
);

function Services({ project }: { project: ProjectStatus }) {
  const rows = serviceRows(project);
  return (
    <Card as="section" aria-labelledby="svc-title">
      <CardHeader title="Services" titleId="svc-title">
        <span className={`muted ${styles.small}`}>Probes from the host snapshot</span>
      </CardHeader>
      {rows.length === 0 ? (
        <EmptyState>No application services declared.</EmptyState>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>Service</Th>
              <Th>Probe</Th>
              <Th>Measure</Th>
              <Th>Gate</Th>
              <Th>State</Th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.name}>
                <Td>
                  <strong>{row.name}</strong>
                  {row.image && (
                    <>
                      <br />
                      <span className={`muted mono ${styles.image}`}>{row.image}</span>
                    </>
                  )}
                </Td>
                <Td className="muted">{row.probe}</Td>
                <Td className="mono">{row.measure}</Td>
                <Td className="muted" title="The deployment gate setting is not reported by the API">
                  —
                </Td>
                <Td>
                  <Badge tone={row.state.tone}>{row.state.label}</Badge>
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </Card>
  );
}

function RecentJobs({ name, nowMs }: { name: string; nowMs: number }) {
  const jobs = useProjectJobs(name, 5);
  return (
    <Card as="section" aria-labelledby="recent-title">
      <CardHeader title="Recent jobs" titleId="recent-title">
        <ButtonLink size="sm" variant="ghost" to="/projects/$name/activity" params={{ name }}>
          All activity
        </ButtonLink>
      </CardHeader>
      {jobs.isPending ? (
        <EmptyState>Loading jobs…</EmptyState>
      ) : jobs.isError ? (
        <EmptyState>Jobs are not available right now.</EmptyState>
      ) : jobs.data.jobs.length === 0 ? (
        <EmptyState>No jobs yet for this project.</EmptyState>
      ) : (
        jobs.data.jobs.map((job) => {
          const state = jobState(job);
          const detail = jobDetail(job);
          return (
            <div key={job.id} className={styles.kv}>
              <span>
                <Badge tone={state.tone}>{state.label}</Badge> {jobTitle(job)}
                {detail && <span className="muted"> · {detail}</span>}
              </span>
              <span className="muted">{formatWhen(job.created_at_unix, nowMs)}</span>
            </div>
          );
        })
      )}
    </Card>
  );
}

function ReleaseCard({ project, nowMs }: { project: ProjectStatus; nowMs: number }) {
  const { release, previous_release: previous } = project;
  const invalid = isInvalidState(project);
  const current = releaseName(release);
  const previousPoint = project.restore_points.find((p) => p.position === "previous");
  return (
    <Card as="section" aria-labelledby="rel-title">
      <CardHeader title="Release" titleId="rel-title">
        <ButtonLink size="sm" variant="ghost" to="/projects/$name/releases" params={{ name: project.name }}>
          History
        </ButtonLink>
      </CardHeader>
      {invalid ? (
        <EmptyState>The release record is unreadable. Check the state file on the host.</EmptyState>
      ) : (
        <>
          <Row label="Current">
            <span className="mono">{current ?? "No release recorded"}</span>
          </Row>
          <Row label="Deployed">
            {release.deployed_at_unix != null ? formatDayTime(release.deployed_at_unix, nowMs) : "—"}
          </Row>
          <Row label="Previous">
            <span className="mono">{releaseName(previous) ?? "—"}</span>
          </Row>
          {project.pending_release.record_status === "recorded" && (
            <Row label="Pending">
              <Badge tone="warn">{project.pending_release.status ?? "interrupted"}</Badge>
            </Row>
          )}
          {previousPoint && (
            <div className={styles.kv}>
              <span>Restore previous release</span>
              <RestoreButton project={project.name} revision={previousPoint.id} />
            </div>
          )}
        </>
      )}
    </Card>
  );
}

function SourceCard({ project }: { project: ProjectStatus }) {
  const cli = project.cli;
  return (
    <Card as="section" aria-labelledby="git-title">
      <CardHeader title="Source" titleId="git-title" />
      {!cli ? (
        <EmptyState>No CLI diagnosis collected yet. Run yard status on the host.</EmptyState>
      ) : (
        <>
          <Row label="Branch">
            <span className="mono">{cli.branch ?? "—"}</span>
          </Row>
          <Row label="HEAD">
            <span className="mono">{cli.head ? shortRevision(cli.head) : "—"}</span>
          </Row>
          <Row label="Diagnosis">{cli.reason ?? cli.verdict}</Row>
          {cli.drift.length > 0 && <Row label="Drift">{cli.drift.join(", ")}</Row>}
        </>
      )}
    </Card>
  );
}

function BackupsCard({ project, nowMs }: { project: ProjectStatus; nowMs: number }) {
  const local = backupLine(project.last_backup, nowMs);
  const offsite = backupLine(project.last_offsite, nowMs);
  return (
    <Card as="section" aria-labelledby="bk-title">
      <CardHeader title="Backups" titleId="bk-title">
        <BackupButton project={project.name} />
      </CardHeader>
      <Row label="Local">
        <Badge tone={local.tone}>{local.label}</Badge> {local.when}
      </Row>
      <Row label="Offsite">
        {project.offsite_configured === false ? (
          <span className="muted">Not configured</span>
        ) : project.offsite_configured == null && !project.last_offsite ? (
          <span className="muted">Unknown</span>
        ) : (
          <>
            <Badge tone={offsite.tone}>{offsite.label}</Badge> {offsite.when}
          </>
        )}
      </Row>
    </Card>
  );
}

export function OverviewTab({ name }: { name: string }) {
  const { project } = useProject(name);
  if (!project || project.url_monitor) return null;
  const nowMs = Date.now();
  return (
    <div className={styles.cols}>
      <div className={`${styles.stack} ${styles.main}`}>
        <Services project={project} />
        <RecentJobs name={name} nowMs={nowMs} />
      </div>
      <div className={`${styles.stack} ${styles.side}`}>
        <ReleaseCard project={project} nowMs={nowMs} />
        <SourceCard project={project} />
        <BackupsCard project={project} nowMs={nowMs} />
      </div>
    </div>
  );
}
