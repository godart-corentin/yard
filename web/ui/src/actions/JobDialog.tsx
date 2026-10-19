import * as Dialog from "@radix-ui/react-dialog";
import { Link } from "@tanstack/react-router";
import type { ReactNode } from "react";

import type { CreateJobRequest } from "../api";
import { Button } from "../components/Button";
import styles from "./JobDialog.module.css";
import { useStartJob } from "./useStartJob";

interface JobDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Small command line above the title, e.g. "yard deploy hello-api". */
  kicker: string;
  title: string;
  confirmLabel: string;
  variant?: "primary" | "danger";
  /** Extra condition that must hold before the confirm button works. */
  ready?: boolean;
  request: CreateJobRequest;
  children: ReactNode;
}

/** Shared frame of the confirmation dialogs: starts the job, shows honest errors, opens the job page. */
export function JobDialog({
  open,
  onOpenChange,
  kicker,
  title,
  confirmLabel,
  variant = "primary",
  ready = true,
  request,
  children,
}: JobDialogProps) {
  const { start, pending, error, reset } = useStartJob(() => onOpenChange(false));

  return (
    <Dialog.Root
      open={open}
      onOpenChange={(next) => {
        if (!next) reset();
        onOpenChange(next);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className={styles.overlay} />
        <Dialog.Content className={styles.dialog}>
          <div className={styles.head}>
            <Dialog.Description className={styles.kicker}>{kicker}</Dialog.Description>
            <Dialog.Title asChild>
              <h2>{title}</h2>
            </Dialog.Title>
          </div>
          <div className={styles.body}>
            {children}
            {error && (
              <p className={styles.error} role="alert">
                {error.message}
                {error.runningJobId && (
                  <>
                    {" "}
                    <Link to="/activity/$jobId" params={{ jobId: error.runningJobId }} onClick={() => onOpenChange(false)}>
                      Open the running job
                    </Link>
                  </>
                )}
              </p>
            )}
          </div>
          <div className={styles.foot}>
            <Dialog.Close asChild>
              <Button>Cancel</Button>
            </Dialog.Close>
            <Button
              variant={variant === "primary" ? "primary" : "default"}
              className={variant === "danger" ? styles.danger : undefined}
              disabled={!ready || pending}
              onClick={() => start(request)}
            >
              {pending ? "Starting…" : confirmLabel}
            </Button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
