import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  degraded,
  down,
  hostAvailable,
  hostMissing,
  job,
  NOW_MS,
  operational,
  payload,
  unknown,
  urlMonitor,
} from "../test/fixtures";
import { renderApp, stubApi } from "../test/render";

beforeEach(() => vi.useFakeTimers({ toFake: ["Date"], now: NOW_MS }));
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

const projectsTable = () => screen.getByRole("region", { name: "Projects" });
const row = (name: string) =>
  within(projectsTable()).getByRole("link", { name: new RegExp(`^${name}`) }).closest("tr")!;
const stat = (label: string) => screen.getByRole("group", { name: label });

describe("Overview", () => {
  it("shows an all-operational fleet without any attention banner", async () => {
    stubApi({ "/api/status": payload([operational]), "/api/jobs": { jobs: [], next_cursor: null } });
    renderApp();

    expect(await screen.findByText("All operational")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(within(stat("Projects")).getByText("1")).toBeInTheDocument();
    expect(within(stat("Operational")).getByText("2 of 3 services healthy")).toBeInTheDocument();
    expect(within(stat("Attention")).getByText("All clear")).toBeInTheDocument();
    expect(within(stat("Host")).getByText("Healthy")).toBeInTheDocument();
    expect(within(stat("Host")).getByText("CPU 18 % · RAM 61 % · disk 72 %")).toBeInTheDocument();
  });

  it("renders a project row with health, latency, release, services, backup and actions", async () => {
    stubApi({ "/api/status": payload([operational]) });
    renderApp();
    await screen.findByText("All operational");

    const r = within(row("hello-api"));
    expect(r.getByText("Compose · main")).toBeInTheDocument();
    expect(r.getByText("Operational")).toBeInTheDocument();
    expect(r.getByText("142 ms")).toBeInTheDocument();
    expect(r.getByText("abc123def456")).toBeInTheDocument();
    expect(r.getByText("deployed 2 h ago")).toBeInTheDocument();
    for (const service of ["api", "worker", "migrate"]) expect(r.getByText(service)).toBeInTheDocument();
    expect(r.getByText("Today 11:00")).toBeInTheDocument();
    expect(r.getByRole("link", { name: "Logs hello-api" })).toHaveAttribute("href", "/projects/hello-api/logs");
    expect(r.getByRole("link", { name: "Deploy hello-api" })).toHaveAttribute("href", "/projects/hello-api/overview");
  });

  it("raises a banner for a degraded service and counts it as needing attention", async () => {
    stubApi({ "/api/status": payload([operational, degraded]) });
    renderApp();

    const banner = await screen.findByRole("alert");
    expect(within(banner).getByText("Degraded")).toBeInTheDocument();
    expect(within(banner).getByText("notes-app · worker")).toBeInTheDocument();
    expect(within(banner).getByText("heartbeat 4 m 12 s old — limit 2 m")).toBeInTheDocument();
    expect(within(banner).getByRole("link", { name: "Open worker logs" })).toHaveAttribute(
      "href",
      "/projects/notes-app/logs",
    );
    expect(screen.getByText("1 needs attention")).toBeInTheDocument();
    expect(screen.getByText("notes-app degraded")).toBeInTheDocument();
    // A failed backup is shown as such, not as a date alone.
    expect(within(row("notes-app")).getByText("Failed · Yesterday 06:00")).toBeInTheDocument();
  });

  it("treats Down as an outage with its HTTP reason", async () => {
    stubApi({ "/api/status": payload([down]) });
    renderApp();

    const banner = await screen.findByRole("alert");
    expect(within(banner).getByText("Down")).toBeInTheDocument();
    expect(within(banner).getByText("HTTP 503")).toBeInTheDocument();
    expect(within(row("broken")).getByText("Down")).toBeInTheDocument();
    expect(screen.getByText("1 needs attention")).toBeInTheDocument();
  });

  it("keeps Unknown distinct from Down: no banner, not counted as attention or operational", async () => {
    stubApi({ "/api/status": payload([unknown]) });
    renderApp();

    expect(await screen.findAllByText("1 unknown")).toHaveLength(2); // header badge and Attention card
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(within(row("mystery")).getByText("Unknown")).toBeInTheDocument();
    expect(within(stat("Attention")).getByText("0")).toBeInTheDocument();
    expect(within(stat("Operational")).getByText("0")).toBeInTheDocument();
    // Services without probes carry no health claim.
    expect(within(row("mystery")).getByText("web").closest("span")).toHaveTextContent("web");
  });

  it("shows URL monitors without release, backup or actions", async () => {
    stubApi({ "/api/status": payload([urlMonitor]) });
    renderApp();
    await screen.findByText("All operational");

    const r = within(row("docs-site"));
    expect(r.getByText("URL monitor")).toBeInTheDocument();
    expect(r.getByText("not deployed by Yard")).toBeInTheDocument();
    expect(r.getByText("Monitor only")).toBeInTheDocument();
    expect(r.queryByRole("link", { name: /Logs|Deploy/ })).not.toBeInTheDocument();
    expect(screen.getByText("0 deployable · 1 URL monitor")).toBeInTheDocument();
  });

  it("reports an interrupted release even when the project is operational", async () => {
    const pending = {
      ...operational,
      pending_release: { ...operational.release, tag: "newtag", status: "activating" },
    };
    stubApi({ "/api/status": payload([pending]) });
    renderApp();

    const banner = await screen.findByRole("alert");
    expect(within(banner).getByText(/release newtag activating/)).toBeInTheDocument();
  });

  it("renders without a host snapshot and says why", async () => {
    stubApi({ "/api/status": payload([operational], hostMissing) });
    renderApp();
    await screen.findByText("All operational");

    const host = stat("Host");
    expect(within(host).getByText("Unknown")).toBeInTheDocument();
    expect(within(host).getByText("Host snapshot unavailable")).toBeInTheDocument();
    expect(screen.getByText("No snapshot")).toBeInTheDocument();
  });

  it("derives host health only from CPU, RAM and disks", async () => {
    const warm = structuredClone(hostAvailable);
    warm.snapshot!.disks[0]!.status = "critical";
    stubApi({ "/api/status": payload([operational], warm) });
    renderApp();
    await screen.findByText("All operational");
    expect(screen.getByText("Critical")).toBeInTheDocument();
  });

  it("shows a retryable error when status cannot be loaded", async () => {
    stubApi({ "/api/status": new Response("nope", { status: 500 }) });
    renderApp();
    expect(await screen.findByText(/Status unavailable/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Retry now" })).toBeInTheDocument();
  });
});

describe("Recent activity", () => {
  it("lists jobs from /api/jobs", async () => {
    stubApi({
      "/api/status": payload([operational]),
      "/api/jobs": {
        jobs: [
          job({ id: "2", revision: "abc123def4567" }),
          job({ id: "1", action: "backup", project: "kilnr", state: "failed", error: "disk full", finished_at_unix: null }),
        ],
        next_cursor: null,
      },
    });
    renderApp();

    const card = await screen.findByRole("region", { name: "Recent activity" });
    expect(await within(card).findByText("Deploy hello-api")).toBeInTheDocument();
    expect(within(card).getByText("abc123def456 · 1 m 42 s")).toBeInTheDocument();
    expect(within(card).getByText("Succeeded")).toBeInTheDocument();
    expect(within(card).getByText("Backup kilnr")).toBeInTheDocument();
    expect(within(card).getByText("Failed")).toBeInTheDocument();
    expect(within(card).getByText("disk full")).toBeInTheDocument();
    expect(within(card).getAllByText(/corentin · 2 h ago/)).toHaveLength(2);
    expect(within(card).getByRole("link", { name: "View all" })).toHaveAttribute("href", "/activity");
  });

  it.each([404, 503])("stays quiet when the jobs API answers %i", async (status) => {
    stubApi({ "/api/status": payload([operational]), "/api/jobs": new Response("x", { status }) });
    renderApp();

    const card = await screen.findByRole("region", { name: "Recent activity" });
    expect(await within(card).findByText("Activity is not available right now.")).toBeInTheDocument();
    // The rest of the page is unaffected.
    expect(screen.getByText("All operational")).toBeInTheDocument();
  });

  it("says so when there is no activity yet", async () => {
    stubApi({ "/api/status": payload([operational]), "/api/jobs": { jobs: [], next_cursor: null } });
    renderApp();
    expect(await screen.findByText("No activity yet.")).toBeInTheDocument();
  });
});

describe("Status polling", () => {
  it("refetches every 30 seconds and on demand", async () => {
    stubApi({ "/api/status": payload([operational]) });
    const fetchStatus = () =>
      (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls.filter(([url]) => url === "/api/status").length;
    // Real timers except Date: advance the interval by hand.
    vi.useRealTimers();
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"], now: NOW_MS });
    renderApp();
    await screen.findByText("All operational");
    expect(fetchStatus()).toBe(1);

    await vi.advanceTimersByTimeAsync(30_000);
    await waitFor(() => expect(fetchStatus()).toBe(2));

    await userEvent.setup({ advanceTimers: vi.advanceTimersByTime }).click(
      screen.getByRole("button", { name: /Refresh status/ }),
    );
    await waitFor(() => expect(fetchStatus()).toBe(3));
  });

  it("shows how long ago the status was fetched", async () => {
    stubApi({ "/api/status": payload([operational]) });
    renderApp();
    expect(await screen.findByRole("button", { name: "Refresh status" })).toHaveTextContent("0 s ago");
  });
});
