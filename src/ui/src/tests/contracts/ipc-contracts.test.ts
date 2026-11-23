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
  makeWireConfig,
  makeWireUiState,
  type PayloadForWire,
} from "./helpers";
import { makeCollapsedDirNode } from "./factories";
import type { TreeNode } from "$lib/types";

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
      ["toggleExpansion", "/repo/src"],
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

  it("accepts boolean payload for expandCollapseAll", () => {
    const pTrue = validateCommand("expandCollapseAll", true);
    const pFalse = validateCommand("expandCollapseAll", false);
    expect(pTrue.command).toBe("expandCollapseAll");
    expect(pFalse.command).toBe("expandCollapseAll");
  });

  it("accepts full wire config for updateConfig", () => {
    const cfg = makeWireConfig({
      output_filename: "cfc_output.txt",
      ignore_patterns: ["node_modules/", "*.log"],
    });
    const parsed = validateCommand("updateConfig", cfg);
    expect(parsed.command).toBe("updateConfig");
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

  it("rejects incomplete config for updateConfig", () => {
    // Deliberately pass a *UI* style partial config to show that the wire schema is stricter.
    const invalid = AnyCommandMessageSchema.safeParse({
      command: "updateConfig",
      payload: {
        ignore_patterns: [],
        output_filename: "x.txt",
        case_sensitive_search: false,
        include_tree_by_default: false,
        use_relative_paths: false,
        remove_empty_directories: false,
        output_directory: "",
      },
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

  it("requires the tree totals the pruned tree cannot provide", () => {
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

  it("accepts a collapsed directory without children (pruned snapshot)", () => {
    const collapsed = makeCollapsedDirNode("/repo/docs", {
      selection_state: "partial",
    });
    const parsed = UiStateSchema.parse({
      ...makeWireUiState(),
      tree: [collapsed],
    });
    expect(parsed.tree[0]).toMatchObject({
      is_expanded: false,
      children: [],
      has_children: true,
      children_loaded: true,
      selection_state: "partial",
    });
  });

  it("rejects a tree node without the pruning flags", () => {
    for (const flag of ["has_children", "children_loaded"] as const) {
      const node: Partial<TreeNode> = makeCollapsedDirNode();
      delete node[flag];
      const result = UiStateSchema.safeParse({
        ...makeWireUiState(),
        tree: [node],
      });
      expect(result.success, flag).toBe(false);
    }
  });

  it("rejects a nested tree node without the pruning flags", () => {
    const child: Partial<TreeNode> = makeCollapsedDirNode("/repo/src/app");
    delete child.children_loaded;
    const parent = {
      ...makeCollapsedDirNode("/repo/src"),
      is_expanded: true,
      children: [child],
    };
    const result = UiStateSchema.safeParse({
      ...makeWireUiState(),
      tree: [parent],
    });
    expect(result.success).toBe(false);
  });

  it("rejects non-boolean pruning flags on a tree node", () => {
    for (const flag of ["has_children", "children_loaded"] as const) {
      const node = { ...makeCollapsedDirNode(), [flag]: "yes" };
      const result = UiStateSchema.safeParse({
        ...makeWireUiState(),
        tree: [node],
      });
      expect(result.success).toBe(false);
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
