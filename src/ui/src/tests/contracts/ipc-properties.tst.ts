import { describe, it, expect } from "vitest";
import fc from "fast-check";
import { validateCommand } from "./helpers";
import { AnyCommandMessageSchema } from "$lib/ipc/schema";

describe("Property: expandCollapseAll accepts any boolean", () => {
  it("parses for all booleans", () => {
    fc.assert(
      fc.property(fc.boolean(), (b) => {
        const msg = validateCommand("expandCollapseAll", b);
        expect(msg.command).toBe("expandCollapseAll");
      })
    );
  });
});

describe("Property: path-based commands accept non-empty strings", () => {
  const pathArb = fc.stringMatching(/^(\/|[A-Za-z]:\\).+/); // simple POSIX/Windows Start
  const cmds = [
    "loadDirectoryLevel",
    "loadFilePreview",
    "toggleSelection",
    "toggleDirectorySelection",
    "toggleExpansion",
    "addIgnorePath",
  ] as const;

  for (const cmd of cmds) {
    it(`${cmd} parses for plausible paths`, () => {
      fc.assert(
        fc.property(pathArb, (p) => {
          const msg = validateCommand(cmd, p);
          expect(msg.command).toBe(cmd);
        })
      );
    });
  }
});

describe("Property: expandCollapseAll rejects non-boolean", () => {
  it("fails for anything that's not boolean", () => {
    const nonBoolean = fc.oneof(
      fc.string(),
      fc.integer(),
      fc.double(),
      fc.array(fc.anything()),
      fc.object()
    );

    fc.assert(
      fc.property(nonBoolean, (x) => {
        const res = AnyCommandMessageSchema.safeParse({
          command: "expandCollapseAll",
          payload: x,
        });
        expect(res.success).toBe(false);
      })
    );
  });
});
