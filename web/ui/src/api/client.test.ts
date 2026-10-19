import { afterEach, describe, expect, it, vi } from "vitest";

import { api, CSRF_HEADER, jobOutputUrl, YardApiError } from "./client";

function mockFetch(status: number, body: unknown) {
  const fn = vi.fn(async () => new Response(JSON.stringify(body), { status }));
  vi.stubGlobal("fetch", fn);
  return fn;
}

afterEach(() => vi.unstubAllGlobals());

describe("api client", () => {
  it("sends the CSRF header only on mutating requests", async () => {
    const fn = mockFetch(200, { id: "j1" });
    await api.createJob({ action: "deploy", project: "demo" });
    await api.getJob("j1");
    const calls = fn.mock.calls as unknown as [string, RequestInit][];
    expect(new Headers(calls[0]![1].headers).get(CSRF_HEADER)).toBe("1");
    expect(new Headers(calls[1]![1].headers).has(CSRF_HEADER)).toBe(false);
  });

  it("raises a typed error carrying the server body", async () => {
    mockFetch(409, { error: "busy", code: "conflict", running_job_id: "j0" });
    const error = await api.listJobs({ project: "demo" }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(YardApiError);
    expect((error as YardApiError).body?.running_job_id).toBe("j0");
  });

  it("builds the SSE resume URL", () => {
    expect(jobOutputUrl("a/b", 12)).toBe("/api/jobs/a%2Fb/output?offset=12");
  });
});
