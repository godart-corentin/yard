import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { operational, payload, project } from "../test/fixtures";
import { stubApi } from "../test/render";
import { LogsTab } from "./LogsTab";
import { MAX_LINES } from "./useLogStream";

class MockEventSource {
  static instances: MockEventSource[] = [];
  closed = false;
  private listeners = new Map<string, Array<(event: Event) => void>>();
  constructor(readonly url: string) {
    MockEventSource.instances.push(this);
  }
  addEventListener(name: string, listener: (event: Event) => void) {
    this.listeners.set(name, [...(this.listeners.get(name) ?? []), listener]);
  }
  close() {
    this.closed = true;
  }
  emit(name: string, data?: unknown) {
    const event = data === undefined ? new Event(name) : new MessageEvent(name, { data: JSON.stringify(data) });
    for (const listener of this.listeners.get(name) ?? []) listener(event);
  }
  line(text: string) {
    this.emit("line", { text });
  }
  get params() {
    return new URL(this.url, "http://x").searchParams;
  }
}

const last = () => MockEventSource.instances.at(-1)!;
const app = project({ name: "notes-app", application_services: ["api", "worker"], status: "operational" });

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <LogsTab project="notes-app" />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  MockEventSource.instances = [];
  vi.stubGlobal("EventSource", MockEventSource);
  stubApi({ "/api/status": payload([operational, app]) });
});
afterEach(() => vi.unstubAllGlobals());

const term = () => screen.getByRole("log", { name: "Log output" });

describe("LogsTab", () => {
  it("opens a stream with the default since and appends lines", async () => {
    mount();
    expect(last().params.get("project")).toBe("notes-app");
    expect(last().params.get("since")).toBe("1h");
    act(() => last().emit("open"));
    expect(screen.getByText("Live")).toBeInTheDocument();
    act(() => {
      last().line("notes-app-api-1  | 2026-10-19T10:02:11Z INFO hello");
      last().line("plain line");
    });
    await waitFor(() => expect(within(term()).getByText("INFO hello")).toBeInTheDocument());
    expect(within(term()).getByText("api")).toBeInTheDocument();
    expect(within(term()).getByText("10:02:11")).toBeInTheDocument();
    expect(within(term()).getByText("plain line")).toBeInTheDocument();
  });

  it("offers All plus the configured application services and restarts on pick", async () => {
    mount();
    const group = await screen.findByRole("button", { name: "worker" });
    expect(screen.getAllByRole("button", { name: /^(All|api|worker)$/ })).toHaveLength(3);
    act(() => last().line("old"));
    await userEvent.click(group);
    expect(MockEventSource.instances[0]!.closed).toBe(true);
    expect(last().params.get("service")).toBe("worker");
    expect(within(term()).queryByText("old")).not.toBeInTheDocument();
  });

  it("keeps only the last 5000 lines", async () => {
    mount();
    act(() => {
      for (let i = 0; i < MAX_LINES + 5; i++) last().line(`row-${i}`);
    });
    await waitFor(() => expect(within(term()).getByText(`row-${MAX_LINES + 4}`)).toBeInTheDocument());
    expect(within(term()).queryByText("row-4")).not.toBeInTheDocument();
    expect(within(term()).getByText("row-5")).toBeInTheDocument();
  });

  it("filters client-side without touching the stream", async () => {
    mount();
    act(() => {
      last().line("alpha one");
      last().line("beta two");
    });
    await screen.findByText("beta two");
    await userEvent.type(screen.getByLabelText("Filter"), "ALPHA");
    expect(screen.queryByText("beta two")).not.toBeInTheDocument();
    expect(screen.getByText("alpha one")).toBeInTheDocument();
    expect(MockEventSource.instances).toHaveLength(1);
    await userEvent.type(screen.getByLabelText("Filter"), "zzz");
    expect(screen.getByText("No buffered line matches the filter.")).toBeInTheDocument();
  });

  it("pauses by closing the stream and resumes after the last timestamp", async () => {
    mount();
    act(() => {
      last().emit("open");
      last().line("x-api-1 | 2026-10-19T10:02:11.5Z hello");
    });
    await screen.findByText("hello");
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(MockEventSource.instances[0]!.closed).toBe(true);
    expect(screen.getByText("Paused")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(MockEventSource.instances).toHaveLength(2);
    expect(last().params.get("since")).toBe("2026-10-19T10:02:11.5Z");
    expect(screen.queryByText("hello")).toBeInTheDocument();
  });

  it("resumes with a tail and says so when no timestamp was seen", async () => {
    mount();
    act(() => last().line("no timestamp here"));
    await screen.findByText("no timestamp here");
    await userEvent.click(screen.getByRole("button", { name: "Pause" }));
    act(() => {});
    await userEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(last().params.get("since")).toBe("1h");
    expect(last().params.get("tail")).toBe("1000");
  });

  it("validates Since like the server and restarts only for valid values", async () => {
    mount();
    const input = screen.getByLabelText("Since");
    await userEvent.clear(input);
    await userEvent.type(input, "; rm");
    expect(screen.getByRole("alert")).toHaveTextContent(/Since accepts/);
    await new Promise((resolve) => setTimeout(resolve, 600));
    expect(MockEventSource.instances).toHaveLength(1);
    await userEvent.clear(input);
    await userEvent.type(input, "10m");
    await waitFor(() => expect(last().params.get("since")).toBe("10m"));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("reconnects after the server time limit and after an error end", async () => {
    mount();
    act(() => {
      last().emit("open");
      last().emit("end", { reason: "closed" });
    });
    expect(screen.getByText("Reconnecting")).toBeInTheDocument();
    await waitFor(() => expect(MockEventSource.instances).toHaveLength(2));
    act(() => last().emit("open"));
    await waitFor(() => expect(screen.getByText("Live")).toBeInTheDocument());
  });

  it.each([
    [400, /refused/i, "Not streaming"],
    [429, /Too many live log streams/, "Not streaming"],
    [503, /executor is unavailable/i, "Unavailable"],
  ])("explains a %i answer", async (status, text, badge) => {
    const fn = vi.fn(async () => new Response("{}", { status }));
    vi.stubGlobal("fetch", fn);
    mount();
    act(() => last().emit("error"));
    expect(await screen.findByRole("alert")).toHaveTextContent(text);
    expect(screen.getByText(badge)).toBeInTheDocument();
    expect(last().closed).toBe(true);
    if (status !== 503) {
      expect(screen.getByRole("button", { name: "Retry" })).toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Retry" }));
      expect(MockEventSource.instances).toHaveLength(2);
    }
  });

  it("downloads the buffer as a text file", async () => {
    const urls: Blob[] = [];
    URL.createObjectURL = vi.fn((blob: Blob | MediaSource) => {
      urls.push(blob as Blob);
      return "blob:x";
    });
    URL.revokeObjectURL = vi.fn();
    const click = vi.spyOn(HTMLAnchorElement.prototype, "click").mockImplementation(() => {});
    mount();
    act(() => {
      last().line("one");
      last().line("two");
    });
    await screen.findByText("two");
    await userEvent.click(screen.getByRole("button", { name: "Download" }));
    expect(click).toHaveBeenCalled();
    expect(await urls[0]!.text()).toBe("one\ntwo\n");
    click.mockRestore();
  });
});
