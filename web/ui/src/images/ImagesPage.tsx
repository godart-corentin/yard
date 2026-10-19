import { useQuery } from "@tanstack/react-query";
import { Fragment } from "react";

import { PruneButton } from "../actions/PruneButton";
import { api, YardApiError } from "../api";
import { Badge } from "../components/Badge";
import { Button } from "../components/Button";
import { Card, CardHeader } from "../components/Card";
import { EmptyState } from "../components/EmptyState";
import { PageHeader } from "../components/PageHeader";
import { Table, Td, Th } from "../components/Table";
import type { Tone } from "../lib/tone";
import { formatBytes } from "../host/HostPage";
import { parseImages, type ImageState } from "./parse";
import styles from "./ImagesPage.module.css";

export function useImages() {
  return useQuery({
    queryKey: ["read", "images"],
    queryFn: () => api.read({ op: "images" }),
    retry: false,
    refetchOnWindowFocus: false,
  });
}

/** Maps the documented failure statuses; never shows backend text. */
export function unavailableMessage(error: unknown): string {
  const status = error instanceof YardApiError ? error.status : null;
  if (status === 400) return "The image inventory read was refused.";
  if (status === 503) return "The host read executor could not answer.";
  if (status === 502) return "The image inventory command failed, timed out, exceeded its output limit or returned an invalid reply.";
  return "The image inventory could not be loaded.";
}

const STATE: Record<ImageState, { tone: Tone; label: string }> = {
  running: { tone: "ok", label: "In use" },
  keep: { tone: "unk", label: "Protected" },
  candidate: { tone: "warn", label: "Reclaimable" },
};

export function ImagesPage() {
  const images = useImages();
  const output = images.data?.output;
  const inventory = output !== undefined ? parseImages(output) : null;

  return (
    <>
      <PageHeader
        title="Images"
        subtitle="Yard image revisions as reported by the host (yard images)"
      >
        <div className={styles.actions}>
          <Button onClick={() => void images.refetch()} disabled={images.isFetching}>
            {images.isFetching ? "Running…" : "Re-run inventory"}
          </Button>
          <PruneButton />
        </div>
      </PageHeader>

      {output === undefined ? (
        <Card>
          {images.isError ? (
            <EmptyState>
              Inventory unavailable. {unavailableMessage(images.error)} Nothing is shown rather than a guessed list.
            </EmptyState>
          ) : (
            <EmptyState>Running inventory…</EmptyState>
          )}
        </Card>
      ) : (
        <>
          {images.isError && (
            <Card tone="warn" role="alert">
              <EmptyState>Last re-run failed. {unavailableMessage(images.error)} Showing the earlier result.</EmptyState>
            </Card>
          )}
          {inventory ? <Inventory inventory={inventory} /> : <Raw output={output} />}
        </>
      )}
    </>
  );
}

function Stat({ label, value, hint, tone }: { label: string; value: string | number; hint: string; tone?: "ok" | "warn" }) {
  return (
    <Card className={styles.stat} role="group" aria-label={label}>
      <span className={styles.label}>{label}</span>
      <span className={styles.value} data-tone={tone}>{value}</span>
      <span className={`muted ${styles.hint}`}>{hint}</span>
    </Card>
  );
}

function Raw({ output }: { output: string }) {
  return (
    <Card as="section" aria-labelledby="raw-title">
      <CardHeader title="Raw output" titleId="raw-title">
        <span className={`muted ${styles.note}`}>Not in the expected format; shown as reported</span>
      </CardHeader>
      <pre className={styles.raw}>{output || "(empty output)"}</pre>
    </Card>
  );
}

function Inventory({ inventory }: { inventory: NonNullable<ReturnType<typeof parseImages>> }) {
  const rows = inventory.projects.flatMap((p) => p.rows);
  const count = (s: ImageState) => rows.filter((r) => r.state === s).length;
  const reclaim = inventory.projects.reduce((sum, p) => sum + p.reclaimableBytes, 0);
  const candidates = count("candidate");

  return (
    <>
      <div className={styles.stats}>
        <Stat label="Revisions" value={rows.length} hint={`${inventory.projects.length} project${inventory.projects.length === 1 ? "" : "s"}`} />
        <Stat label="In use" value={count("running")} tone={count("running") ? "ok" : undefined} hint="running containers" />
        <Stat label="Protected" value={count("keep")} hint="recorded releases" />
        <Stat
          label="Reclaimable"
          value={formatBytes(reclaim)}
          tone={candidates ? "warn" : undefined}
          hint={`${candidates} revision${candidates === 1 ? "" : "s"} · estimate`}
        />
      </div>
      <Card as="section" aria-labelledby="inv-title">
        <CardHeader title="Inventory" titleId="inv-title">
          <span className={`muted ${styles.note}`}>Fails closed if release state or Docker inspection is unavailable</span>
        </CardHeader>
        {inventory.projects.length === 0 ? (
          <EmptyState>No deployable projects in the inventory.</EmptyState>
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>Image</Th>
                <Th align="right">Size</Th>
                <Th>State</Th>
              </tr>
            </thead>
            <tbody>
              {inventory.projects.map((p) => (
                <Fragment key={p.name}>
                  <tr className={styles.group}>
                    <td colSpan={3}>{p.name}</td>
                  </tr>
                  {p.rows.map((r) => (
                    <tr key={`${r.state}/${r.reference}`}>
                      <Td className="mono">{r.reference}</Td>
                      <Td align="right" className="mono">{r.sizeBytes != null ? formatBytes(r.sizeBytes) : "—"}</Td>
                      <Td>
                        <Badge tone={STATE[r.state].tone}>{STATE[r.state].label}</Badge>
                      </Td>
                    </tr>
                  ))}
                </Fragment>
              ))}
            </tbody>
          </Table>
        )}
      </Card>
    </>
  );
}
