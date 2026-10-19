import { useState } from "react";

import { Button } from "../components/Button";
import { RestoreDialog } from "./dialogs";
import { useProjectBusy } from "./useBusy";

export function RestoreButton({ project, revision }: { project: string; revision: string }) {
  const [open, setOpen] = useState(false);
  const busy = useProjectBusy(project);
  return (
    <>
      <Button
        size="sm"
        disabled={busy}
        title={busy ? "A job is running for this project" : undefined}
        aria-label={`Restore ${revision}`}
        onClick={() => setOpen(true)}
      >
        Restore…
      </Button>
      <RestoreDialog open={open} onOpenChange={setOpen} project={project} revision={revision} />
    </>
  );
}
