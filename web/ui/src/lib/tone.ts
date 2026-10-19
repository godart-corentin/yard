import type { HostLevel, ServiceLevel } from "../api";

/** Visual severity shared by Badge and StatusDot. */
export type Tone = "ok" | "warn" | "bad" | "unk" | "run";

/**
 * The server already combines CLI diagnosis, Web probes and missing
 * measurements into `project.status`; the UI never recomputes it. Anything
 * unexpected is Unknown, and Unknown is deliberately not Down.
 */
export type ProjectLevel = "operational" | "degraded" | "down" | "unknown";

export function projectLevel(status: string): ProjectLevel {
  switch (status) {
    case "operational":
    case "healthy":
      return "operational";
    case "degraded":
      return "degraded";
    case "down":
    case "unhealthy":
      return "down";
    default:
      return "unknown";
  }
}

const PROJECT_TONE: Record<ProjectLevel, Tone> = {
  operational: "ok",
  degraded: "warn",
  down: "bad",
  unknown: "unk",
};

const PROJECT_LABEL: Record<ProjectLevel, string> = {
  operational: "Operational",
  degraded: "Degraded",
  down: "Down",
  unknown: "Unknown",
};

export const projectTone = (status: string): Tone => PROJECT_TONE[projectLevel(status)];
export const projectLabel = (status: string): string => PROJECT_LABEL[projectLevel(status)];

const SERVICE_TONE: Record<ServiceLevel, Tone> = {
  healthy: "ok",
  degraded: "warn",
  unhealthy: "bad",
  unknown: "unk",
};

export const serviceTone = (level: ServiceLevel): Tone => SERVICE_TONE[level] ?? "unk";

const HOST_TONE: Record<HostLevel, Tone> = { normal: "ok", warning: "warn", critical: "bad", unknown: "unk" };
const HOST_LABEL: Record<HostLevel, string> = {
  normal: "Healthy",
  warning: "Warning",
  critical: "Critical",
  unknown: "Unknown",
};
const HOST_RANK: Record<HostLevel, number> = { normal: 0, unknown: 1, warning: 2, critical: 3 };

export const hostTone = (level: HostLevel): Tone => HOST_TONE[level] ?? "unk";
export const hostLabel = (level: HostLevel): string => HOST_LABEL[level] ?? "Unknown";

/** The most severe of the given levels (Normal when empty). */
export function worstHostLevel(levels: HostLevel[]): HostLevel {
  return levels.reduce<HostLevel>(
    (worst, level) => ((HOST_RANK[level] ?? 1) > (HOST_RANK[worst] ?? 0) ? level : worst),
    "normal",
  );
}
