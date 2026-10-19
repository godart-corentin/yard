import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { degraded, operational, payload, urlMonitor } from "./test/fixtures";
import { renderApp, stubApi } from "./test/render";

afterEach(() => vi.unstubAllGlobals());

const api = () => stubApi({ "/api/status": payload([operational, degraded, urlMonitor]) });

describe("routes", () => {
  it.each([
    ["/", "Overview"],
    ["/activity", "Activity"],
    ["/activity/job-42", "Job job-42"],
    ["/images", "Images"],
    ["/host", "Host"],
    ["/projects/hello-api/overview", "hello-api"],
    ["/projects/hello-api/releases", "hello-api"],
    ["/projects/hello-api/logs", "hello-api"],
    ["/projects/hello-api/activity", "hello-api"],
  ])("%s renders inside the shell", async (path, heading) => {
    api();
    renderApp(path);
    expect(await screen.findByRole("heading", { level: 1, name: heading })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Main" })).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "Breadcrumb" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Search or run a command/ })).toBeInTheDocument();
  });

  it("redirects a bare project URL to its overview tab", async () => {
    api();
    const { router } = renderApp("/projects/hello-api");
    await screen.findByRole("heading", { level: 1, name: "hello-api" });
    expect(router.state.location.pathname).toBe("/projects/hello-api/overview");
  });

  it("navigates from the sidebar and marks the current page", async () => {
    api();
    const user = userEvent.setup();
    const { router } = renderApp("/");
    const main = await screen.findByRole("navigation", { name: "Main" });
    expect(within(main).getByRole("link", { name: "Overview" })).toHaveAttribute("aria-current", "page");

    await user.click(within(main).getByRole("link", { name: "Host" }));
    expect(router.state.location.pathname).toBe("/host");
    expect(await screen.findByRole("heading", { level: 1, name: "Host" })).toBeInTheDocument();
    expect(within(main).getByRole("link", { name: "Host" })).toHaveAttribute("aria-current", "page");
    expect(within(main).getByRole("link", { name: "Overview" })).not.toHaveAttribute("aria-current");
  });

  it("lists projects in the sidebar with URL monitors marked", async () => {
    api();
    renderApp("/");
    const projects = await screen.findByRole("navigation", { name: "Projects" });
    const links = await within(projects).findAllByRole("link");
    expect(links.map((link) => link.textContent)).toEqual(["hello-api", "notes-app", "docs-siteURL"]);
    expect(links[0]).toHaveAttribute("href", "/projects/hello-api");
  });

  it("opens a project from its row and keeps the project highlighted across tabs", async () => {
    api();
    const user = userEvent.setup();
    const { router } = renderApp("/");
    const table = await screen.findByRole("region", { name: "Projects" });
    await user.click(within(table).getByText("Compose · main", { selector: "span" }));
    expect(router.state.location.pathname).toBe("/projects/hello-api/overview");

    await router.navigate({ to: "/projects/$name/logs", params: { name: "hello-api" } });
    const sidebar = screen.getByRole("navigation", { name: "Projects" });
    expect(await within(sidebar).findByRole("link", { name: "hello-api" })).toHaveAttribute("aria-current", "page");
  });

  it("shows breadcrumbs for nested pages", async () => {
    api();
    renderApp("/projects/hello-api/releases");
    const crumbs = await screen.findByRole("navigation", { name: "Breadcrumb" });
    expect(within(crumbs).getByRole("link", { name: "Projects" })).toHaveAttribute("href", "/");
    expect(within(crumbs).getByRole("link", { name: "hello-api" })).toHaveAttribute(
      "href",
      "/projects/hello-api/overview",
    );
    expect(within(crumbs).getByText("Releases & backups")).toHaveAttribute("aria-current", "page");
  });

  it("resets scroll on navigation by letting the router manage it", async () => {
    api();
    const { router } = renderApp("/");
    await screen.findByRole("heading", { level: 1, name: "Overview" });
    expect(router.options.scrollRestoration).toBe(true);
  });
});
