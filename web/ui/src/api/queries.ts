import { useQuery } from "@tanstack/react-query";

import { api } from "./client";

export const STATUS_REFRESH_MS = 30_000;

/** Shared by the shell (sidebar, refresh button) and pages: one request per interval. */
export function useStatus() {
  return useQuery({ queryKey: ["status"], queryFn: api.status, refetchInterval: STATUS_REFRESH_MS });
}

/**
 * Recent jobs. `/api/jobs` may be missing or unavailable (older server, no
 * executor), so a failure is a quiet state rather than something to retry hard.
 */
export function useRecentJobs(limit = 5) {
  return useQuery({
    queryKey: ["jobs", { limit }],
    queryFn: () => api.listJobs({ limit }),
    refetchInterval: STATUS_REFRESH_MS,
    retry: false,
  });
}
