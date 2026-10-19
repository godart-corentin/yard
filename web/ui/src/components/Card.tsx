import type { HTMLAttributes, ReactNode } from "react";

import styles from "./Card.module.css";

interface CardProps extends HTMLAttributes<HTMLElement> {
  as?: "div" | "section";
  /** Tinted background and border for banners that need attention. */
  tone?: "warn" | "bad";
}

export function Card({ as: Tag = "div", tone, className, ...rest }: CardProps) {
  return <Tag className={[styles.card, tone && styles[tone], className].filter(Boolean).join(" ")} {...rest} />;
}

interface CardHeaderProps {
  title: string;
  /** Id for the title, so the card can use `aria-labelledby`. */
  titleId?: string;
  children?: ReactNode;
}

/** Card title on the left, optional hint or action on the right. */
export function CardHeader({ title, titleId, children }: CardHeaderProps) {
  return (
    <div className={styles.header}>
      <h2 id={titleId}>{title}</h2>
      {children}
    </div>
  );
}
