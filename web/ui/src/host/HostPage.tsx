import { Badge } from "../components/Badge";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { Table, Td, Th } from "../components/Table";
import { HostCollectButton } from "../actions/HostCollectButton";
import { STATUS_REFRESH_MS, useStatus } from "../api/queries";
import type { HostLevel } from "../api";
import { formatAge } from "../lib/format";
import { hostLabel, hostTone, type Tone } from "../lib/tone";
import { summarizeHost } from "../shell/hostSummary";
import { Button } from "../components/Button";
import styles from "./HostPage.module.css";

const GIB = 1024 ** 3;
export function formatBytes(bytes: number): string {
  if (bytes >= 1024 ** 4) return `${(bytes / 1024 ** 4).toFixed(1)} TB`;
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(1)} GB`;
  if (bytes >= 1024 ** 2) return `${Math.round(bytes / 1024 ** 2)} MB`;
  return `${Math.round(bytes / 1024)} KB`;
}

const pct = (used: number, total: number) => (total > 0 ? Math.min(100, (used / total) * 100) : 0);

function Meter({ percent, tone }: { percent: number | null; tone: Tone }) {
  return (
    <div className={styles.bar} aria-hidden="true">
      <div className={styles.fill} data-tone={tone} style={{ width: `${percent ?? 0}%` }} />
    </div>
  );
}

const levelText = (level: HostLevel) => (level === "normal" ? "OK" : hostLabel(level));

export function HostPage() {
  const status = useStatus();
  const host = status.data?.host;

  if (!status.data) {
    return (
      <>
        <PageHeader title="Host" />
        <Card>
          {status.isError ? (
            <EmptyState>
              Status unavailable. Retrying every {STATUS_REFRESH_MS / 1000} s.{" "}
              <Button size="sm" onClick={() => void status.refetch()}>
                Retry now
              </Button>
            </EmptyState>
          ) : (
            <EmptyState>Loading status…</EmptyState>
          )}
        </Card>
      </>
    );
  }

  const summary = summarizeHost(host);
  const snapshot = host?.status === "available" ? host.snapshot : null;
  const maxAge = host?.age_seconds != null ? `Snapshot ${formatAge(host.age_seconds)} ago` : "No snapshot age";
  const subtitle = snapshot ? `${maxAge} · collected by the Yard CLI` : `${summary.detail}${host?.age_seconds != null ? ` · last snapshot ${formatAge(host.age_seconds)} ago` : ""}`;

  return (
    <>
      <PageHeader
        title="Host"
        subtitle={subtitle}
      >
        <div className={styles.titleRow}>
          <Badge tone={hostTone(summary.level)}>{hostLabel(summary.level)}</Badge>
          <HostCollectButton />
        </div>
      </PageHeader>
      {!snapshot ? (
        <Card>
          <EmptyState>
            Host measurements are Unknown: {summary.detail}. Run <span className="mono">yard host</span> on the host to collect
            a fresh snapshot.
          </EmptyState>
        </Card>
      ) : (
        <Snapshot snapshot={snapshot} />
      )}
    </>
  );
}

function Snapshot({ snapshot }: { snapshot: NonNullable<import("../api").HostStatus["snapshot"]> }) {
  const { cpu, memory, load, disks, docker, containers, thresholds } = snapshot;
  const mem = memory.value;
  const memPct = mem ? pct(mem.used_bytes, mem.total_bytes) : null;
  const diskWarnings = disks.filter((d) => d.status === "warning" || d.status === "critical").length;
  const worstDisk = disks.some((d) => d.status === "critical") ? "bad" : diskWarnings ? "warn" : disks.some((d) => d.status === "unknown") ? "unk" : "ok";
  const dockerValue = docker.value;

  return (
    <>
      <div className={styles.metrics}>
        <Card className={styles.metric} role="group" aria-label="CPU">
          <div className={styles.top}>
            <span className="muted">CPU</span>
            <Badge tone={hostTone(cpu.status)}>{levelText(cpu.status)}</Badge>
          </div>
          <span className={styles.big}>{cpu.value != null ? `${Math.round(cpu.value)} %` : "Unknown"}</span>
          <Meter percent={cpu.value} tone={hostTone(cpu.status)} />
          {cpu.message && <span className={`muted ${styles.small}`}>{cpu.message}</span>}
        </Card>
        <Card className={styles.metric} role="group" aria-label="Memory">
          <div className={styles.top}>
            <span className="muted">Memory</span>
            <Badge tone={hostTone(memory.status)}>{levelText(memory.status)}</Badge>
          </div>
          <span className={styles.big}>{memPct != null ? `${Math.round(memPct)} %` : "Unknown"}</span>
          <Meter percent={memPct} tone={hostTone(memory.status)} />
          <span className={`muted ${styles.small}`}>
            {mem ? `${formatBytes(mem.used_bytes)} of ${formatBytes(mem.total_bytes)} · pressure ${thresholds.mem_pressure_percent} %` : memory.message ?? ""}
          </span>
        </Card>
        <Card className={styles.metric} role="group" aria-label="Load">
          <div className={styles.top}>
            <span className="muted">Load</span>
            <Badge tone="unk">Info</Badge>
          </div>
          <span className={styles.big}>{load.value ? load.value[0].toFixed(2) : "Unknown"}</span>
          <span className={`muted ${styles.small}`}>
            {load.value ? `1 m · ${load.value[1].toFixed(2)} 5 m · ${load.value[2].toFixed(2)} 15 m` : load.message ?? ""}
          </span>
        </Card>
      </div>

      <div className={styles.columns}>
        <Card as="section" className={styles.disks} aria-labelledby="disk-title">
          <CardHeader title="Disks" titleId="disk-title">
            {disks.length === 0 ? (
              <Badge tone="unk">None reported</Badge>
            ) : (
              <Badge tone={worstDisk}>{diskWarnings ? `${diskWarnings} warning${diskWarnings === 1 ? "" : "s"}` : worstDisk === "ok" ? "OK" : "Unknown"}</Badge>
            )}
          </CardHeader>
          {disks.map((d, i) => {
            const v = d.value;
            const percent = v ? pct(v.used_bytes, v.total_bytes) : null;
            const tone = hostTone(d.status);
            return (
              <div className={styles.disk} key={v?.mount ?? i}>
                <div className={styles.top}>
                  <span className="mono">{v?.mount ?? "Unknown disk"}</span>
                  <span className={`muted ${styles.value}`} data-tone={tone} style={{ fontSize: 13 }}>
                    {v && percent != null
                      ? `${Math.round(percent)} % · ${formatBytes(v.used_bytes)} of ${formatBytes(v.total_bytes)}`
                      : d.message ?? "Unknown"}
                  </span>
                </div>
                <Meter percent={percent} tone={tone} />
              </div>
            );
          })}
          <div className={styles.disk}>
            <div className={styles.top}>
              <span className="muted">Docker</span>
              <span className="muted" style={{ fontSize: 13 }}>
                {dockerValue
                  ? `images ${dockerValue.images} · containers ${dockerValue.containers} · volumes ${dockerValue.volumes}`
                  : docker.message ?? "Unknown"}
              </span>
            </div>
          </div>
        </Card>

        <Card as="section" className={styles.containers} aria-labelledby="ct-title">
          <CardHeader title="Containers" titleId="ct-title">
            <span className={`muted ${styles.small}`}>Yard projects only</span>
          </CardHeader>
          {containers.length === 0 ? (
            <EmptyState>{snapshot.containers_message ?? "No containers reported"}</EmptyState>
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>Project</Th>
                  <Th>Service</Th>
                  <Th>State</Th>
                </tr>
              </thead>
              <tbody>
                {containers.map((c) => (
                  <tr key={`${c.project}/${c.service}`}>
                    <Td>{c.project}</Td>
                    <Td className="mono">{c.service}</Td>
                    <Td>
                      <Badge tone={hostTone(c.status)}>{c.state}</Badge>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          )}
        </Card>
      </div>
    </>
  );
}
