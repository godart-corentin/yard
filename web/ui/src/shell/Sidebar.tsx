import { Link } from "@tanstack/react-router";

import { Icon } from "../components/Icon";
import { StatusDot } from "../components/StatusDot";
import { useRunningJobs } from "../api/jobQueries";
import { useStatus } from "../api/queries";
import { formatAge } from "../lib/format";
import { hostLabel, hostTone, projectTone } from "../lib/tone";
import { summarizeHost } from "./hostSummary";
import styles from "./Sidebar.module.css";

function BrandMark() {
  return (
    <svg className={styles.mark} width="26" height="26" viewBox="0 0 32 32" aria-hidden="true">
      <path d="M16 2 28 9v14l-12 7L4 23V9L16 2Z" fill="currentColor" />
      <path d="m10.5 10.5 5.5 3.25 5.5-3.25v4.25L18 16.8v5.7h-4v-5.7l-3.5-2.05V10.5Z" fill="var(--color-sidebar)" />
    </svg>
  );
}

export function Sidebar() {
  const { data } = useStatus();
  const host = summarizeHost(data?.host);
  const running = useRunningJobs().data?.jobs.length ?? 0;

  return (
    <aside className={styles.side}>
      <Link to="/" className={styles.brand}>
        <BrandMark />
        Yard
      </Link>

      <nav className={styles.group} aria-label="Main">
        <Link to="/" className={styles.nav} activeOptions={{ exact: true }}>
          <Icon name="overview" />
          Overview
        </Link>
        <Link to="/activity" className={styles.nav}>
          <Icon name="activity" />
          Activity
          {running > 0 && <span className={styles.pill}>{running} running</span>}
        </Link>
        <Link to="/images" className={styles.nav}>
          <Icon name="images" />
          Images
        </Link>
        <Link to="/host" className={styles.nav}>
          <Icon name="host" />
          Host
        </Link>
      </nav>

      <nav className={styles.group} aria-label="Projects">
        <div className={styles.label}>Projects</div>
        {data?.projects.map((project) => (
          <Link
            key={project.name}
            // The project route redirects to its overview tab; linking to the parent keeps
            // the entry highlighted on every tab.
            to="/projects/$name"
            params={{ name: project.name }}
            className={styles.nav}
          >
            <StatusDot tone={projectTone(project.status)} />
            {project.name}
            {project.url_monitor && <span className={styles.count}>URL</span>}
          </Link>
        ))}
      </nav>

      <div className={styles.foot}>
        <span>
          <StatusDot tone={hostTone(host.level)} /> Host · {hostLabel(host.level).toLowerCase()}
        </span>
        <span>{host.ageSeconds == null ? "No snapshot" : `Snapshot ${formatAge(host.ageSeconds)} ago`}</span>
      </div>
    </aside>
  );
}
