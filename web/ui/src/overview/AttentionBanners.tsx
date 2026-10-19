import type { ProjectStatus } from "../api";
import { Badge } from "../components/Badge";
import { ButtonLink } from "../components/Button";
import { Card } from "../components/Card";
import styles from "./AttentionBanners.module.css";
import { attentionItems, hasPendingRelease } from "./model";

/** One banner per Degraded/Down project, plus interrupted releases. Nothing when all is calm. */
export function AttentionBanners({ projects }: { projects: ProjectStatus[] }) {
  const pending = projects.filter(hasPendingRelease);
  return (
    <>
      {attentionItems(projects).map(({ project, tone, label, service, detail, more }) => (
        <Card key={project.name} tone={tone} className={styles.banner} role="alert">
          <Badge tone={tone}>{label}</Badge>
          <div className={styles.text}>
            <strong>{service ? `${project.name} · ${service}` : project.name}</strong>{" "}
            <span className="muted">
              {detail}
              {more > 0 && ` (+${more} more)`}
            </span>
          </div>
          {service && !project.url_monitor && (
            <ButtonLink size="sm" to="/projects/$name/logs" params={{ name: project.name }}>
              Open {service} logs
            </ButtonLink>
          )}
          <ButtonLink size="sm" variant="ghost" to="/projects/$name/overview" params={{ name: project.name }}>
            View project
          </ButtonLink>
        </Card>
      ))}
      {pending.map((project) => (
        <Card key={`${project.name}-pending`} tone="warn" className={styles.banner} role="alert">
          <Badge tone="warn">Interrupted</Badge>
          <div className={styles.text}>
            <strong>{project.name}</strong>{" "}
            <span className="muted">
              release {project.pending_release.tag ?? "—"} {project.pending_release.status ?? "activating"} — Docker
              state may differ from the recorded release
            </span>
          </div>
          <ButtonLink size="sm" variant="ghost" to="/projects/$name/overview" params={{ name: project.name }}>
            View project
          </ButtonLink>
        </Card>
      ))}
    </>
  );
}
