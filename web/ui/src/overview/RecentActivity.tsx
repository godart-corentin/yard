import { Badge } from "../components/Badge";
import { ButtonLink } from "../components/Button";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { useRecentJobs } from "../api/queries";
import { formatWhen } from "../lib/format";
import { jobDetail, jobState, jobTitle } from "../lib/jobs";
import styles from "./RecentActivity.module.css";

/** Latest jobs. A missing or unavailable jobs API is a quiet state, never an error page. */
export function RecentActivity({ nowMs }: { nowMs: number }) {
  const jobs = useRecentJobs(5);

  return (
    <Card as="section" aria-labelledby="activity-title">
      <CardHeader title="Recent activity" titleId="activity-title">
        <ButtonLink size="sm" variant="ghost" to="/activity">
          View all
        </ButtonLink>
      </CardHeader>
      {jobs.isPending ? (
        <EmptyState>Loading activity…</EmptyState>
      ) : jobs.isError ? (
        <EmptyState>Activity is not available right now.</EmptyState>
      ) : jobs.data.jobs.length === 0 ? (
        <EmptyState>No activity yet.</EmptyState>
      ) : (
        <ul className={styles.feed}>
          {jobs.data.jobs.map((job) => {
            const state = jobState(job);
            const detail = jobDetail(job);
            return (
              <li key={job.id} className={styles.item}>
                <Badge tone={state.tone} className={styles.badge}>
                  {state.label}
                </Badge>
                <div className={styles.text}>
                  <strong>{jobTitle(job)}</strong> {detail && <span className="muted">{detail}</span>}
                </div>
                <span className={`muted ${styles.meta}`}>
                  {job.actor} · {formatWhen(job.created_at_unix, nowMs)}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
