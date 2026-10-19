import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, RouterProvider } from "@tanstack/react-router";
import { render, screen } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { makeRouter } from "../router";

afterEach(() => vi.unstubAllGlobals());

it("renders the placeholder overview with the server status", async () => {
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response(JSON.stringify({ status: "operational", projects: [] }))),
  );
  const router = makeRouter(createMemoryHistory({ initialEntries: ["/"] }));
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <RouterProvider router={router} />
    </QueryClientProvider>,
  );
  expect(await screen.findByRole("heading", { name: "Yard" })).toBeInTheDocument();
  expect(await screen.findByText("operational")).toBeInTheDocument();
});
