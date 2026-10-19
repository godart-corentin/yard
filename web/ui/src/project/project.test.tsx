import { screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  job,
  NOW_MS,
  NOW_UNIX,
  operational,
  payload,
  project,
  urlMonitor,
} from "../test/fixtures";
import { renderApp, stubApi } from "../test/render";

beforeEach(() => vi.useFakeTimers({ toFake: ["Date"], now: NOW_MS }));
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const release = (tag: string) => ({
  record_status: "recorded",
  tag,
  revision: `${tag}ffff`,
  deployed_at_unix: NOW_UNIX - 7200,
  status: "active",
  services: [{ name: "api", image: `hello-api:${tag}` }],
});

const full = {
  ...operational,
  release: release("abc123"),
  previous_release: { ...release("7c41e0"), status: "stopped" },
  restore_points: [
    { position: "previous", id: "release:7c41e0", deployed_at_unix: NOW_UNIX - 4 * 86400, status: "stopped" },
    { position: "current", id: "release:abc123", deployed_at_unix: NOW_UNIX - 7200, status: "active" },
  ],
  last_offsite: { started_at_unix: NOW_UNIX - 600, duration_ms: 1, result: "success" as const },
  offsite_configured: true,
};

const jobs = { jobs: [job({ id: "j1", action: "backup", state: "failed", error: "disk full" })], next_cursor: null };

describe("Project overview tab", () => {
  it("shows header, tabs, services, release, backups and jobs", async () => {
    stubApi({ "/api/status": payload([full]), "/api/jobs": jobs });
    renderApp("/projects/hello-api/overview");

    expect(await screen.findByText("Operational")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "hello-api" })).toBeInTheDocument();
    expect(screen.getByText("Operational")).toBeInTheDocument();
    expect(screen.getByText(/branch main/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Deploy / })).toHaveTextContent("Deploy main");
    const tabs = within(screen.getByRole("navigation", { name: "Project views" }));
    expect(tabs.getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");
    expect(tabs.getAllByRole("link")).toHaveLength(4);

    const services = within(screen.getByRole("region", { name: "Services" }));
    expect(services.getByText("hello-api:abc123")).toBeInTheDocument();
    expect(services.getAllByText("Healthy").length).toBeGreaterThan(0);
    expect(services.getByText("worker")).toBeInTheDocument();

    const rel = within(screen.getByRole("region", { name: "Release" }));
    expect(rel.getByText("abc123")).toBeInTheDocument();
    expect(rel.getByText("7c41e0")).toBeInTheDocument();
    expect(within(screen.getByRole("region", { name: "Backups" })).getAllByText("OK")).toHaveLength(2);
    expect(await screen.findByText(/disk full/)).toBeInTheDocument();
  });

  it("handles a missing state record and unavailable jobs quietly", async () => {
    stubApi({ "/api/status": payload([project({ name: "fresh", application_services: ["web"] })]) });
    renderApp("/projects/fresh/overview");

    expect(await screen.findByText("No release recorded")).toBeInTheDocument();
    expect(screen.getByText(/No CLI diagnosis/)).toBeInTheDocument();
    expect(screen.getByText("None recorded")).toBeInTheDocument();
    expect(await screen.findByText("Jobs are not available right now.")).toBeInTheDocument();
  });

  it("flags an invalid release record and an invalid backup", async () => {
    const bad = project({
      name: "broken",
      state_status: "invalid",
      release: { ...release("x"), record_status: "invalid" },
      last_backup: { result: "garbled" },
    });
    stubApi({ "/api/status": payload([bad]) });
    renderApp("/projects/broken/overview");

    expect(await screen.findByText(/release record is unreadable/)).toBeInTheDocument();
    expect(screen.getByText("Invalid record")).toBeInTheDocument();
  });

  it("shows a bare monitor view for URL monitors", async () => {
    stubApi({ "/api/status": payload([urlMonitor]) });
    renderApp("/projects/docs-site/overview");

    expect(await screen.findByText(/Yard only checks this URL/)).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 1, name: "docs-site" })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Project views" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Deploy|Backup/ })).not.toBeInTheDocument();
  });

  it("says when the project does not exist", async () => {
    stubApi({ "/api/status": payload([full]) });
    renderApp("/projects/nope/overview");
    expect(await screen.findByRole("heading", { name: "Project not found" })).toBeInTheDocument();
  });
});

describe("Releases & backups tab", () => {
  it("lists restore points, backup attempts and the restore journal", async () => {
    stubApi({
      "/api/status": payload([full]),
      "/api/read": { output: "2026-10-15 restored release:7c41e0 ok\n" },
    });
    renderApp("/projects/hello-api/releases");

    const points = within(await screen.findByRole("region", { name: "Restore points" }));
    expect(points.getByText("release:7c41e0")).toBeInTheDocument();
    expect(points.getByText("Running now")).toBeInTheDocument();
    expect(points.getAllByRole("button", { name: /^Restore/ })).toHaveLength(1);
    const attempts = within(screen.getByRole("region", { name: "Backup attempts" }));
    expect(attempts.getByText("Offsite")).toBeInTheDocument();
    expect(await screen.findByText(/restored release:7c41e0 ok/)).toBeInTheDocument();
  });

  it("shows honest empty and unavailable states", async () => {
    stubApi({ "/api/status": payload([project({ name: "fresh" })]) });
    renderApp("/projects/fresh/releases");

    expect(await screen.findByText("No restore points recorded yet.")).toBeInTheDocument();
    expect(await screen.findByText("The restore journal is not available right now.")).toBeInTheDocument();
  });

  it("does not list restore points for an invalid state record", async () => {
    stubApi({ "/api/status": payload([project({ name: "broken", state_status: "invalid" })]), "/api/read": { output: "" } });
    renderApp("/projects/broken/releases");

    expect(await screen.findByText(/state record is unreadable/)).toBeInTheDocument();
    expect(await screen.findByText("No restores recorded.")).toBeInTheDocument();
  });
});
