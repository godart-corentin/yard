import type { ReactNode } from "react";

import styles from "./EmptyState.module.css";

/** Quiet one-line message inside a card (nothing to show, or not available). */
export function EmptyState({ children }: { children: ReactNode }) {
  return (
    <p className={styles.empty} role="status">
      {children}
    </p>
  );
}
