import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  RouterProvider,
} from "@tanstack/react-router";
import { render } from "@testing-library/react";
import type { ReactNode } from "react";
import { vi } from "vitest";

export interface Call {
  method: string;
  path: string;
  search: string;
  headers: Headers;
  body: unknown;
}

type Handler = (call: Call) => unknown;

/**
 * Method-aware `fetch` stub. Keys are "METHOD /path"; a handler may return a body
 * (200) or a `Response`. Unlisted requests answer 404.
 */
export function stubFetch(routes: Record<string, unknown | Handler>) {
  const calls: Call[] = [];
  const fn = vi.fn(async (input: RequestInfo | URL, init: RequestInit = {}) => {
    const url = new URL(String(input), "http://localhost");
    const call: Call = {
      method: init.method ?? "GET",
      path: url.pathname,
      search: url.search,
      headers: new Headers(init.headers),
      body: typeof init.body === "string" ? JSON.parse(init.body) : undefined,
    };
    calls.push(call);
    const route = routes[`${call.method} ${call.path}`];
    if (route === undefined) return new Response("Not found\n", { status: 404 });
    const value = typeof route === "function" ? (route as Handler)(call) : route;
    return value instanceof Response ? value : new Response(JSON.stringify(value));
  });
  vi.stubGlobal("fetch", fn);
  return { calls, fn };
}

export const errorResponse = (status: number, code: string, extra: object = {}) =>
  new Response(JSON.stringify({ error: "generic", code, ...extra }), { status });

/** Renders `ui` at the root of a router that also knows the job pages. */
export function renderIn(ui: ReactNode, path = "/") {
  const root = createRootRoute({ component: () => <>{ui}</> });
  const jobs = createRoute({ getParentRoute: () => root, path: "/activity/$jobId", component: () => <p>job page</p> });
  const index = createRoute({ getParentRoute: () => root, path: "/" });
  const router = createRouter({
    routeTree: root.addChildren([index, jobs]),
    history: createMemoryHistory({ initialEntries: [path] }),
  });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const utils = render(
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  return { router, client, ...utils };
}

type Listener = (event: MessageEvent<string> | Event) => void;

/** Minimal EventSource: tests push events with `emit` and simulate failures with `fail`. */
export class FakeEventSource {
  static instances: FakeEventSource[] = [];
  readyState = 0;
  closed = false;
  private listeners = new Map<string, Listener[]>();
  constructor(readonly url: string) {
    FakeEventSource.instances.push(this);
  }
  addEventListener(type: string, listener: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  close() {
    this.closed = true;
    this.readyState = 2;
  }
  emit(type: string, data?: unknown) {
    if (type === "open") this.readyState = 1;
    const event = data === undefined ? new Event(type) : new MessageEvent(type, { data: JSON.stringify(data) });
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
  /** The browser gave up on the connection (e.g. HTTP error): readyState CLOSED. */
  fail() {
    this.readyState = 2;
    this.emit("error");
  }
  static get last() {
    return FakeEventSource.instances[FakeEventSource.instances.length - 1] as FakeEventSource;
  }
  static install() {
    FakeEventSource.instances = [];
    vi.stubGlobal("EventSource", FakeEventSource);
  }
}
