import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";

import { api, type CreateJobRequest } from "../api";
import { describeJobError } from "./errors";

/** Starts a job, then opens its live page. Refreshes the jobs queries either way. */
export function useStartJob(onStarted?: () => void) {
  const navigate = useNavigate();
  const client = useQueryClient();
  const mutation = useMutation({
    mutationFn: (request: CreateJobRequest) => api.createJob(request),
    onSuccess: (job) => {
      void client.invalidateQueries({ queryKey: ["jobs"] });
      onStarted?.();
      void navigate({ to: "/activity/$jobId", params: { jobId: job.id } });
    },
    onError: () => void client.invalidateQueries({ queryKey: ["jobs"] }),
  });
  return {
    start: mutation.mutate,
    pending: mutation.isPending,
    error: mutation.error ? describeJobError(mutation.error) : null,
    reset: mutation.reset,
  };
}
