import { describe, expect, it } from "vitest";

import { hostAvailable, hostMissing } from "../test/fixtures";
import { breadcrumbs } from "./breadcrumbs";
import { summarizeHost } from "./hostSummary";

describe("breadcrumbs", () => {
  it("names top-level pages", () => {
    expect(breadcrumbs("/")).toEqual([{ label: "Overview" }]);
    expect(breadcrumbs("/images")).toEqual([{ label: "Images" }]);
    expect(breadcrumbs("/host")).toEqual([{ label: "Host" }]);
  });

  it("links parents of a job and a project tab, decoding names", () => {
    expect(breadcrumbs("/activity/j1")).toEqual([
      { label: "Activity", link: { to: "/activity" } },
      { label: "Job j1" },
    ]);
    expect(breadcrumbs("/projects/my%20app/logs")).toEqual([
      { label: "Projects", link: { to: "/" } },
      { label: "my app", link: { to: "/projects/$name/overview", params: { name: "my app" } } },
      { label: "Logs" },
    ]);
  });
});

describe("summarizeHost", () => {
  it("summarises CPU, RAM and the fullest disk", () => {
    expect(summarizeHost(hostAvailable)).toMatchObject({
      level: "normal",
      detail: "CPU 18 % · RAM 61 % · disk 72 %",
      ageSeconds: 42,
    });
  });

  it("is Unknown without a snapshot, with the server's reason", () => {
    expect(summarizeHost(hostMissing)).toMatchObject({ level: "unknown", detail: "Host snapshot unavailable" });
    expect(summarizeHost(undefined).level).toBe("unknown");
  });

  it("ignores load, Docker and container state", () => {
    const host = structuredClone(hostAvailable);
    host.snapshot!.load.status = "critical";
    host.snapshot!.docker.status = "critical";
    host.snapshot!.containers_status = "critical";
    expect(summarizeHost(host).level).toBe("normal");
  });

  it("is Unknown, not stale-healthy, when the server marks the snapshot stale", () => {
    const stale = { ...hostAvailable, status: "unknown", age_seconds: 900 };
    expect(summarizeHost(stale)).toMatchObject({ level: "unknown", ageSeconds: 900 });
  });
});
