import {
  createRootRoute,
  createRoute,
  createRouter,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";

import { ActivityList, ActivityPage } from "./activity/ActivityPage";
import { JobPage } from "./activity/JobPage";
import { HostPage } from "./host/HostPage";
import { ImagesPage } from "./images/ImagesPage";
import { LogsTab } from "./logs/LogsTab";
import { OverviewPage } from "./overview/OverviewPage";
import { OverviewTab } from "./project/OverviewTab";
import { ProjectLayout } from "./project/ProjectLayout";
import { ReleasesTab } from "./project/ReleasesTab";
import { AppShell } from "./shell/AppShell";

// Code-based routing: one flat route tree, easy to extend. Pages other than the
// overview are placeholders until their own tasks land.
const rootRoute = createRootRoute({ component: AppShell });

const overviewRoute = createRoute({ getParentRoute: () => rootRoute, path: "/", component: OverviewPage });

const projectRoute = createRoute({ getParentRoute: () => rootRoute, path: "/projects/$name", component: ProjectPage });

const projectIndexRoute = createRoute({
  getParentRoute: () => projectRoute,
  path: "/",
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/projects/$name/overview", params, replace: true });
  },
});

function ProjectPage() {
  const { name } = projectRoute.useParams();
  return <ProjectLayout name={name} />;
}

function projectTab(path: "overview" | "releases" | "logs" | "activity") {
  return createRoute({
    getParentRoute: () => projectRoute,
    path,
    component: function ProjectTab() {
      const { name } = projectRoute.useParams();
      if (path === "overview") return <OverviewTab name={name} />;
      if (path === "releases") return <ReleasesTab name={name} />;
      if (path === "logs") return <LogsTab project={name} />;
      return <ActivityList project={name} />;
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
  component: ImagesPage,
});

const hostRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/host",
  component: HostPage,
});

export const routeTree = rootRoute.addChildren([
  overviewRoute,
  projectRoute.addChildren([
    projectIndexRoute,
    projectTab("overview"),
    projectTab("releases"),
    projectTab("logs"),
    projectTab("activity"),
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
