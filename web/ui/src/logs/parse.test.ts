import { describe, expect, it } from "vitest";

import { parseLine, validSince } from "./parse";

describe("parseLine", () => {
  it("extracts the compose service, timestamp and level when present", () => {
    const line = parseLine("notes-app-api-1  | 2026-10-19T10:02:11.250Z ERROR boom", "notes-app");
    expect(line).toMatchObject({
      service: "api",
      time: "10:02:11",
      sinceStamp: "2026-10-19T10:02:11.250Z",
      level: "error",
      message: "ERROR boom",
    });
  });

  it("does not invent anything for plain lines", () => {
    expect(parseLine("just some output", "p")).toEqual({
      message: "just some output",
      service: null,
      time: null,
      sinceStamp: null,
      level: null,
    });
  });

  it("shows a timestamp without a zone but never uses it for since", () => {
    const line = parseLine("2026-10-19 10:02:11 hello", "p");
    expect(line.time).toBe("10:02:11");
    expect(line.sinceStamp).toBeNull();
  });

  it("strips ANSI colours and reads level=...", () => {
    const line = parseLine("\u001b[31mmsg\u001b[0m level=warn x", "p");
    expect(line.message).toBe("msg level=warn x");
    expect(line.level).toBe("warn");
  });

  it("leaves debug and unknown words uncoloured", () => {
    expect(parseLine("DEBUG x", "p").level).toBeNull();
    expect(parseLine("information", "p").level).toBeNull();
  });
});

describe("validSince", () => {
  it("matches the server rule", () => {
    for (const ok of ["", "10m", "1h30m", "2026-10-19T10:00:00Z", "2026-10-19T10:00:00+02:00"]) expect(validSince(ok)).toBe(true);
    for (const bad of ["; rm", "1 h", "x".repeat(5), "1".repeat(41), "10M"]) expect(validSince(bad)).toBe(false);
  });
});
