import { useState } from "react";

import { Button } from "../components/Button";
import { PruneDialog } from "./dialogs";
import { useAnyBusy } from "./useBusy";

/** Runs the dry run inside the dialog first; the real prune is a second, explicit step. */
export function PruneButton() {
  const [open, setOpen] = useState(false);
  const busy = useAnyBusy();
  return (
    <>
      <Button disabled={busy} title={busy ? "Another job is running" : undefined} onClick={() => setOpen(true)}>
        Simulate prune…
      </Button>
      <PruneDialog open={open} onOpenChange={setOpen} />
    </>
  );
}
