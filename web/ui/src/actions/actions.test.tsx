import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { job, NOW_UNIX, operational, payload } from "../test/fixtures";
import { errorResponse, renderIn, stubFetch } from "../test/harness";
import { BackupButton } from "./BackupButton";
import { HostCollectButton } from "./HostCollectButton";
import { ProjectActions } from "./ProjectActions";
import { PruneButton } from "./PruneButton";
import { RestoreButton } from "./RestoreButton";
import { parsePrunePlan } from "./prunePlan";

afterEach(() => vi.unstubAllGlobals());

const accepted = job({ id: "20261019T142501Z-3fa9c01b", state: "running", finished_at_unix: null });
const noJobs = { jobs: [], next_cursor: null };

const base = {
  "GET /api/status": payload([{ ...operational, restore_points: [{ position: "previous", id: "7c41e0a9b2d1", deployed_at_unix: NOW_UNIX - 4 * 86400, status: "superseded" }] }]),
  "GET /api/jobs": noJobs,
};

const PLAN = `Project: hello-api
  keep hello-api:abc123def456
  candidate hello-api:e2b8a1c04d97 (412000000 bytes)
  candidate hello-api:0d9f3b6e21aa (398000000 bytes)
  reclaimable: 810000000 bytes (estimate; shared layers may reduce savings)
Simulation only; run yard images --prune --yes to remove candidates.
`;

describe("deploy dialog", () => {
  it("lists the steps, posts with the CSRF header and opens the job page", async () => {
    const { calls } = stubFetch({ ...base, "POST /api/jobs": () => new Response(JSON.stringify(accepted), { status: 202 }) });
    const { router } = renderIn(<ProjectActions project={operational} />);
    await userEvent.click(await screen.findByRole("button", { name: "Deploy hello-api" }));
    const dialog = await screen.findByRole("dialog", { name: "Deploy hello-api?" });
    expect(dialog).toHaveTextContent("Git update on main");
    expect(dialog).toHaveTextContent("roll back automatically");
    await userEvent.click(screen.getByRole("button", { name: "Deploy" }));
    await waitFor(() => expect(router.state.location.pathname).toBe(`/activity/${accepted.id}`));
    const post = calls.find((c) => c.method === "POST");
    expect(post?.body).toEqual({ action: "deploy", project: "hello-api" });
    expect(post?.headers.get("X-Yard-Request")).toBe("1");
  });

  it("disables project actions while a job runs for the project", async () => {
    stubFetch({ ...base, "GET /api/jobs": { jobs: [accepted], next_cursor: null } });
    renderIn(<ProjectActions project={operational} />);
    await waitFor(() => expect(screen.getByRole("button", { name: "Backup" })).toBeDisabled());
    expect(screen.getByRole("button", { name: "Deploy hello-api" })).toBeDisabled();
  });

  it("Restore… links to the releases tab", async () => {
    stubFetch(base);
    renderIn(<ProjectActions project={operational} />);
    expect(await screen.findByRole("link", { name: "Restore…" })).toHaveAttribute("href", "/projects/hello-api/releases");
  });
});

describe("error mappings", () => {
  const cases: [number, string, object, RegExp][] = [
    [403, "forbidden", {}, /reverse proxy is not configured for actions/],
    [429, "refused", {}, /busy right now/],
    [503, "executor_unavailable", {}, /executor is unavailable/],
    [400, "refused", {}, /Yard refused this request/],
    [500, "failed", {}, /status 500/],
  ];
  it.each(cases)("maps %i honestly and keeps the dialog open", async (status, code, extra, text) => {
    stubFetch({ ...base, "POST /api/jobs": () => errorResponse(status, code, extra) });
    const { router } = renderIn(<BackupButton project="hello-api" />);
    await userEvent.click(await screen.findByRole("button", { name: "Backup" }));
    await userEvent.click(await screen.findByRole("button", { name: "Back up" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(text);
    expect(router.state.location.pathname).toBe("/");
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("offers a link to the running job on 409", async () => {
    stubFetch({ ...base, "POST /api/jobs": () => errorResponse(409, "conflict", { running_job_id: "20261019T100000Z-aaaaaaaa" }) });
    renderIn(<BackupButton project="hello-api" />);
    await userEvent.click(await screen.findByRole("button", { name: "Backup" }));
    await userEvent.click(await screen.findByRole("button", { name: "Back up" }));
    const link = await screen.findByRole("link", { name: "Open the running job" });
    expect(link).toHaveAttribute("href", "/activity/20261019T100000Z-aaaaaaaa");
  });
});

describe("restore dialog", () => {
  it("needs the project name typed before the danger button works", async () => {
    const { calls } = stubFetch({ ...base, "POST /api/jobs": () => new Response(JSON.stringify({ ...accepted, action: "restore" }), { status: 202 }) });
    renderIn(<RestoreButton project="hello-api" revision="7c41e0a9b2d1" />);
    await userEvent.click(await screen.findByRole("button", { name: /Restore/ }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Application images only");
    const confirm = screen.getByRole("button", { name: "Restore" });
    expect(confirm).toBeDisabled();
    const input = screen.getByRole("textbox");
    await userEvent.type(input, "hello-a");
    expect(confirm).toBeDisabled();
    await userEvent.type(input, "pi");
    expect(confirm).toBeEnabled();
    await userEvent.click(confirm);
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      action: "restore",
      project: "hello-api",
      revision: "7c41e0a9b2d1",
      confirm: true,
    });
  });
});

describe("prune dialog", () => {
  const open = async () => {
    renderIn(<PruneButton />);
    await userEvent.click(await screen.findByRole("button", { name: "Simulate prune…" }));
  };

  it("runs the dry run first, then prunes every candidate with confirm", async () => {
    const { calls } = stubFetch({
      ...base,
      "GET /api/read": { output: PLAN },
      "POST /api/jobs": () => new Response(JSON.stringify({ ...accepted, action: "images-prune", project: null }), { status: 202 }),
    });
    await open();
    expect(await screen.findByText("hello-api:e2b8a1c04d97")).toBeInTheDocument();
    expect(calls.find((c) => c.path === "/api/read")?.search).toBe("?op=images-prune-plan");
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    expect(screen.getByText("810 MB")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toHaveTextContent("never a selection");
    await userEvent.click(screen.getByRole("button", { name: "Prune 2 revisions" }));
    await waitFor(() => expect(calls.find((c) => c.method === "POST")?.body).toEqual({ action: "images-prune", confirm: true }));
  });

  it("shows the raw output when the plan cannot be parsed", async () => {
    stubFetch({ ...base, "GET /api/read": { output: "something unexpected" } });
    await open();
    expect(await screen.findByText("something unexpected")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Prune images" })).toBeEnabled();
  });

  it("blocks pruning when nothing is eligible or the simulation fails", async () => {
    stubFetch({ ...base, "GET /api/read": { output: "Project: hello-api\n  reclaimable: 0 bytes (estimate)\n" } });
    await open();
    expect(await screen.findByText(/Nothing to prune/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Prune 0/ })).toBeDisabled();
  });

  it("blocks pruning when the dry run fails", async () => {
    stubFetch({ ...base, "GET /api/read": () => errorResponse(503, "executor_unavailable") });
    await open();
    expect(await screen.findByRole("alert")).toHaveTextContent("simulation could not run");
    expect(screen.getByRole("button", { name: "Prune images" })).toBeDisabled();
  });
});

describe("host collect", () => {
  it("starts the job without a dialog and reports 403 inline", async () => {
    stubFetch({ ...base, "POST /api/jobs": () => errorResponse(403, "forbidden") });
    renderIn(<HostCollectButton />);
    await userEvent.click(await screen.findByRole("button", { name: "Collect now" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("not configured for actions");
  });
});

describe("parsePrunePlan", () => {
  it("parses candidates and sums bytes", () => {
    expect(parsePrunePlan(PLAN)).toEqual({
      candidates: [
        { reference: "hello-api:e2b8a1c04d97", bytes: 412000000 },
        { reference: "hello-api:0d9f3b6e21aa", bytes: 398000000 },
      ],
      reclaimableBytes: 810000000,
    });
  });
  it("rejects unrecognized or malformed output", () => {
    expect(parsePrunePlan("hello")).toBeNull();
    expect(parsePrunePlan("Project: a\n  candidate broken line")).toBeNull();
  });
});
