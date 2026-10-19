import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, beforeEach, vi } from "vitest";

afterEach(() => cleanup());

// jsdom does not implement scrolling; the router calls it on navigation.
window.scrollTo = () => {};

// jsdom has no EventSource: pages that stream get one that never connects.
// Tests that exercise streaming replace it with `vi.stubGlobal`.
class InertEventSource {
  addEventListener() {}
  close() {}
}
// Re-applied before each test because suites call `vi.unstubAllGlobals()`.
beforeEach(() => vi.stubGlobal("EventSource", InertEventSource));
