import type { ReactNode } from "react";

import styles from "./PageHeader.module.css";

interface PageHeaderProps {
  title: string;
  subtitle?: ReactNode;
  /** Right-aligned badges or buttons. */
  children?: ReactNode;
}

export function PageHeader({ title, subtitle, children }: PageHeaderProps) {
  return (
    <div className={styles.head}>
      <div>
        <h1>{title}</h1>
        {subtitle && <p className={styles.sub}>{subtitle}</p>}
      </div>
      {children}
    </div>
  );
}
