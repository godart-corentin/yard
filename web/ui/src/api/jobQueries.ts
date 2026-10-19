import { useInfiniteQuery, useQuery } from "@tanstack/react-query";

import { api } from "./client";
import type { JobState } from "./types/JobState";

export const JOBS_REFRESH_MS = 5_000;
export const JOBS_PAGE_SIZE = 25;

/**
 * Running jobs, polled. Drives the sidebar pill and the "a job is running"
 * state of action buttons. A failing endpoint just means "none known".
 */
export function useRunningJobs() {
  return useQuery({
    queryKey: ["jobs", "running"],
    queryFn: () => api.listJobs({ state: "running", limit: 50 }),
    refetchInterval: JOBS_REFRESH_MS,
    retry: false,
  });
}

/** One job, polled while it is not finished (the SSE stream is the fast path). */
export function useJob(id: string) {
  return useQuery({
    queryKey: ["jobs", "one", id],
    queryFn: () => api.getJob(id),
    retry: false,
    refetchInterval: (query) => {
      const state = query.state.data?.state;
      return state === "succeeded" || state === "failed" ? false : JOBS_REFRESH_MS * 2;
    },
  });
}

/** Paginated job list, newest first; `state` undefined means all, `project` narrows to one project. */
export function useJobList(state: JobState | undefined, project?: string) {
  return useInfiniteQuery({
    queryKey: ["jobs", "list", state ?? "all", project ?? "*"],
    queryFn: ({ pageParam }) => api.listJobs({ state, project, limit: JOBS_PAGE_SIZE, cursor: pageParam }),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    refetchInterval: JOBS_REFRESH_MS * 2,
    retry: false,
  });
}
