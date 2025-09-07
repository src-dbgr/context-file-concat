import { describe, it, expect } from "vitest";
import { validateCommand, makeWireConfig } from "./helpers";

describe("[IPC Contracts] minimal happy path", () => {
  it("accepts updateConfig with a full wire payload", () => {
    const cfg = makeWireConfig();
    const parsed = validateCommand("updateConfig", cfg);
    expect(parsed.command).toBe("updateConfig");
    expect(parsed.payload).not.toBeNull();
  });

  it("accepts expandCollapseAll with true", () => {
    const p = validateCommand("expandCollapseAll", true);
    expect(p.command).toBe("expandCollapseAll");
  });

  it("accepts expandCollapseAll with false", () => {
    const p = validateCommand("expandCollapseAll", false);
    expect(p.command).toBe("expandCollapseAll");
  });
});

describe("[IPC Contracts] negatives", () => {
  it("rejects updateConfig with an invalid payload", () => {
    // @ts-expect-error intentionally invalid
    expect(() => validateCommand("updateConfig", {})).toThrowError();
  });

  it("rejects expandCollapseAll with non-boolean", () => {
    // @ts-expect-error intentionally invalid
    expect(() => validateCommand("expandCollapseAll", "yes")).toThrowError();
  });
});
