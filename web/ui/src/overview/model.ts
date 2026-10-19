import type { BackupRecord, ProjectStatus, ServiceHealth } from "../api";
import { formatAge, formatDayTime, formatDuration, shortRevision } from "../lib/format";
import { projectLevel, type Tone } from "../lib/tone";

/** The CLI reports unconfigured probes as Unknown rows; they are not worth showing. */
export const visibleProbes = (project: ProjectStatus): ServiceHealth[] =>
  project.services.filter((probe) => probe.message !== "No service probe configured");

export interface Attention {
  project: ProjectStatus;
  tone: "warn" | "bad";
  /** Label shown in the badge. */
  label: "Degraded" | "Down";
  /** The first unhealthy service, when one explains the status. */
  service: string | null;
  detail: string;
  /** Further unhealthy services not shown. */
  more: number;
}

export function probeDetail(probe: ServiceHealth): string {
  if (probe.kind === "heartbeat" && probe.age_seconds != null && probe.max_age_seconds != null) {
    return `heartbeat ${formatDuration(probe.age_seconds)} old — limit ${formatDuration(probe.max_age_seconds)}`;
  }
  if (probe.kind === "http" && probe.status === "degraded" && probe.latency_ms != null) {
    return `slow response — ${probe.latency_ms} ms`;
  }
  return probe.message ?? (probe.kind === "http" ? "HTTP check failed" : "Service check failed");
}

/** One banner per Degraded or Down project. Unknown is not an outage and gets none. */
export function attentionItems(projects: ProjectStatus[]): Attention[] {
  return projects.flatMap((project): Attention[] => {
    const level = projectLevel(project.status);
    if (level !== "degraded" && level !== "down") return [];
    const unhealthy = visibleProbes(project).filter(
      (probe) => probe.status === "degraded" || probe.status === "unhealthy",
    );
    const [first, ...rest] = unhealthy;
    const fallback =
      project.error ??
      (level === "down"
        ? project.http_status != null
          ? `HTTP ${project.http_status}`
          : "health check failed"
        : "reduced health");
    return [
      {
        project,
        tone: level === "down" ? "bad" : "warn",
        label: level === "down" ? "Down" : "Degraded",
        service: first?.service ?? null,
        detail: first ? probeDetail(first) : fallback,
        more: rest.length,
      },
    ];
  });
}

export interface Summary {
  total: number;
  deployable: number;
  monitors: number;
  operational: number;
  unknown: number;
  attention: ProjectStatus[];
  servicesHealthy: number;
  servicesTotal: number;
}

export function summarize(projects: ProjectStatus[]): Summary {
  const probes = projects.flatMap(visibleProbes);
  return {
    total: projects.length,
    monitors: projects.filter((p) => p.url_monitor).length,
    deployable: projects.filter((p) => !p.url_monitor).length,
    operational: projects.filter((p) => projectLevel(p.status) === "operational").length,
    unknown: projects.filter((p) => projectLevel(p.status) === "unknown").length,
    attention: projects.filter((p) => ["degraded", "down"].includes(projectLevel(p.status))),
    servicesHealthy: probes.filter((p) => p.status === "healthy").length,
    servicesTotal: probes.length,
  };
}

export const kindLabel = (project: ProjectStatus): string =>
  project.url_monitor ? "URL monitor" : project.cli?.branch ? `Compose · ${project.cli.branch}` : "Compose";

export interface ReleaseCell {
  tag: string;
  note: string;
}

export function releaseCell(project: ProjectStatus, nowMs: number): ReleaseCell {
  if (project.url_monitor) return { tag: "—", note: "not deployed by Yard" };
  const { release } = project;
  if (release.record_status === "invalid" || project.state_status === "invalid") {
    return { tag: "Unreadable record", note: "state file unreadable" };
  }
  const tag = release.tag ?? (release.revision ? shortRevision(release.revision) : null);
  if (!tag) return { tag: "—", note: "no release recorded" };
  const note =
    release.deployed_at_unix != null
      ? `deployed ${formatAge(nowMs / 1000 - release.deployed_at_unix)} ago`
      : "deploy time unknown";
  return { tag, note };
}

export interface BackupCell {
  text: string;
  failed: boolean;
}

/** Only the local backup; off-site detail belongs to the project page. */
export function backupCell(project: ProjectStatus, nowMs: number): BackupCell {
  if (project.url_monitor) return { text: "—", failed: false };
  const record: BackupRecord | null = project.last_backup;
  if (!record) return { text: "No backup recorded", failed: false };
  if (!("started_at_unix" in record)) return { text: "Invalid record", failed: true };
  const when = formatDayTime(record.started_at_unix, nowMs);
  return record.result === "failure" ? { text: `Failed · ${when}`, failed: true } : { text: when, failed: false };
}

export interface ServiceChip {
  name: string;
  tone: Tone;
}

/** Probe states when configured; otherwise the application services, with no claim about health. */
export function serviceChips(project: ProjectStatus): ServiceChip[] {
  const probes = visibleProbes(project);
  if (probes.length) {
    return probes.map((probe) => ({
      name: probe.service,
      tone: { healthy: "ok", degraded: "warn", unhealthy: "bad", unknown: "unk" }[probe.status] as Tone,
    }));
  }
  return project.application_services.map((name) => ({ name, tone: "unk" }));
}

/** Interrupted activations mean the recorded release and Docker may disagree. */
export const hasPendingRelease = (project: ProjectStatus): boolean =>
  project.pending_release.record_status === "recorded";
