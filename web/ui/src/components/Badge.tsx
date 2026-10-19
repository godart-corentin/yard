import type { ReactNode } from "react";

import type { Tone } from "../lib/tone";
import styles from "./Badge.module.css";

interface BadgeProps {
  tone: Tone;
  children: ReactNode;
  className?: string;
}

export function Badge({ tone, children, className }: BadgeProps) {
  return (
    <span className={[styles.badge, styles[tone], className].filter(Boolean).join(" ")} data-tone={tone}>
      {children}
    </span>
  );
}
