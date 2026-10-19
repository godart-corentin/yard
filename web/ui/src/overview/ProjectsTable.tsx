import { Link, useNavigate } from "@tanstack/react-router";
import type { MouseEvent } from "react";

import type { ProjectStatus } from "../api";
import { Badge } from "../components/Badge";
import { DeployButton } from "../actions/DeployButton";
import { ButtonLink } from "../components/Button";
import { Card, CardHeader } from "../components/Card";
import { Chip } from "../components/Chip";
import { EmptyState } from "../components/EmptyState";
import { StatusDot } from "../components/StatusDot";
import { Table, Td, Th } from "../components/Table";
import { projectLabel, projectTone } from "../lib/tone";
import { backupCell, kindLabel, releaseCell, serviceChips } from "./model";
import styles from "./ProjectsTable.module.css";

function Row({ project, nowMs }: { project: ProjectStatus; nowMs: number }) {
  const navigate = useNavigate();
  const tone = projectTone(project.status);
  const release = releaseCell(project, nowMs);
  const backup = backupCell(project, nowMs);
  const chips = serviceChips(project);

  // The whole row opens the project, but real links and buttons keep their own behavior.
  const open = (event: MouseEvent) => {
    // Portaled dialogs bubble React events through the row; only clicks inside the row count.
    if (!event.currentTarget.contains(event.target as Node)) return;
    if ((event.target as HTMLElement).closest("a, button")) return;
    void navigate({ to: "/projects/$name/overview", params: { name: project.name } });
  };

  return (
    <tr className={styles.row} onClick={open}>
      <Td>
        <Link to="/projects/$name/overview" params={{ name: project.name }} className={styles.name}>
          <StatusDot tone={tone} />
          <span>
            <strong>{project.name}</strong>
            <br />
            <span className={`muted ${styles.small}`}>{kindLabel(project)}</span>
          </span>
        </Link>
      </Td>
      <Td>
        <Badge tone={tone}>{projectLabel(project.status)}</Badge>
        <br />
        <span className={`muted mono ${styles.latency}`}>
          {project.latency_ms != null ? `${project.latency_ms} ms` : "—"}
        </span>
      </Td>
      <Td>
        <span className="mono">{release.tag}</span>
        <br />
        <span className={`muted ${styles.small}`}>{release.note}</span>
      </Td>
      <Td>
        {chips.length ? (
          chips.map((chip) => (
            <Chip key={chip.name} tone={chip.tone}>
              {chip.name}
            </Chip>
          ))
        ) : (
          <span className="muted">—</span>
        )}
      </Td>
      <Td className={backup.failed ? styles.failed : "muted"}>{backup.text}</Td>
      <Td align="right" className={styles.actions}>
        {project.url_monitor ? (
          <span className={`muted ${styles.small}`}>Monitor only</span>
        ) : (
          <>
            <ButtonLink size="sm" to="/projects/$name/logs" params={{ name: project.name }} aria-label={`Logs ${project.name}`}>
              Logs
            </ButtonLink>{" "}
            <DeployButton project={project} label="Deploy" size="sm" variant="default" />
          </>
        )}
      </Td>
    </tr>
  );
}

export function ProjectsTable({ projects, nowMs }: { projects: ProjectStatus[]; nowMs: number }) {
  return (
    <Card as="section" aria-labelledby="projects-title">
      <CardHeader title="Projects" titleId="projects-title">
        <span className={`muted ${styles.small}`}>Click a row for details</span>
      </CardHeader>
      {projects.length === 0 ? (
        <EmptyState>No projects configured. Add a manifest under /etc/yard/projects.</EmptyState>
      ) : (
        <Table>
          <thead>
            <tr>
              <Th>Project</Th>
              <Th>Health</Th>
              <Th>Release</Th>
              <Th>Services</Th>
              <Th>Last backup</Th>
              <Th align="right">Actions</Th>
            </tr>
          </thead>
          <tbody>
            {projects.map((project) => (
              <Row key={project.name} project={project} nowMs={nowMs} />
            ))}
          </tbody>
        </Table>
      )}
    </Card>
  );
}
