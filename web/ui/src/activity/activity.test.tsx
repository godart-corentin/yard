import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { job, operational, payload } from "../test/fixtures";
import { FakeEventSource, stubFetch } from "../test/harness";
import { renderApp } from "../test/render";
import { deriveSteps } from "./steps";

const ID = "20261019T142501Z-3fa9c01b";
const running = job({ id: ID, state: "running", finished_at_unix: null, exit_code: null, revision: "abc123def456" });
const chunk = (offset: number, text: string) => ({ offset, next_offset: offset + new TextEncoder().encode(text).length, text });

beforeEach(() => FakeEventSource.install());
afterEach(() => vi.unstubAllGlobals());

function jobApi(current = running) {
  return stubFetch({
    "GET /api/status": payload([operational]),
    [`GET /api/jobs/${ID}`]: current,
    "GET /api/jobs": { jobs: [], next_cursor: null },
  });
}

describe("job page", () => {
  it("streams output, derives steps, resumes after a dropped connection and stops at end", async () => {
    jobApi();
    renderApp(`/activity/${ID}`);
    expect(await screen.findByRole("heading", { level: 1, name: "Deploy hello-api" })).toBeInTheDocument();
    expect(screen.getByText(/can't be cancelled/)).toBeInTheDocument();
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    const first = FakeEventSource.last;
    expect(first.url).toBe(`/api/jobs/${ID}/output?offset=0`);

    const a = chunk(0, "✓ Git update\n");
    const b = chunk(a.next_offset, "✓ Backup\nDeploying hello-api\n");
    act(() => {
      first.emit("open");
      first.emit("state", running);
      first.emit("output", a);
      first.emit("output", b);
    });
    const log = screen.getByRole("log", { name: "Job output" });
    expect(log).toHaveTextContent("✓ Git update");
    expect(log).toHaveTextContent("Deploying hello-api");
    const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
    expect(steps.map((s) => s.getAttribute("data-state"))).toEqual(["done", "done", "now", "todo", "todo"]);
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "2");

    // The browser gave up: the page reopens at the last offset, without replaying text.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    act(() => first.fail());
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2500);
    });
    vi.useRealTimers();
    expect(FakeEventSource.instances).toHaveLength(2);
    const second = FakeEventSource.last;
    expect(second.url).toBe(`/api/jobs/${ID}/output?offset=${b.next_offset}`);
    act(() => {
      second.emit("open");
      second.emit("output", b); // a replayed chunk is ignored
      second.emit("output", chunk(b.next_offset, "✓ Build\n"));
    });
    expect(log.textContent?.match(/Backup/g)).toHaveLength(1);
    expect(log).toHaveTextContent("✓ Build");

    const done = { ...running, state: "succeeded" as const, finished_at_unix: running.started_at_unix! + 64, exit_code: 0 };
    act(() => second.emit("end", done));
    expect(second.closed).toBe(true);
    expect(await screen.findByText(/Succeeded · 1 m 4 s/)).toBeInTheDocument();
    expect(screen.queryByText(/can't be cancelled/)).not.toBeInTheDocument();
    expect(screen.getByText("Complete")).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(2);
  });

  it("copies the whole output", async () => {
    jobApi();
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderApp(`/activity/${ID}`);
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => FakeEventSource.last.emit("output", chunk(0, "hello\n")));
    await userEvent.click(screen.getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith("hello\n");
  });

  it("shows a failed job with its error and the failing step", async () => {
    jobApi(job({ id: ID, state: "failed", error: "exit code 2", exit_code: 2 }));
    renderApp(`/activity/${ID}`);
    expect(await screen.findByText(/Failed · /)).toBeInTheDocument();
    expect(screen.getByText(/exit code 2/)).toBeInTheDocument();
    await waitFor(() => expect(FakeEventSource.instances).toHaveLength(1));
    act(() => FakeEventSource.last.emit("output", chunk(0, "✓ Git update\n")));
    const steps = within(screen.getByRole("region", { name: "Steps" })).getAllByRole("listitem");
    expect(steps.map((s) => s.getAttribute("data-state"))).toEqual(["done", "failed", "todo", "todo", "todo"]);
  });

  it("explains an unknown job", async () => {
    stubFetch({ "GET /api/status": payload([operational]) });
    renderApp("/activity/nope");
    expect(await screen.findByText(/does not exist/)).toBeInTheDocument();
    expect(FakeEventSource.instances).toHaveLength(0);
  });
});

describe("deriveSteps", () => {
  it("marks every deploy step done on success and notes a skipped migration", () => {
    const steps = deriveSteps(job({ id: "x", state: "succeeded" }), "✓ Git update\n✓ Backup\n✓ Build\nHealthy: a @ b\n");
    expect(steps.every((s) => s.state === "done")).toBe(true);
    expect(steps.find((s) => s.label === "Migrations")?.note).toBe("not configured");
  });
  it("uses one step for other actions", () => {
    expect(deriveSteps(job({ id: "x", action: "backup", state: "running" }), "")).toEqual([{ label: "Run backup", state: "now" }]);
  });
});

describe("sidebar", () => {
  it("shows how many jobs are running", async () => {
    stubFetch({ "GET /api/status": payload([operational]), "GET /api/jobs": { jobs: [running, { ...running, id: "b" }], next_cursor: null } });
    renderApp("/");
    expect(await screen.findByText("2 running")).toBeInTheDocument();
  });
});

describe("activity page", () => {
  const rows = [
    job({ id: "20261019T120000Z-aaaaaaaa", state: "running", finished_at_unix: null }),
    job({ id: "20261019T110000Z-bbbbbbbb", action: "backup", state: "failed", error: "exit 2" }),
  ];

  it("filters by result with the state query and links rows to job pages", async () => {
    const { calls } = stubFetch({
      "GET /api/status": payload([operational]),
      "GET /api/jobs": (call: { search: string }) => {
        const state = new URLSearchParams(call.search).get("state");
        return { jobs: state ? rows.filter((r) => r.state === state) : rows, next_cursor: null };
      },
    });
    renderApp("/activity");
    const table = await screen.findByRole("table");
    expect(within(table).getAllByRole("row")).toHaveLength(3);
    expect(screen.getByRole("button", { name: "All" })).toHaveAttribute("aria-pressed", "true");

    await userEvent.click(screen.getByRole("button", { name: "Failed" }));
    await waitFor(() => expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(2));
    expect(calls.some((c) => c.search.includes("state=failed"))).toBe(true);
    const link = screen.getByRole("link", { name: /Output of Backup hello-api bbbbbbbb/ });
    expect(link).toHaveAttribute("href", "/activity/20261019T110000Z-bbbbbbbb");
  });

  it("paginates with the cursor", async () => {
    const { calls } = stubFetch({
      "GET /api/status": payload([operational]),
      "GET /api/jobs": (call: { search: string }) =>
        new URLSearchParams(call.search).get("cursor")
          ? { jobs: [rows[1]], next_cursor: null }
          : { jobs: [rows[0]], next_cursor: rows[0]!.id },
    });
    renderApp("/activity");
    await userEvent.click(await screen.findByRole("button", { name: "Load more" }));
    await waitFor(() => expect(within(screen.getByRole("table")).getAllByRole("row")).toHaveLength(3));
    expect(calls.some((c) => c.search.includes(`cursor=${rows[0]!.id}`))).toBe(true);
    expect(screen.queryByRole("button", { name: "Load more" })).not.toBeInTheDocument();
  });

  it("explains an empty filter", async () => {
    stubFetch({ "GET /api/status": payload([operational]), "GET /api/jobs": { jobs: [], next_cursor: null } });
    renderApp("/activity");
    expect(await screen.findByText("No activity yet.")).toBeInTheDocument();
  });
});

describe("recent activity", () => {
  it("links each row to its job page", async () => {
    stubFetch({ "GET /api/status": payload([operational]), "GET /api/jobs": { jobs: [running], next_cursor: null } });
    renderApp("/");
    const link = await screen.findByRole("link", { name: "Deploy hello-api" });
    expect(link).toHaveAttribute("href", `/activity/${ID}`);
  });
});
