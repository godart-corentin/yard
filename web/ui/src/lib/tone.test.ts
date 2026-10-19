import { describe, expect, it } from "vitest";

import { hostLabel, hostTone, projectLabel, projectLevel, projectTone, serviceTone, worstHostLevel } from "./tone";

describe("project status semantics", () => {
  it.each([
    ["operational", "operational", "ok", "Operational"],
    ["healthy", "operational", "ok", "Operational"],
    ["degraded", "degraded", "warn", "Degraded"],
    ["down", "down", "bad", "Down"],
    ["unhealthy", "down", "bad", "Down"],
    ["unknown", "unknown", "unk", "Unknown"],
  ])("%s -> %s", (status, level, tone, label) => {
    expect(projectLevel(status)).toBe(level);
    expect(projectTone(status)).toBe(tone);
    expect(projectLabel(status)).toBe(label);
  });

  it("never turns an unrecognised status into Down or Operational", () => {
    expect(projectLevel("")).toBe("unknown");
    expect(projectLevel("weird")).toBe("unknown");
    expect(projectTone("alert")).toBe("unk");
  });

  it("maps service and host levels", () => {
    expect(serviceTone("unhealthy")).toBe("bad");
    expect(serviceTone("unknown")).toBe("unk");
    expect(hostTone("warning")).toBe("warn");
    expect(hostLabel("normal")).toBe("Healthy");
    expect(worstHostLevel([])).toBe("normal");
    expect(worstHostLevel(["normal", "unknown"])).toBe("unknown");
    expect(worstHostLevel(["unknown", "warning", "normal"])).toBe("warning");
    expect(worstHostLevel(["warning", "critical"])).toBe("critical");
  });
});
