import { createRootRoute, createRoute, createRouter, Outlet } from "@tanstack/react-router";

import { OverviewPage } from "./routes/overview";

// Code-based routing: one flat route tree, easy to extend to ~10 routes.
const rootRoute = createRootRoute({ component: () => <Outlet /> });

const overviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: OverviewPage,
});

export const routeTree = rootRoute.addChildren([overviewRoute]);

export function makeRouter(options?: Parameters<typeof createRouter>[0]["history"]) {
  return createRouter({ routeTree, history: options });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof makeRouter>;
  }
}
