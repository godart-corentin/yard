import { Button } from "../components/Button";

/** Placeholder; the actions task replaces this with the real confirmed backup. */
export function BackupButton({ project }: { project: string }) {
  return (
    <Button size="sm" disabled aria-label={`Run backup ${project}`}>
      Run now
    </Button>
  );
}
