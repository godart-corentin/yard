import { useState } from "react";

import { Button } from "../components/Button";
import { BackupDialog } from "./dialogs";
import { useProjectBusy } from "./useBusy";

export function BackupButton({ project }: { project: string }) {
  const [open, setOpen] = useState(false);
  const busy = useProjectBusy(project);
  return (
    <>
      <Button
        disabled={busy}
        title={busy ? "A job is running for this project" : undefined}
        onClick={() => setOpen(true)}
      >
        Backup
      </Button>
      <BackupDialog open={open} onOpenChange={setOpen} project={project} />
    </>
  );
}
