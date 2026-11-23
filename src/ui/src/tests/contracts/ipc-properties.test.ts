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
      }),
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
    "addIgnorePath",
  ] as const;

  for (const cmd of cmds) {
    it(`${cmd} parses for plausible paths`, () => {
      fc.assert(
        fc.property(pathArb, (p) => {
          const msg = validateCommand(cmd, p);
          expect(msg.command).toBe(cmd);
        }),
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
      fc.object(),
    );

    fc.assert(
      fc.property(nonBoolean, (x) => {
        const res = AnyCommandMessageSchema.safeParse({
          command: "expandCollapseAll",
          payload: x,
        });
        expect(res.success).toBe(false);
      }),
    );
  });
});

describe("Property: path-based commands reject the empty path", () => {
  it("accepts every non-empty text and nothing else", () => {
    fc.assert(
      fc.property(fc.string(), (p) => {
        const res = AnyCommandMessageSchema.safeParse({
          command: "loadDirectoryLevel",
          payload: p,
        });
        expect(res.success).toBe(p.length > 0);
      }),
    );
  });
});

describe("Property: setExpansion names the target state", () => {
  it("parses for any non-empty path and any boolean", () => {
    fc.assert(
      fc.property(
        fc.string({ minLength: 1 }),
        fc.boolean(),
        (path, expanded) => {
          const msg = validateCommand("setExpansion", { path, expanded });
          expect(msg.payload).toEqual({ path, expanded });
        },
      ),
    );
  });

  it("rejects any additional field", () => {
    const otherKey = fc
      .string({ minLength: 1 })
      .filter((key) => !["path", "expanded", "__proto__"].includes(key));

    fc.assert(
      fc.property(otherKey, fc.anything(), (key, value) => {
        const res = AnyCommandMessageSchema.safeParse({
          command: "setExpansion",
          payload: { path: "/repo/src", expanded: true, [key]: value },
        });
        expect(res.success).toBe(false);
      }),
    );
  });
});

describe("Property: ignore pattern intents", () => {
  const blank = fc
    .array(fc.constantFrom(" ", "\t", "\n", " "), { maxLength: 8 })
    .map((chars) => chars.join(""));

  it("addIgnorePattern accepts exactly the texts that are not blank", () => {
    fc.assert(
      fc.property(fc.oneof(fc.string(), blank), (pattern) => {
        const res = AnyCommandMessageSchema.safeParse({
          command: "addIgnorePattern",
          payload: { pattern },
        });
        expect(res.success).toBe(pattern.trim().length > 0);
      }),
    );
  });

  it("removeIgnorePattern accepts every text, blank ones included", () => {
    fc.assert(
      fc.property(fc.oneof(fc.string(), blank), (pattern) => {
        const msg = validateCommand("removeIgnorePattern", { pattern });
        expect(msg.payload).toEqual({ pattern });
      }),
    );
  });
});
