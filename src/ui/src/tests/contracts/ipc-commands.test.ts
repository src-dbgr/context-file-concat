import { describe, it, expect } from "vitest";
import { validateCommand } from "./helpers";
import { AnyCommandMessageSchema, ConfigOptionSchema } from "$lib/ipc/schema";

const rejects = (command: string, payload: unknown) =>
  expect(
    AnyCommandMessageSchema.safeParse({ command, payload }).success,
    `${command} ${JSON.stringify(payload)}`,
  ).toBe(false);

describe("[IPC Contracts] minimal happy path", () => {
  it("accepts expandCollapseAll with true", () => {
    const p = validateCommand("expandCollapseAll", true);
    expect(p.command).toBe("expandCollapseAll");
  });

  it("accepts expandCollapseAll with false", () => {
    const p = validateCommand("expandCollapseAll", false);
    expect(p.command).toBe("expandCollapseAll");
  });
});

describe("[IPC Contracts] config intents", () => {
  it("accepts one ignore pattern per command", () => {
    for (const name of ["addIgnorePattern", "removeIgnorePattern"] as const) {
      expect(validateCommand(name, { pattern: "node_modules/" })).toEqual({
        command: name,
        payload: { pattern: "node_modules/" },
      });
    }
    expect(validateCommand("clearIgnorePatterns", null).payload).toBeNull();
  });

  it("rejects pattern payloads the backend would not accept", () => {
    for (const name of ["addIgnorePattern", "removeIgnorePattern"]) {
      for (const payload of [
        "*.log",
        null,
        {},
        { pattern: 7 },
        { pattern: "" },
        { pattern: "   " },
        { pattern: "*.log", extra: true },
        { ignore_patterns: ["*.log"] },
      ]) {
        rejects(name, payload);
      }
    }
  });

  it("accepts every user-editable option with its value type", () => {
    const options = [
      { option: "case_sensitive_search", value: true },
      { option: "remove_empty_directories", value: false },
      { option: "include_tree_by_default", value: true },
      { option: "use_relative_paths", value: false },
      { option: "output_filename", value: "out.txt" },
      { option: "output_directory", value: "/out" },
      { option: "output_directory", value: null },
    ] as const;

    for (const payload of options) {
      expect(validateCommand("setConfigOption", payload).payload).toEqual(
        payload,
      );
    }
    // Every option of the schema is exercised above.
    expect(new Set(options.map((o) => o.option))).toEqual(
      new Set(ConfigOptionSchema.options.map((o) => o.shape.option.value)),
    );
  });

  it("rejects options the UI must not set and wrong value types", () => {
    for (const payload of [
      null,
      {},
      { option: "case_sensitive_search" },
      { option: "case_sensitive_search", value: "true" },
      { option: "case_sensitive_search", value: true, extra: 1 },
      { option: "output_filename", value: null },
      { option: "output_directory", value: 7 },
      // Owned by the host: not settable through IPC.
      { option: "window_size", value: [800, 600] },
      { option: "window_position", value: [0, 0] },
      { option: "last_directory", value: "/elsewhere" },
      { option: "max_file_size_mb", value: 1 },
      { option: "tree_ignore_patterns", value: [] },
      { option: "ignore_patterns", value: [] },
    ]) {
      rejects("setConfigOption", payload);
    }
  });

  it("knows no command that replaces the whole config", () => {
    rejects("updateConfig", {
      ignore_patterns: [],
      output_directory: null,
      output_filename: "output.txt",
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
    });
  });
});

describe("[IPC Contracts] negatives", () => {
  it("rejects expandCollapseAll with non-boolean", () => {
    // @ts-expect-error intentionally invalid
    expect(() => validateCommand("expandCollapseAll", "yes")).toThrowError();
  });
});
