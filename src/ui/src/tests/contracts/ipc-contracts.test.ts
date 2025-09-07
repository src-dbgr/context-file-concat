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
  "expandAllFully",
  "selectAllFully",
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

  it("rejects UiState missing required fields", () => {
    const good = makeWireUiState();
    // Remove required field in a type-safe way for this negative test:
    const bad: Partial<z.input<typeof UiStateSchema>> = { ...good };
    delete (bad as { status_message?: string }).status_message;

    const result = UiStateSchema.safeParse(bad);
    expect(result.success).toBe(false);
  });
});
