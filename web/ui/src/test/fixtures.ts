import type { Job, ProjectStatus, StatusPayload } from "../api";

export const NOW_MS = Date.parse("2026-10-19T12:00:00Z");
export const NOW_UNIX = Math.floor(NOW_MS / 1000);

const emptyRelease = { record_status: "missing", tag: null, revision: null, deployed_at_unix: null, status: null, services: [] };

export function project(overrides: Partial<ProjectStatus> & { name: string }): ProjectStatus {
  return {
    application_services: [],
    url_monitor: false,
    services: [],
    state_status: "recorded",
    release: emptyRelease,
    previous_release: emptyRelease,
    pending_release: emptyRelease,
    restore_points: [],
    cli: null,
    last_backup: null,
    last_offsite: null,
    offsite_configured: null,
    config_error: null,
    checked_at: "2026-10-19T12:00:00Z",
    latency_ms: null,
    http_status: null,
    status: "unknown",
    error: null,
    ...overrides,
  };
}

export const probe = (
  service: string,
  status: "healthy" | "degraded" | "unhealthy" | "unknown",
  extra: Partial<StatusPayload["projects"][number]["services"][number]> = {},
) => ({
  project: "x",
  service,
  status,
  kind: "http",
  latency_ms: null,
  age_seconds: null,
  message: null,
  ...extra,
});

const released = (tag: string, hoursAgo: number) => ({
  record_status: "recorded",
  tag,
  revision: `${tag}ffffffff`,
  deployed_at_unix: NOW_UNIX - hoursAgo * 3600,
  status: "active",
  services: [],
});

const attempt = (secondsAgo: number) => ({ started_at_unix: NOW_UNIX - secondsAgo, duration_ms: 38_000, result: "success" as const });

export const operational = project({
  name: "hello-api",
  status: "operational",
  latency_ms: 142,
  release: released("abc123def456", 2),
  cli: { name: "hello-api", verdict: "ok", reason: null, branch: "main", head: null, env_tag: null, runtime: null, drift: [], repo_bytes: null, measured_at_unix: NOW_UNIX },
  services: [probe("api", "healthy"), probe("worker", "healthy"), probe("migrate", "unknown")],
  last_backup: attempt(3600),
});

export const degraded = project({
  name: "notes-app",
  status: "degraded",
  latency_ms: 204,
  release: released("51c0d9e8a7f3", 6 * 24),
  services: [
    probe("api", "healthy"),
    probe("worker", "degraded", { kind: "heartbeat", age_seconds: 252, max_age_seconds: 120 }),
  ],
  last_backup: { started_at_unix: NOW_UNIX - 30 * 3600, duration_ms: 1000, result: "failure" },
});

export const down = project({
  name: "broken",
  status: "down",
  http_status: 503,
  release: released("deadbeef0001", 1),
  application_services: ["web"],
});

export const unknown = project({ name: "mystery", application_services: ["web"] });

export const urlMonitor = project({
  name: "docs-site",
  url_monitor: true,
  status: "operational",
  latency_ms: 61,
  state_status: "missing",
});

const metric = <T,>(value: T, status: "normal" | "warning" | "critical" | "unknown" = "normal") => ({ value, status, message: null });

export const hostAvailable: StatusPayload["host"] = {
  status: "available",
  age_seconds: 42,
  message: "",
  snapshot: {
    version: 2,
    collected_at_unix: NOW_UNIX - 42,
    thresholds: { disk_warn_percent: 80, disk_crit_percent: 90, mem_pressure_percent: 90 },
    cpu: metric(18),
    load: metric<[number, number, number]>([0.1, 0.2, 0.3]),
    memory: metric({ used_bytes: 61, total_bytes: 100 }),
    disks: [metric({ mount: "/", used_bytes: 72, total_bytes: 100 })],
    docker: metric({ images: 1, containers: 1, volumes: 1 } as never),
    containers: [],
    containers_status: "normal",
    containers_message: null,
    services: [],
  },
};

export const hostMissing: StatusPayload["host"] = {
  status: "unknown",
  age_seconds: null,
  snapshot: null,
  message: "Host snapshot unavailable",
};

export function payload(projects: ProjectStatus[], host = hostAvailable): StatusPayload {
  return { status: "operational", checked_at: "2026-10-19T12:00:00Z", projects, host };
}

export const job = (overrides: Partial<Job> & { id: string }): Job => ({
  action: "deploy",
  project: "hello-api",
  revision: null,
  state: "succeeded",
  actor: "corentin",
  created_at_unix: NOW_UNIX - 2 * 3600,
  started_at_unix: NOW_UNIX - 2 * 3600,
  finished_at_unix: NOW_UNIX - 2 * 3600 + 102,
  exit_code: 0,
  output_bytes: 0,
  error: null,
  ...overrides,
});
