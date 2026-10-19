import { Badge } from "../components/Badge";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { STATUS_REFRESH_MS, useStatus } from "../api/queries";
import { projectLevel } from "../lib/tone";
import { AttentionBanners } from "./AttentionBanners";
import { ProjectsTable } from "./ProjectsTable";
import { RecentActivity } from "./RecentActivity";
import { StatCards } from "./StatCards";

export function OverviewPage() {
  const status = useStatus();

  if (!status.data) {
    return (
      <>
        <PageHeader title="Overview" />
        <Card>
          {status.isError ? (
            <EmptyState>
              Status unavailable. Retrying every {STATUS_REFRESH_MS / 1000} s.{" "}
              <Button size="sm" onClick={() => void status.refetch()}>
                Retry now
              </Button>
            </EmptyState>
          ) : (
            <EmptyState>Loading status…</EmptyState>
          )}
        </Card>
      </>
    );
  }

  const { projects } = status.data;
  const nowMs = Date.now();
  const levels = projects.map((p) => projectLevel(p.status));
  const attention = levels.filter((l) => l === "degraded" || l === "down").length;
  const unknown = levels.filter((l) => l === "unknown").length;

  return (
    <>
      <PageHeader
        title="Overview"
        subtitle={
          <>
            {projects.length} project{projects.length === 1 ? "" : "s"} · refreshes every {STATUS_REFRESH_MS / 1000} s
            {status.isError && " · last refresh failed, showing earlier data"}
          </>
        }
      >
        {attention > 0 ? (
          <Badge tone={levels.includes("down") ? "bad" : "warn"}>{attention} needs attention</Badge>
        ) : projects.length === 0 || unknown > 0 ? (
          <Badge tone="unk">{unknown > 0 ? `${unknown} unknown` : "No projects"}</Badge>
        ) : (
          <Badge tone="ok">All operational</Badge>
        )}
      </PageHeader>
      <AttentionBanners projects={projects} />
      <StatCards status={status.data} />
      <ProjectsTable projects={projects} nowMs={nowMs} />
      <RecentActivity nowMs={nowMs} />
    </>
  );
}
