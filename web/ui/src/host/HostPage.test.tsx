import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { vi } from "vitest";

import { hostAvailable, hostMissing, operational, payload } from "../test/fixtures";
import { renderApp, stubApi } from "../test/render";

afterEach(() => vi.unstubAllGlobals());

describe("Host page", () => {
  it("shows measurements for a fresh snapshot", async () => {
    stubApi({ "/api/status": payload([operational]) });
    renderApp("/host");
    const cpu = await screen.findByRole("group", { name: "CPU" });
    expect(within(cpu).getByText("18 %")).toBeInTheDocument();
    expect(within(screen.getByRole("group", { name: "Memory" })).getByText("61 %")).toBeInTheDocument();
    expect(screen.getByText(/Snapshot 42 s ago · collected/)).toBeInTheDocument();
    expect(screen.getByText("Healthy")).toBeInTheDocument();
    expect(screen.getByText("/")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Collect now" })).toBeInTheDocument();
  });

  it("flags a disk warning and lists containers", async () => {
    const snapshot = structuredClone(hostAvailable.snapshot!);
    snapshot.disks = [{ value: { mount: "/backup", used_bytes: 86, total_bytes: 100 }, status: "warning", message: null }];
    snapshot.containers = [{ project: "hello-api", service: "api", state: "running", status: "normal" }];
    stubApi({ "/api/status": payload([operational], { ...hostAvailable, snapshot }) });
    renderApp("/host");
    expect(await screen.findByText("1 warning")).toBeInTheDocument();
    expect(screen.getByText(/86 %/)).toBeInTheDocument();
    expect(screen.getByText("Warning")).toBeInTheDocument();
    expect(screen.getByRole("cell", { name: "api" })).toBeInTheDocument();
  });

  it("is Unknown with no snapshot", async () => {
    stubApi({ "/api/status": payload([operational], hostMissing) });
    renderApp("/host");
    expect(await screen.findByText("Unknown")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "CPU" })).not.toBeInTheDocument();
  });

  it("is Unknown for a stale snapshot but shows its age", async () => {
    const stale = { status: "stale", age_seconds: 900, snapshot: null, message: "Host snapshot is stale" };
    stubApi({ "/api/status": payload([operational], stale) });
    renderApp("/host");
    expect(await screen.findByText("Unknown")).toBeInTheDocument();
    expect(screen.getByText(/last snapshot 15 m ago/)).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "CPU" })).not.toBeInTheDocument();
  });
});
