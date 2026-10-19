import type { TableHTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from "react";

import styles from "./Table.module.css";

/** Table that scrolls horizontally inside its card instead of widening the page. */
export function Table({ className, ...rest }: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className={styles.wrap}>
      <table className={[styles.table, className].filter(Boolean).join(" ")} {...rest} />
    </div>
  );
}

type Align = { align?: "left" | "right" };

export function Th({ align, ...rest }: ThHTMLAttributes<HTMLTableCellElement> & Align) {
  return <th data-align={align} {...rest} />;
}

export function Td({ align, ...rest }: TdHTMLAttributes<HTMLTableCellElement> & Align) {
  return <td data-align={align} {...rest} />;
}
