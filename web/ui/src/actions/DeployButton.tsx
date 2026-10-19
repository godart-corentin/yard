import { useState } from "react";

import type { ProjectStatus } from "../api";
import { Button } from "../components/Button";
import { Icon } from "../components/Icon";
import { DeployDialog } from "./dialogs";
import { useProjectBusy } from "./useBusy";

interface DeployButtonProps {
  project: ProjectStatus;
  /** Button text; defaults to "Deploy <branch>". */
  label?: string;
  size?: "md" | "sm";
  variant?: "default" | "primary";
}

/** Opens the deploy confirmation; disabled while a job holds the project. */
export function DeployButton({ project, label, size = "md", variant = "primary" }: DeployButtonProps) {
  const [open, setOpen] = useState(false);
  const busy = useProjectBusy(project.name);
  const branch = project.cli?.branch;
  return (
    <>
      <Button
        variant={variant}
        size={size}
        disabled={busy}
        title={busy ? "A job is running for this project" : undefined}
        aria-label={`Deploy ${project.name}`}
        onClick={() => setOpen(true)}
      >
        <Icon name="deploy" size={14} />
        {label ?? (branch ? `Deploy ${branch}` : "Deploy")}
      </Button>
      <DeployDialog open={open} onOpenChange={setOpen} project={project} />
    </>
  );
}
