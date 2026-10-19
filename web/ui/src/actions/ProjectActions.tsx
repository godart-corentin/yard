import type { ProjectStatus } from "../api";
import { ButtonLink } from "../components/Button";
import styles from "./buttons.module.css";
import { BackupButton } from "./BackupButton";
import { DeployButton } from "./DeployButton";
import { useProjectBusy } from "./useBusy";

/** Backup, Restore… (opens the releases tab, where a revision is chosen) and Deploy. */
export function ProjectActions({ project }: { project: ProjectStatus }) {
  const busy = useProjectBusy(project.name);
  if (project.url_monitor) return null; // URL-only monitors have no manifest to act on
  return (
    <div className={styles.group}>
      <BackupButton project={project.name} />
      <ButtonLink
        to="/projects/$name/releases"
        params={{ name: project.name }}
        aria-disabled={busy || undefined}
        title={busy ? "A job is running for this project" : undefined}
        style={busy ? { pointerEvents: "none", opacity: 0.5 } : undefined}
        tabIndex={busy ? -1 : undefined}
      >
        Restore…
      </ButtonLink>
      <DeployButton project={project} />
    </div>
  );
}
