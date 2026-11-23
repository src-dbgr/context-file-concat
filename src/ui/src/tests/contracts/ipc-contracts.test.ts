/* @vitest-environment jsdom */

import { describe, it, expect } from "vitest";
import { z } from "zod";
import {
  AnyCommandMessageSchema,
  UiStateSchema,
  type CommandName,
  type NullaryCommandName,
} from "$lib/ipc/schema";
import {
  validateCommand,
  makeWireUiState,
  type PayloadForWire,
} from "./helpers";
import { makeCollapsedDirRow, makeValidRows } from "./factories";
import type { TreeRow } from "$lib/types";

/**
 * Positive cases: commands that accept `null` payloads.
 * We assert these parse successfully and preserve the command literal.
 */
const NULLARY_COMMANDS: readonly NullaryCommandName[] = [
  "selectDirectory",
  "rescanDirectory",
  "generatePreview",
  "clearDirectory",
  "cancelScan",
  "initialize",
  "selectAll",
  "deselectAll",
  "cancelGeneration",
  "clearPreviewState",
  "pickOutputDirectory",
  "exportConfig",
  "importConfig",
  "clearIgnorePatterns",
] as const;

describe("IPC contracts – outgoing commands (positive)", () => {
  it("accepts all nullary commands with `null` payload", () => {
    for (const name of NULLARY_COMMANDS) {
      const parsed = validateCommand(name, null);
      expect(parsed.command).toBe(name);
      expect(parsed.payload).toBeNull();
    }
  });

  it("accepts string-path payload commands", () => {
    const cases: ReadonlyArray<
      readonly [CommandName, PayloadForWire<CommandName>]
    > = [
      ["loadDirectoryLevel", "/repo/src"],
      ["loadFilePreview", "/repo/README.md"],
      ["toggleSelection", "/repo/src/main.rs"],
      ["toggleDirectorySelection", "/repo/src"],
      ["addIgnorePath", "/repo/dist"],
      ["saveFile", "Hello world"], // content to save
    ] as const;

    for (const [name, payload] of cases) {
      const parsed = validateCommand(name, payload);
      expect(parsed.command).toBe(name);
    }
  });

  it("accepts a target state for setExpansion", () => {
    for (const expanded of [true, false]) {
      const parsed = validateCommand("setExpansion", {
        path: "/repo/src",
        expanded,
      });
      expect(parsed).toEqual({
        command: "setExpansion",
        payload: { path: "/repo/src", expanded },
      });
    }
  });

  it("rejects setExpansion payloads the backend would not accept", () => {
    const invalid: unknown[] = [
      "/repo/src",
      null,
      true,
      { path: "/repo/src" },
      { expanded: true },
      { path: "", expanded: true },
      { path: "/repo/src", expanded: "true" },
      { path: "/repo/src", expanded: true, extra: 1 },
    ];
    for (const payload of invalid) {
      const result = AnyCommandMessageSchema.safeParse({
        command: "setExpansion",
        payload,
      });
      expect(result.success, JSON.stringify(payload)).toBe(false);
    }
  });

  it("accepts the window of rows the UI shows", () => {
    expect(validateCommand("setViewport", { start: 120, count: 90 })).toEqual({
      command: "setViewport",
      payload: { start: 120, count: 90 },
    });
  });

  it("rejects setViewport payloads the backend would not accept", () => {
    const invalid: unknown[] = [
      null,
      7,
      {},
      { start: 0 },
      { count: 10 },
      { start: -1, count: 10 },
      { start: 0, count: 0 },
      { start: 0.5, count: 10 },
      { start: 0, count: 10.5 },
      { start: "0", count: 10 },
      { start: 0, count: 10, extra: 1 },
    ];
    for (const payload of invalid) {
      const result = AnyCommandMessageSchema.safeParse({
        command: "setViewport",
        payload,
      });
      expect(result.success, JSON.stringify(payload)).toBe(false);
    }
  });

  it("accepts every kind of row lookup", () => {
    const targets = [
      { kind: "path", path: "/repo/src/main.rs" },
      { kind: "parent_of", path: "/repo/src/main.rs" },
      { kind: "typeahead", prefix: "ma", from: 0 },
    ] as const;
    for (const target of targets) {
      const payload = { request: 4, target };
      expect(validateCommand("locateRow", payload)).toEqual({
        command: "locateRow",
        payload,
      });
    }
  });

  it("rejects locateRow payloads the backend would not accept", () => {
    const path = { kind: "path", path: "/repo/a" };
    const invalid: unknown[] = [
      null,
      path,
      { request: 1 },
      { target: path },
      { request: -1, target: path },
      { request: 1.5, target: path },
      { request: 1, target: path, extra: 1 },
      { request: 1, target: { kind: "path" } },
      { request: 1, target: { kind: "path", path: "" } },
      { request: 1, target: { ...path, extra: 1 } },
      { request: 1, target: { kind: "parent", path: "/repo/a" } },
      { request: 1, target: { kind: "typeahead", prefix: "a" } },
      { request: 1, target: { kind: "typeahead", prefix: "", from: 0 } },
      { request: 1, target: { kind: "typeahead", prefix: "a", from: -1 } },
      { request: 1, target: { kind: "typeahead", prefix: "a", from: 0, x: 1 } },
    ];
    for (const payload of invalid) {
      const result = AnyCommandMessageSchema.safeParse({
        command: "locateRow",
        payload,
      });
      expect(result.success, JSON.stringify(payload)).toBe(false);
    }
  });

  it("accepts boolean payload for expandCollapseAll", () => {
    const pTrue = validateCommand("expandCollapseAll", true);
    const pFalse = validateCommand("expandCollapseAll", false);
    expect(pTrue.command).toBe("expandCollapseAll");
    expect(pFalse.command).toBe("expandCollapseAll");
  });
});

describe("IPC contracts – outgoing commands (negative)", () => {
  it("rejects wrong payload type for path-based commands", () => {
    const invalid = AnyCommandMessageSchema.safeParse({
      command: "loadDirectoryLevel",
      payload: 123, // invalid, must be string
    });
    expect(invalid.success).toBe(false);
  });

  it("rejects wrong payload type for expandCollapseAll", () => {
    const invalid = AnyCommandMessageSchema.safeParse({
      command: "expandCollapseAll",
      payload: "true", // invalid, must be boolean
    });
    expect(invalid.success).toBe(false);
  });
});

describe("IPC contracts – inbound events/state", () => {
  it("accepts a valid UiState payload", () => {
    const ui = makeWireUiState();
    const parsed = UiStateSchema.parse(ui);
    expect(parsed.visible_files_count).toBe(1);
  });

  it("requires the tree totals a window of rows cannot provide", () => {
    for (const counter of ["tree_files_count", "tree_dirs_count"] as const) {
      const missing: Record<string, unknown> = { ...makeWireUiState() };
      delete missing[counter];
      expect(UiStateSchema.safeParse(missing).success).toBe(false);

      const negative = { ...makeWireUiState(), [counter]: -1 };
      expect(UiStateSchema.safeParse(negative).success).toBe(false);
    }
    const parsed = UiStateSchema.parse(makeWireUiState());
    expect(parsed.tree_files_count).toBe(0);
    expect(parsed.tree_dirs_count).toBe(1);
  });

  it("accepts the row of a collapsed directory", () => {
    const collapsed = makeCollapsedDirRow("/repo/docs", {
      selection_state: "partial",
    });
    const parsed = UiStateSchema.parse({
      ...makeWireUiState(),
      rows: [collapsed],
    });
    expect(parsed.rows[0]).toMatchObject({
      depth: 0,
      is_expanded: false,
      has_children: true,
      children_loaded: true,
      selection_state: "partial",
    });
  });

  it("rejects a row without one of its fields", () => {
    const complete = makeCollapsedDirRow();
    for (const field of Object.keys(complete) as (keyof TreeRow)[]) {
      const row: Partial<TreeRow> = { ...complete };
      delete row[field];
      const result = UiStateSchema.safeParse({
        ...makeWireUiState(),
        rows: [row],
      });
      expect(result.success, field).toBe(false);
    }
  });

  it("rejects an invalid row anywhere in the window", () => {
    const rows: unknown[] = makeValidRows();
    rows[2] = { ...makeCollapsedDirRow("/repo/src/app"), depth: -1 };
    const result = UiStateSchema.safeParse({
      ...makeWireUiState(),
      rows,
      rows_total: 3,
    });
    expect(result.success).toBe(false);
  });

  it("rejects wrongly typed fields of a row", () => {
    const wrong: Record<string, unknown> = {
      has_children: "yes",
      children_loaded: "yes",
      depth: 1.5,
      selection_state: "some",
      path: 7,
    };
    for (const [field, value] of Object.entries(wrong)) {
      const row = { ...makeCollapsedDirRow(), [field]: value };
      const result = UiStateSchema.safeParse({
        ...makeWireUiState(),
        rows: [row],
      });
      expect(result.success, field).toBe(false);
    }
  });

  it("requires the description of the window", () => {
    for (const field of [
      "rows",
      "rows_start",
      "rows_total",
      "rows_revision",
      "located",
    ] as const) {
      const missing: Record<string, unknown> = { ...makeWireUiState() };
      delete missing[field];
      expect(UiStateSchema.safeParse(missing).success, field).toBe(false);
    }
    for (const field of ["rows_start", "rows_total", "rows_revision"]) {
      for (const value of [-1, 0.5, "1", null]) {
        const wrong = { ...makeWireUiState(), [field]: value };
        expect(UiStateSchema.safeParse(wrong).success, field).toBe(false);
      }
    }
  });

  it("rejects a window that does not lie inside the list", () => {
    const window = (rows_start: number, rows_total: number) =>
      UiStateSchema.safeParse({
        ...makeWireUiState(),
        rows: makeValidRows(),
        rows_start,
        rows_total,
      }).success;

    expect(window(0, 3)).toBe(true);
    expect(window(7, 10)).toBe(true);
    expect(window(40, 1000)).toBe(true);
    expect(window(0, 2)).toBe(false);
    expect(window(8, 10)).toBe(false);
  });

  it("accepts the answer to a row lookup, found or not", () => {
    for (const located of [
      null,
      { request: 3, row: 17 },
      { request: 3, row: null },
    ]) {
      const parsed = UiStateSchema.parse({ ...makeWireUiState(), located });
      expect(parsed.located).toEqual(located);
    }
    for (const located of [
      { request: 3 },
      { row: 1 },
      { request: -1, row: 1 },
      { request: 1, row: -1 },
      { request: "1", row: 1 },
      7,
    ]) {
      const result = UiStateSchema.safeParse({ ...makeWireUiState(), located });
      expect(result.success, JSON.stringify(located)).toBe(false);
    }
  });

  it("rejects UiState missing required fields", () => {
    const good = makeWireUiState();
    // Remove required field in a type-safe way for this negative test:
    const bad: Partial<z.input<typeof UiStateSchema>> = { ...good };
    delete (bad as { status_message?: string }).status_message;

    const result = UiStateSchema.safeParse(bad);
    expect(result.success).toBe(false);
  });
});
