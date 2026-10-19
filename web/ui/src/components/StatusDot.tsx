import type { Tone } from "../lib/tone";
import styles from "./StatusDot.module.css";

/** Decorative colour dot; always pair it with text that carries the status. */
export function StatusDot({ tone }: { tone: Tone }) {
  return <span className={`${styles.dot} ${styles[tone]}`} data-tone={tone} aria-hidden="true" />;
}
