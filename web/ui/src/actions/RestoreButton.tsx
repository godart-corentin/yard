import { Button } from "../components/Button";

/** Placeholder; the actions task replaces this with the real confirmed restore. */
export function RestoreButton({ project, revision }: { project: string; revision: string }) {
  return (
    <Button size="sm" disabled aria-label={`Restore ${project} ${revision}`}>
      Restore
    </Button>
  );
}
