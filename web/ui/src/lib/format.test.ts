import { describe, expect, it } from "vitest";

import { formatAge, formatDayTime, formatDuration, formatWhen } from "./format";

const NOW = Date.parse("2026-10-19T12:00:00Z");
const unix = (iso: string) => Date.parse(iso) / 1000;

describe("format", () => {
  it("ages use one coarse unit", () => {
    expect([0, 59, 60, 3599, 3600, 86399, 86400, -5].map(formatAge)).toEqual([
      "0 s", "59 s", "1 m", "59 m", "1 h", "23 h", "1 d", "0 s",
    ]);
  });

  it("durations are exact without zero tails", () => {
    expect([38, 120, 252, 3600, 7500].map(formatDuration)).toEqual(["38 s", "2 m", "4 m 12 s", "1 h", "2 h 5 m"]);
  });

  it("relative times", () => {
    expect(formatWhen(unix("2026-10-19T11:59:40Z"), NOW)).toBe("just now");
    expect(formatWhen(unix("2026-10-19T10:00:00Z"), NOW)).toBe("2 h ago");
    expect(formatWhen(unix("2026-10-18T09:00:00Z"), NOW)).toBe("yesterday");
    expect(formatWhen(unix("2026-10-15T12:00:00Z"), NOW)).toBe("4 d ago");
  });

  it("day times (UTC in tests)", () => {
    expect(formatDayTime(unix("2026-10-19T03:00:00Z"), NOW)).toBe("Today 03:00");
    expect(formatDayTime(unix("2026-10-18T23:30:00Z"), NOW)).toBe("Yesterday 23:30");
    expect(formatDayTime(unix("2026-10-10T03:05:00Z"), NOW)).toBe("10 Oct 03:05");
  });
});
