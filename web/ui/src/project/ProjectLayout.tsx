import { Link, Outlet } from "@tanstack/react-router";

import { ProjectActions } from "../actions/ProjectActions";
import type { ProjectStatus } from "../api";
import { STATUS_REFRESH_MS } from "../api/queries";
import { Badge } from "../components/Badge";
import { Button } from "../components/Button";
import { Card } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { projectLabel, projectTone } from "../lib/tone";
import styles from "./ProjectLayout.module.css";
import { projectSubtitle } from "./model";
import { useProject } from "./queries";

const TABS = [
  { to: "/projects/$name/overview", label: "Overview" },
  { to: "/projects/$name/releases", label: "Releases & backups" },
  { to: "/projects/$name/logs", label: "Logs" },
  { to: "/projects/$name/activity", label: "Activity" },
] as const;

function Heading({ project }: { project: ProjectStatus }) {
  return (
    <div>
      <div className={styles.title}>
        <h1>{project.name}</h1>
        <Badge tone={projectTone(project.status)}>{projectLabel(project.status)}</Badge>
      </div>
      <p className={styles.sub}>{projectSubtitle(project)}</p>
    </div>
  );
}

/** URL monitors only have an HTTP result: no actions, tabs or CLI reads. */
function MonitorView({ project }: { project: ProjectStatus }) {
  return (
    <>
      <div className={styles.head}>
        <Heading project={project} />
      </div>
      <Card as="section" aria-labelledby="monitor-title">
        <div className={styles.kv} style={{ borderTop: 0 }}>
          <h2 id="monitor-title">Monitor</h2>
        </div>
        <div className={styles.kv}>
          <span>HTTP status</span>
          <span className="mono">{project.http_status ?? "—"}</span>
        </div>
        <div className={styles.kv}>
          <span>Latency</span>
          <span className="mono">{project.latency_ms != null ? `${project.latency_ms} ms` : "—"}</span>
        </div>
        {project.error && (
          <div className={styles.kv}>
            <span>Error</span>
            <span>{project.error}</span>
          </div>
        )}
        <EmptyState>
          Yard only checks this URL. Releases, backups, logs and deployment actions are not available.
        </EmptyState>
      </Card>
    </>
  );
}

/** Shared frame for `/projects/$name/*`: header, actions and tabs around the routed tab. */
export function ProjectLayout({ name }: { name: string }) {
  const { status, project } = useProject(name);

  if (!status.data) {
    return (
      <>
        <PageHeader title={name} />
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

  if (!project) {
    return (
      <>
        <PageHeader title="Project not found" />
        <Card>
          <EmptyState>
            No project named “{name}” is configured. <Link to="/">Back to overview</Link>
          </EmptyState>
        </Card>
      </>
    );
  }

  if (project.url_monitor) return <MonitorView project={project} />;

  return (
    <>
      <div className={styles.head}>
        <Heading project={project} />
        <div className={styles.actions}>
          <ProjectActions project={project} />
        </div>
      </div>
      <nav className={styles.tabs} aria-label="Project views">
        {TABS.map((tab) => (
          <Link key={tab.to} to={tab.to} params={{ name }} className={styles.tab} activeOptions={{ exact: true }}>
            {tab.label}
          </Link>
        ))}
      </nav>
      <Outlet />
    </>
  );
}
