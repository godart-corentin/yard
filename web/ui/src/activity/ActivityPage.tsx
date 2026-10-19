import { useState } from "react";

import type { JobState } from "../api";
import { useJobList } from "../api/jobQueries";
import { Badge } from "../components/Badge";
import { Button, ButtonLink } from "../components/Button";
import { Card } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { Table, Td, Th } from "../components/Table";
import { formatDuration, formatWhen } from "../lib/format";
import { jobActionLabel, jobDetail, jobState, jobTitle } from "../lib/jobs";
import { useNow } from "../shell/useNow";
import styles from "./ActivityPage.module.css";
import { jobSeconds } from "./JobPage";

const FILTERS: { label: string; state: JobState | undefined }[] = [
  { label: "All", state: undefined },
  { label: "Running", state: "running" },
  { label: "Succeeded", state: "succeeded" },
  { label: "Failed", state: "failed" },
];

/** Strips the date part of an id (`20261019T142501Z-3fa9c01b`), keeping the random suffix. */
const shortId = (id: string) => id.split("-").pop() ?? id;

export function ActivityPage() {
  const [filter, setFilter] = useState<JobState | undefined>(undefined);
  const jobs = useJobList(filter);
  const now = useNow(5000);
  const rows = jobs.data?.pages.flatMap((page) => page.jobs) ?? [];

  return (
    <>
      <PageHeader title="Activity" subtitle="Every action started from Web, with its full output · kept 90 days" />

      <div className={styles.filters} role="group" aria-label="Filter by result">
        {FILTERS.map((item) => (
          <button
            key={item.label}
            type="button"
            className={styles.chip}
            aria-pressed={item.state === filter}
            onClick={() => setFilter(item.state)}
          >
            {item.label}
          </button>
        ))}
      </div>

      <Card as="section" aria-label="Jobs">
        {jobs.isPending ? (
          <EmptyState>Loading activity…</EmptyState>
        ) : jobs.isError ? (
          <EmptyState>
            Activity is not available right now.{" "}
            <Button size="sm" onClick={() => void jobs.refetch()}>
              Retry
            </Button>
          </EmptyState>
        ) : rows.length === 0 ? (
          <EmptyState>{filter ? "No job matches this filter." : "No activity yet."}</EmptyState>
        ) : (
          <>
            <Table>
              <thead>
                <tr>
                  <Th>Job</Th>
                  <Th>Action</Th>
                  <Th>Project</Th>
                  <Th>By</Th>
                  <Th>Started</Th>
                  <Th>Duration</Th>
                  <Th>Result</Th>
                  <Th align="right">
                    <span className="sr-only">Output</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {rows.map((job) => {
                  const state = jobState(job);
                  const seconds = jobSeconds(job, now);
                  const detail = jobDetail(job);
                  return (
                    <tr key={job.id}>
                      <Td className="mono muted">#{shortId(job.id)}</Td>
                      <Td>
                        <strong>{jobActionLabel(job)}</strong>
                        <br />
                        <span className={`muted ${styles.small}`}>{detail || "—"}</span>
                      </Td>
                      <Td>{job.project ?? "—"}</Td>
                      <Td className="muted">{job.actor}</Td>
                      <Td className="muted">{formatWhen(job.created_at_unix, now)}</Td>
                      <Td className="mono">{seconds != null ? formatDuration(seconds) : "—"}</Td>
                      <Td>
                        <Badge tone={state.tone}>{state.label}</Badge>
                      </Td>
                      <Td align="right">
                        <ButtonLink size="sm" to="/activity/$jobId" params={{ jobId: job.id }} aria-label={`Output of ${jobTitle(job)} ${shortId(job.id)}`}>
                          Output
                        </ButtonLink>
                      </Td>
                    </tr>
                  );
                })}
              </tbody>
            </Table>
            {jobs.hasNextPage && (
              <div className={styles.more}>
                <Button size="sm" disabled={jobs.isFetchingNextPage} onClick={() => void jobs.fetchNextPage()}>
                  {jobs.isFetchingNextPage ? "Loading…" : "Load more"}
                </Button>
              </div>
            )}
          </>
        )}
      </Card>
    </>
  );
}
