import type { HostLevel, HostStatus } from "../api";
import { formatPercent } from "../lib/format";
import { worstHostLevel } from "../lib/tone";

export interface HostSummary {
  level: HostLevel;
  /** "CPU 18 % · RAM 61 % · disk 72 %", or why there is nothing to show. */
  detail: string;
  ageSeconds: number | null;
}

/**
 * Host health as documented: only CPU, RAM and physical disks count (load,
 * Docker usage and containers do not). A missing, invalid or stale snapshot is
 * Unknown, never Healthy.
 */
export function summarizeHost(host: HostStatus | undefined): HostSummary {
  const snapshot = host?.status === "available" ? host.snapshot : null;
  if (!host || !snapshot) {
    return { level: "unknown", detail: host?.message || "Host snapshot unavailable", ageSeconds: host?.age_seconds ?? null };
  }
  const level = worstHostLevel([snapshot.cpu.status, snapshot.memory.status, ...snapshot.disks.map((d) => d.status)]);
  const parts: string[] = [];
  if (snapshot.cpu.value != null) parts.push(`CPU ${formatPercent(snapshot.cpu.value)}`);
  const memory = snapshot.memory.value;
  if (memory && memory.total_bytes > 0) parts.push(`RAM ${formatPercent((memory.used_bytes / memory.total_bytes) * 100)}`);
  const disks = snapshot.disks.flatMap((d) => (d.value && d.value.total_bytes > 0 ? [(d.value.used_bytes / d.value.total_bytes) * 100] : []));
  if (disks.length) parts.push(`disk ${formatPercent(Math.max(...disks))}`);
  return { level, detail: parts.join(" · ") || "No measurements", ageSeconds: host.age_seconds };
}
