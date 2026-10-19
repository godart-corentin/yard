import { BackupButton } from "../actions/BackupButton";
import { RestoreButton } from "../actions/RestoreButton";
import type { ProjectStatus } from "../api";
import { Badge } from "../components/Badge";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { Table, Td, Th } from "../components/Table";
import { formatDayTime } from "../lib/format";
import type { Tone } from "../lib/tone";
import styles from "./ProjectLayout.module.css";
import { backupLine } from "./model";
import { useProject, useRead } from "./queries";

const POSITION: Record<string, string> = { current: "Current", previous: "Previous", pending: "Pending" };

function pointTone(status: string): Tone {
  switch (status) {
    case "active":
      return "ok";
    case "rolled_back":
    case "failed":
      return "bad";
    case "activating":
      return "warn";
    default:
      return "unk";
  }
}

function RestorePoints({ project, nowMs }: { project: ProjectStatus; nowMs: number }) {
  const invalid = project.state_status === "invalid";
  return (
    <Card as="section" aria-labelledby="rp-title">
      <CardHeader title="Restore points" titleId="rp-title">
        <span className={`muted ${styles.small}`}>Application releases only; restoring never touches data.</span>
      </CardHeader>
      {invalid ? (
        <EmptyState>The state record is unreadable, so restore points cannot be listed.</EmptyState>
      ) : project.restore_points.length === 0 ? (
        <EmptyState>No restore points recorded yet.</EmptyState>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>Release</Th>
              <Th>Position</Th>
              <Th>Status</Th>
              <Th>Deployed</Th>
              <Th align="right">Action</Th>
            </tr>
          </thead>
          <tbody>
            {project.restore_points.map((point) => (
              <tr key={`${point.position}-${point.id}`}>
                <Td className="mono">{point.id}</Td>
                <Td className="muted">{POSITION[point.position] ?? point.position}</Td>
                <Td>
                  <Badge tone={pointTone(point.status)}>{point.status}</Badge>
                </Td>
                <Td className="muted">{formatDayTime(point.deployed_at_unix, nowMs)}</Td>
                <Td align="right">
                  {point.position === "current" ? (
                    <span className={`muted ${styles.small}`}>Running now</span>
                  ) : (
                    <RestoreButton project={project.name} revision={point.id} />
                  )}
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
      <div className={styles.note}>
        Manual restore bypasses service deployment gates (emergency recovery). The health URL check still runs.
      </div>
    </Card>
  );
}

function Attempts({ project, nowMs }: { project: ProjectStatus; nowMs: number }) {
  const rows = [
    { kind: "Local", record: project.last_backup, line: backupLine(project.last_backup, nowMs) },
    ...(project.offsite_configured === false
      ? []
      : [{ kind: "Offsite", record: project.last_offsite, line: backupLine(project.last_offsite, nowMs) }]),
  ];
  return (
    <Card as="section" aria-labelledby="bk-title" className={styles.half}>
      <CardHeader title="Backup attempts" titleId="bk-title">
        <BackupButton project={project.name} />
      </CardHeader>
      <Table>
        <thead>
          <tr>
            <Th>Started</Th>
            <Th>Kind</Th>
            <Th>Result</Th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ kind, line }) => (
            <tr key={kind}>
              <Td className="muted">{line.when ?? "—"}</Td>
              <Td>{kind}</Td>
              <Td>
                <Badge tone={line.tone}>{line.label}</Badge>
              </Td>
            </tr>
          ))}
        </tbody>
      </Table>
      <div className={styles.note}>Only the latest attempt of each kind is reported. Data recovery stays manual.</div>
    </Card>
  );
}

function RestoreJournal({ name }: { name: string }) {
  const log = useRead("restore-log", name);
  const text = log.data?.output.trim() ?? "";
  return (
    <Card as="section" aria-labelledby="rl-title" className={styles.half}>
      <CardHeader title="Restore journal" titleId="rl-title">
        <span className={`muted ${styles.small}`}>yard restore-log</span>
      </CardHeader>
      {log.isPending ? (
        <EmptyState>Loading restore journal…</EmptyState>
      ) : log.isError ? (
        <EmptyState>The restore journal is not available right now.</EmptyState>
      ) : text === "" ? (
        <EmptyState>No restores recorded.</EmptyState>
      ) : (
        <pre className={`mono ${styles.log}`} aria-label="Restore journal output">
          {text}
        </pre>
      )}
    </Card>
  );
}

export function ReleasesTab({ name }: { name: string }) {
  const { project } = useProject(name);
  if (!project || project.url_monitor) return null;
  const nowMs = Date.now();
  return (
    <div className={styles.stack}>
      <RestorePoints project={project} nowMs={nowMs} />
      <div className={styles.cols}>
        <Attempts project={project} nowMs={nowMs} />
        <RestoreJournal name={name} />
      </div>
    </div>
  );
}
