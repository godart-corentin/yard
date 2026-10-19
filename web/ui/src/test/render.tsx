import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render } from "@testing-library/react";
import { vi } from "vitest";

import { makeRouter } from "../router";


/** Stubs `fetch`; unspecified `/api/jobs` paths answer 404 like a server without jobs. */
export function stubApi(routes: Record<string, unknown | (() => unknown)>) {
  const fn = vi.fn(async (input: RequestInfo | URL) => {
    const path = String(input).split("?")[0] ?? "";
    if (path in routes) {
      const value = routes[path];
      const body = typeof value === "function" ? (value as () => unknown)() : value;
      if (body instanceof Response) return body;
      return new Response(JSON.stringify(body));
    }
    return new Response("Not found\n", { status: 404 });
  });
  vi.stubGlobal("fetch", fn);
  return fn;
}

export function renderApp(path = "/") {
  const router = makeRouter(createMemoryHistory({ initialEntries: [path] }));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, client, ...utils };
}
