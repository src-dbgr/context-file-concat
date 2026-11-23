import { describe, it, expect, vi, afterEach } from "vitest";
import { z } from "zod";

/**
 * The app runs under a CSP without `unsafe-eval`. Zod would probe with
 * `new Function` whether it may compile parsers; the schema module switches
 * that off, so nothing is evaluated (and no CSP violation is reported).
 */
describe("ipc/schema under a CSP without unsafe-eval", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("never calls the Function constructor", async () => {
    vi.resetModules();
    const FunctionSpy = vi.fn(() => {
      throw new EvalError("blocked by CSP");
    });
    vi.stubGlobal("Function", FunctionSpy);

    const schema = await import("$lib/ipc/schema");
    const fixture = (await import("../contracts/fixtures/ui_state_pruned.json"))
      .default;

    expect(z.config().jitless).toBe(true);
    expect(schema.UiStateSchema.safeParse(fixture).success).toBe(true);
    expect(schema.UiStateEnvelopeSchema.safeParse(fixture).success).toBe(true);
    expect(schema.UiStateSchema.safeParse({ tree: 1 }).success).toBe(false);
    expect(
      schema.ScanProgressSchema.safeParse({
        files_scanned: 1,
        large_files_skipped: 0,
        current_scanning_path: "/a",
      }).success,
    ).toBe(true);
    expect(FunctionSpy).not.toHaveBeenCalled();
  });
});
