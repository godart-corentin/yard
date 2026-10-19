import type { ApiError as ApiErrorBody } from "./types/ApiError";
import type { CreateJobRequest } from "./types/CreateJobRequest";
import type { Job } from "./types/Job";
import type { JobList } from "./types/JobList";
import type { ListJobsQuery } from "./types/ListJobsQuery";
import type { LogsStreamQuery } from "./types/LogsStreamQuery";
import type { ReadOutput } from "./types/ReadOutput";
import type { StatusPayload } from "./types/StatusPayload";

/** Sent on every mutating request; the server rejects mutations without it (CSRF). */
export const CSRF_HEADER = "X-Yard-Request";

export class YardApiError extends Error {
  constructor(
    readonly status: number,
    readonly body: ApiErrorBody | null,
  ) {
    super(body?.error ?? `Request failed with status ${status}`);
    this.name = "YardApiError";
  }
}

function query(params: object): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined && value !== null) search.set(key, String(value));
  }
  const text = search.toString();
  return text ? `?${text}` : "";
}

async function request<T>(path: string, init: RequestInit = {}): Promise<T> {
  const mutating = init.method !== undefined && init.method !== "GET";
  const headers = new Headers(init.headers);
  headers.set("Accept", "application/json");
  if (mutating) headers.set(CSRF_HEADER, "1");
  if (init.body !== undefined) headers.set("Content-Type", "application/json");
  const response = await fetch(path, { ...init, headers, credentials: "same-origin" });
  if (!response.ok) {
    let body: ApiErrorBody | null = null;
    try {
      body = (await response.json()) as ApiErrorBody;
    } catch {
      // Non-JSON error bodies are reported by status only.
    }
    throw new YardApiError(response.status, body);
  }
  return (await response.json()) as T;
}

export const api = {
  status: () => request<StatusPayload>("/api/status"),
  read: (params: Record<string, string | number | undefined>) =>
    request<ReadOutput>(`/api/read${query(params)}`),
  createJob: (body: CreateJobRequest) =>
    request<Job>("/api/jobs", { method: "POST", body: JSON.stringify(body) }),
  listJobs: (filters: ListJobsQuery = {}) => request<JobList>(`/api/jobs${query(filters)}`),
  getJob: (id: string) => request<Job>(`/api/jobs/${encodeURIComponent(id)}`),
};

/** URL for an `EventSource` on a job's output; resume with the last `next_offset`. */
export function jobOutputUrl(id: string, offset = 0): string {
  return `/api/jobs/${encodeURIComponent(id)}/output${query({ offset })}`;
}

/** URL for an `EventSource` on live project logs. */
export function logsStreamUrl(params: LogsStreamQuery): string {
  return `/api/logs/stream${query(params)}`;
}
