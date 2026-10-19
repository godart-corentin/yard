import type { ReactNode } from "react";

import type { Tone } from "../lib/tone";
import styles from "./Chip.module.css";
import { StatusDot } from "./StatusDot";

/** Small outlined label, e.g. a service with its health dot. */
export function Chip({ tone, title, children }: { tone?: Tone; title?: string; children: ReactNode }) {
  return (
    <span className={styles.chip} title={title}>
      {tone && <StatusDot tone={tone} />}
      {children}
    </span>
  );
}
