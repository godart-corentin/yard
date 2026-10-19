import { Button } from "../components/Button";
import styles from "./buttons.module.css";
import { useAnyBusy } from "./useBusy";
import { useStartJob } from "./useStartJob";

/** Collecting a host snapshot is harmless and quick: no dialog, errors shown inline. */
export function HostCollectButton() {
  const busy = useAnyBusy();
  const { start, pending, error } = useStartJob();
  return (
    <span className={styles.group}>
      <Button
        disabled={busy || pending}
        title={busy ? "Another job is running" : undefined}
        onClick={() => start({ action: "host-collect" })}
      >
        {pending ? "Starting…" : "Collect now"}
      </Button>
      {error && (
        <span className={styles.error} role="alert">
          {error.message}
        </span>
      )}
    </span>
  );
}
