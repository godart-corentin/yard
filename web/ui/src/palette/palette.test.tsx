import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { degraded, job, operational, payload, urlMonitor } from "../test/fixtures";
import { stubFetch } from "../test/harness";
import { renderApp } from "../test/render";

beforeEach(() => {
  // cmdk scrolls the selected item into view and observes its list.
  Element.prototype.scrollIntoView = vi.fn();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
});
afterEach(() => vi.unstubAllGlobals());

const noJobs = { jobs: [], next_cursor: null };
const accepted = job({ id: "20261019T142501Z-3fa9c01b", state: "running", finished_at_unix: null });

function api(extra: Record<string, unknown> = {}) {
  return stubFetch({
    "GET /api/status": payload([operational, degraded, urlMonitor]),
    "GET /api/jobs": noJobs,
    ...extra,
  });
}

const palette = () => screen.findByRole("dialog", { name: "Command palette" });
const option = (name: string | RegExp) => screen.findByRole("option", { name });

describe("command palette", () => {
  it("opens from the Topbar button, closes with Escape and gives focus back", async () => {
    api();
    renderApp("/");
    const button = await screen.findByRole("button", { name: /Search or run a command/ });
    await userEvent.click(button);
    await palette();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(button).toHaveFocus();
  });

  it("opens with Ctrl+K and Cmd+K, but not while typing in another field", async () => {
    api();
    renderApp("/");
    await screen.findByRole("button", { name: /Search or run a command/ });
    await userEvent.keyboard("{Control>}k{/Control}");
    await palette();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await palette();
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    const field = document.createElement("input");
    document.body.append(field);
    field.focus();
    await userEvent.keyboard("{Control>}k{/Control}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    field.remove();
  });

  it("filters, and shows project actions with their CLI command when the query matches a project", async () => {
    api();
    renderApp("/");
    await userEvent.click(await screen.findByRole("button", { name: /Search or run a command/ }));
    await palette();
    expect(screen.queryByText("Actions · hello-api")).not.toBeInTheDocument();
    await userEvent.type(screen.getByRole("combobox"), "hello");
    expect(await screen.findByText("Actions · hello-api")).toBeInTheDocument();
    expect(within(await option(/^Deploy hello-api/)).getByText("yard deploy")).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Collect host metrics/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /notes-app/ })).not.toBeInTheDocument();
  });

  it("opens the same deploy confirmation, and runs nothing before confirming", async () => {
    const { calls } = api({ "POST /api/jobs": () => new Response(JSON.stringify(accepted), { status: 202 }) });
    const { router } = renderApp("/projects/hello-api/overview");
    await userEvent.keyboard("{Control>}k{/Control}");
    await palette();
    await userEvent.click(await option(/^Deploy hello-api/));
    const dialog = await screen.findByRole("dialog", { name: "Deploy hello-api?" });
    expect(dialog).toHaveTextContent("Git update");
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Deploy" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/activity/${accepted.id}`));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ action: "deploy", project: "hello-api" });
  });

  it("opens the backup, host collect and prune confirmations", async () => {
    api();
    renderApp("/projects/hello-api/overview");
    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/^Back up hello-api/));
    expect(await screen.findByRole("dialog", { name: "Back up hello-api?" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/Collect host metrics/));
    expect(await screen.findByRole("dialog", { name: "Collect host metrics?" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));

    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/Simulate image prune/));
    expect(await screen.findByRole("dialog", { name: /prune/i })).toBeInTheDocument();
  });

  it("navigates: restore goes to the releases tab, logs and pages too", async () => {
    api();
    const { router } = renderApp("/projects/hello-api/overview");
    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/^Restore hello-api…/));
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/hello-api/releases"));
    expect(screen.queryByRole("dialog", { name: "Command palette" })).not.toBeInTheDocument();

    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/^Follow hello-api logs/));
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/hello-api/logs"));

    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/notes-app/));
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/notes-app/overview"));

    await userEvent.keyboard("{Control>}k{/Control}");
    await userEvent.click(await option(/^Images/));
    await waitFor(() => expect(router.state.location.pathname).toBe("/images"));
  });

  it("offers navigation only for URL monitors", async () => {
    api();
    const { router } = renderApp("/");
    await userEvent.click(await screen.findByRole("button", { name: /Search or run a command/ }));
    await userEvent.type(await screen.findByRole("combobox"), "docs-site");
    const item = await option(/docs-site/);
    expect(screen.queryByText(/^Actions ·/)).not.toBeInTheDocument();
    expect(screen.queryByRole("option", { name: /Deploy|Back up|Restore|logs/ })).not.toBeInTheDocument();
    await userEvent.click(item);
    await waitFor(() => expect(router.state.location.pathname).toBe("/projects/docs-site/overview"));
  });
});

describe("project Activity tab", () => {
  it("lists only the jobs of the project", async () => {
    const { calls } = api({
      "GET /api/jobs": () => ({ jobs: [job({ id: "20261019T100000Z-aaaa1111", action: "backup" })], next_cursor: null }),
    });
    renderApp("/projects/hello-api/activity");
    expect(await screen.findByText("#aaaa1111")).toBeInTheDocument();
    expect(screen.queryByRole("columnheader", { name: "Project" })).not.toBeInTheDocument();
    await waitFor(() => expect(calls.some((c) => c.path === "/api/jobs" && c.search.includes("project=hello-api"))).toBe(true));
  });
});
