import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { renderApp, stubApi } from "../test/render";
import { parseImages } from "./parse";

afterEach(() => vi.unstubAllGlobals());

const OUTPUT = `Project: hello-api
  keep hello-api:abc123def456
  keep hello-api:7c41e0a9b2d1 (running container)
  candidate hello-api:e2b8a1c04d97 (1048576 bytes)
  candidate hello-api:0d9f3b6e21aa (1048576 bytes)
  reclaimable: 2097152 bytes (estimate; shared layers may reduce savings)
Project: kilnr
  keep kilnr:9f1e07c2b4aa
  reclaimable: 0 bytes (estimate; shared layers may reduce savings)
Simulation only; run yard images --prune --yes to remove candidates.
`;

describe("parseImages", () => {
  it("parses the CLI inventory", () => {
    const inv = parseImages(OUTPUT)!;
    expect(inv.projects.map((p) => p.name)).toEqual(["hello-api", "kilnr"]);
    expect(inv.projects[0]!.rows.map((r) => r.state)).toEqual(["keep", "running", "candidate", "candidate"]);
    expect(inv.projects[0]!.reclaimableBytes).toBe(2097152);
  });

  it("rejects unknown lines, wrong totals and removal output", () => {
    expect(parseImages("hello\n")).toBeNull();
    expect(parseImages(OUTPUT.replace("2097152 bytes (est", "5 bytes (est"))).toBeNull();
    expect(parseImages(OUTPUT.replace("Simulation only", "  removed x\nSimulation only"))).toBeNull();
    expect(parseImages("")).toBeNull();
  });
});

describe("Images page", () => {
  it("shows stat cards and the table, and re-runs on demand", async () => {
    const fn = stubApi({ "/api/read": { output: OUTPUT } });
    renderApp("/images");
    const stat = (l: string) => screen.findByRole("group", { name: l });
    expect(within(await stat("Revisions")).getByText("5")).toBeInTheDocument();
    expect(within(await stat("In use")).getByText("1")).toBeInTheDocument();
    expect(within(await stat("Protected")).getByText("2")).toBeInTheDocument();
    expect(within(await stat("Reclaimable")).getByText("2 MB")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Simulate prune…" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Re-run inventory" }));
    await vi.waitFor(() => expect(fn.mock.calls.filter(([u]) => String(u).includes("op=images")).length).toBe(2));
  });

  it("falls back to raw output when it cannot be parsed", async () => {
    stubApi({ "/api/read": { output: "something unexpected\n" } });
    renderApp("/images");
    expect(await screen.findByText(/something unexpected/)).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Revisions" })).not.toBeInTheDocument();
  });

  it("shows an unavailable state for 503", async () => {
    stubApi({ "/api/read": new Response(JSON.stringify({ error: "x", code: "executor_unavailable" }), { status: 503 }) });
    renderApp("/images");
    expect(await screen.findByText(/could not answer/)).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Revisions" })).not.toBeInTheDocument();
  });
});
