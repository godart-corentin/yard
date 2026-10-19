import type { ProjectStatus } from "../api";
import { Button } from "../components/Button";

/** Placeholder; the actions task replaces this with the real confirmed job buttons. */
export function ProjectActions({ project }: { project: ProjectStatus }) {
  const branch = project.cli?.branch;
  return (
    <>
      <Button disabled>Backup</Button>
      <Button disabled>Restore…</Button>
      <Button variant="primary" disabled>
        {branch ? `Deploy ${branch}` : "Deploy"}
      </Button>
    </>
  );
}
