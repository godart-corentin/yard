import { useQuery } from "@tanstack/react-query";

import { api } from "../api";
import { STATUS_REFRESH_MS, useStatus } from "../api/queries";

/** The named project from the shared status poll, with the status query itself. */
export function useProject(name: string) {
  const status = useStatus();
  const project = status.data?.projects.find((p) => p.name === name) ?? null;
  return { status, project };
}

/** Jobs for one project. `/api/jobs` may be missing, so failure is a quiet state. */
export function useProjectJobs(project: string, limit = 5) {
  return useQuery({
    queryKey: ["jobs", { project, limit }],
    queryFn: () => api.listJobs({ project, limit }),
    refetchInterval: STATUS_REFRESH_MS,
    retry: false,
  });
}

/** One-shot CLI text from the read executor. Only worth asking for deployable projects. */
export function useRead(op: "restore-log", project: string) {
  return useQuery({
    queryKey: ["read", op, project],
    queryFn: () => api.read({ op, project }),
    refetchInterval: STATUS_REFRESH_MS,
    retry: false,
  });
}
