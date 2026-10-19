import type { ProjectStatus, ServiceLevel } from "../api";
import { formatDayTime, formatDuration, shortRevision } from "../lib/format";
import { serviceTone, type Tone } from "../lib/tone";
import { visibleProbes } from "../overview/model";

export interface ServiceRow {
  name: string;
  image: string | null;
  probe: string;
  measure: string;
  state: { label: string; tone: Tone };
}

const LEVEL_LABEL: Record<ServiceLevel, string> = {
  healthy: "Healthy",
  degraded: "Degraded",
  unhealthy: "Unhealthy",
  unknown: "Unknown",
};

/** Application services plus any probed service, joined with the recorded image. Nothing is guessed. */
export function serviceRows(project: ProjectStatus): ServiceRow[] {
  const probes = visibleProbes(project);
  const names = [...new Set([...project.application_services, ...probes.map((p) => p.service)])];
  return names.map((name) => {
    const probe = probes.find((p) => p.service === name);
    const image = project.release.services.find((s) => s.name === name)?.image ?? null;
    if (!probe) {
      return { name, image, probe: "None", measure: "—", state: { label: "Unknown", tone: "unk" } };
    }
    let measure = "—";
    if (probe.kind === "heartbeat" && probe.age_seconds != null) {
      measure =
        probe.max_age_seconds != null
          ? `${formatDuration(probe.age_seconds)} / ${formatDuration(probe.max_age_seconds)}`
          : formatDuration(probe.age_seconds);
    } else if (probe.latency_ms != null) {
      measure = `${probe.latency_ms} ms`;
    }
    const kind = probe.kind === "http" ? "HTTP" : probe.kind === "heartbeat" ? "Heartbeat" : (probe.kind ?? "—");
    return {
      name,
      image,
      probe: kind,
      measure,
      state: { label: LEVEL_LABEL[probe.status] ?? "Unknown", tone: serviceTone(probe.status) },
    };
  });
}

export const isInvalidState = (project: ProjectStatus): boolean =>
  project.state_status === "invalid" ||
  project.release.record_status === "invalid" ||
  project.previous_release.record_status === "invalid";

export const releaseName = (r: ProjectStatus["release"]): string | null =>
  r.tag ?? (r.revision ? shortRevision(r.revision) : null);

export function projectSubtitle(project: ProjectStatus): string {
  if (project.url_monitor) {
    return ["URL monitor", project.latency_ms != null ? `health ${project.latency_ms} ms` : null]
      .filter(Boolean)
      .join(" · ");
  }
  return [
    "Compose project",
    project.cli?.branch ? `branch ${project.cli.branch}` : null,
    project.latency_ms != null ? `health ${project.latency_ms} ms` : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

export interface BackupLine {
  tone: Tone;
  label: string;
  when: string | null;
}

/** One backup record as a badge plus time; an unreadable record is shown as such. */
export function backupLine(record: ProjectStatus["last_backup"], nowMs: number): BackupLine {
  if (!record) return { tone: "unk", label: "None recorded", when: null };
  if (!("started_at_unix" in record)) return { tone: "bad", label: "Invalid record", when: null };
  const when = formatDayTime(record.started_at_unix, nowMs);
  return record.result === "failure"
    ? { tone: "bad", label: "Failed", when }
    : { tone: "ok", label: "OK", when };
}
