import { Card } from "./Card";
import { PageHeader } from "./PageHeader";
import styles from "./Placeholder.module.css";

/** Stand-in for pages that later tasks fill in, so every route already resolves. */
export function Placeholder({ title, subtitle }: { title: string; subtitle?: string }) {
  return (
    <>
      <PageHeader title={title} subtitle={subtitle} />
      <Card className={styles.body}>This page is not available yet.</Card>
    </>
  );
}
