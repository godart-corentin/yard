import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";

import type { StatusPayload } from "../api";
import { Card } from "../components/Card";
import { hostLabel } from "../lib/tone";
import { summarizeHost } from "../shell/hostSummary";
import { projectLevel } from "../lib/tone";
import { summarize } from "./model";
import styles from "./StatCards.module.css";

interface StatProps {
  label: string;
  value: ReactNode;
  hint: ReactNode;
  tone?: "ok" | "warn" | "bad";
}

function Stat({ label, value, hint, tone }: StatProps) {
  return (
    <Card className={styles.stat} role="group" aria-label={label}>
      <span className={styles.label}>{label}</span>
      <span className={styles.value} data-tone={tone}>
        {value}
      </span>
      <span className={`muted ${styles.hint}`}>{hint}</span>
    </Card>
  );
}

export function StatCards({ status }: { status: StatusPayload }) {
  const s = summarize(status.projects);
  const host = summarizeHost(status.host);
  const down = status.projects.some((p) => projectLevel(p.status) === "down");
  const names = s.attention.map((p) => `${p.name} ${projectLevel(p.status)}`);
  const attentionHint =
    names.length > 2 ? `${names.slice(0, 2).join(", ")} +${names.length - 2}` : names.join(", ") ||
    (s.unknown > 0 ? `${s.unknown} unknown` : "All clear");

  return (
    <div className={styles.grid}>
      <Stat
        label="Projects"
        value={s.total}
        hint={`${s.deployable} deployable · ${s.monitors} URL monitor${s.monitors === 1 ? "" : "s"}`}
      />
      <Stat
        label="Operational"
        value={s.operational}
        tone={s.operational > 0 ? "ok" : undefined}
        hint={s.servicesTotal ? `${s.servicesHealthy} of ${s.servicesTotal} services healthy` : "No service probes"}
      />
      <Stat
        label="Attention"
        value={s.attention.length}
        tone={s.attention.length ? (down ? "bad" : "warn") : undefined}
        hint={attentionHint}
      />
      <Link to="/host" className={styles.link}>
        <Stat label="Host" value={hostLabel(host.level)} hint={host.detail} />
      </Link>
    </div>
  );
}
