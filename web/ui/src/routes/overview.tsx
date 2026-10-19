import { useQuery } from "@tanstack/react-query";

import { api } from "../api";

/** Placeholder: the real pages arrive in later tasks. */
export function OverviewPage() {
  const status = useQuery({ queryKey: ["status"], queryFn: api.status, refetchInterval: 30_000 });
  return (
    <main className="page">
      <h1>Yard</h1>
      <p className="muted" data-testid="overall">
        {status.isPending ? "Loading…" : status.isError ? "Status unavailable" : status.data.status}
      </p>
    </main>
  );
}
