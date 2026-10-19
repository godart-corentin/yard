import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";

import { ActivityPage } from "./activity/ActivityPage";
import { JobPage } from "./activity/JobPage";
import { Placeholder } from "./components/Placeholder";
import { OverviewPage } from "./overview/OverviewPage";
import { AppShell } from "./shell/AppShell";

// Code-based routing: one flat route tree, easy to extend. Pages other than the
// overview are placeholders until their own tasks land.
const rootRoute = createRootRoute({ component: AppShell });

const overviewRoute = createRoute({ getParentRoute: () => rootRoute, path: "/", component: OverviewPage });

const projectRoute = createRoute({ getParentRoute: () => rootRoute, path: "/projects/$name" });

const projectIndexRoute = createRoute({
  getParentRoute: () => projectRoute,
  path: "/",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/projects/$name/overview", params, replace: true });
  },
});

function projectTab(path: "overview" | "releases" | "logs" | "activity", title: string) {
  return createRoute({
    getParentRoute: () => projectRoute,
    path,
    component: function ProjectTab() {
      const { name } = projectRoute.useParams();
      return <Placeholder title={name} subtitle={title} />;
    },
  });
}

const activityRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/activity",
  component: ActivityPage,
});

const jobRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/activity/$jobId",
  component: function JobRoute() {
    const { jobId } = jobRoute.useParams();
    return <JobPage jobId={jobId} />;
  },
});

const imagesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/images",
  component: () => <Placeholder title="Images" />,
});

const hostRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/host",
  component: () => <Placeholder title="Host" />,
});

export const routeTree = rootRoute.addChildren([
  overviewRoute,
  projectRoute.addChildren([
    projectIndexRoute,
    projectTab("overview", "Overview"),
    projectTab("releases", "Releases & backups"),
    projectTab("logs", "Logs"),
    projectTab("activity", "Activity"),
  ]),
  activityRoute,
  jobRoute,
  imagesRoute,
  hostRoute,
]);

/** Browser history by default; tests pass a memory history. */
export function makeRouter(history?: RouterHistory) {
  return createRouter({
    routeTree,
    history,
    // New pages start at the top; polling and in-page updates never move the scroll.
    scrollRestoration: true,
    defaultPreload: false,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof makeRouter>;
  }
}
