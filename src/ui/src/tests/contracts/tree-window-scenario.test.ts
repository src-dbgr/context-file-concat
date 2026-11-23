/* @vitest-environment node */

// Cross-language conformance test for the window of rows.
//
// `fixtures/tree_window_scenario.json` is written by the host
// (src/app/window_contract_tests.rs): it was posted a scenario, message by
// message as the WebView posts them, and wrote down what it answered, step by
// step (the number of the last command the answer accounts for included),
// plus the limits of the window. Here the same messages are posted to the
// model of the host
// that the UI's tests use in its place (`$lib/dev/treeHostModel.ts`: the E2E
// bridge and the fake host of the component tests). So what those tests run
// against behaves like the host, and CI fails on one of the two sides when
// that stops being true.
//
// Regenerate after an intended change: `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture`

import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { TreeHostModel, type TreeLessState } from "$lib/dev/treeHostModel";
import { UiStateSchema } from "$lib/ipc/schema";
import { DEFAULT_WINDOW_ROWS, MAX_WINDOW_ROWS } from "$lib/modules/treeWindow";
import type { AppState, TreeRow } from "$lib/types";
import { makeValidUiState } from "./factories";

type Expectation = {
  rows_start: number;
  rows_count: number;
  rows_total: number;
  /** `null`: the contract does not say. */
  rows_revision_changed: boolean | null;
  located: AppState["located"];
  first_path: string | null;
  last_path: string | null;
  last_processed_session: number;
  last_processed_seq: number;
};

type Step = {
  name: string;
  action:
    | {
        /**
         * The message changes what the host shows in a way the model does
         * not work out: it receives the message and is told the result.
         */
        kind: "shows";
        message: string;
        list: string;
        current_path: string;
        search_query: string;
      }
    /** A message the model deals with itself. */
    | { kind: "command"; message: string };
  expect: Expectation;
};

type Scenario = {
  limits: { default_rows: number; max_rows: number };
  /** Lists of rows by name, one row per entry: "<depth> <d|f> <path>". */
  lists: Record<string, string[]>;
  steps: Step[];
};

const scenario = JSON.parse(
  readFileSync(
    fileURLToPath(
      new URL("./fixtures/tree_window_scenario.json", import.meta.url),
    ),
    "utf8",
  ),
) as Scenario;

function rowOf(entry: string): TreeRow {
  const [, depth, kind, path] = /^(\d+) ([df]) (.+)$/.exec(entry) ?? [];
  if (path === undefined) throw new Error(`not a row: ${entry}`);
  return {
    path,
    name: path.split("/").pop() ?? path,
    depth: Number(depth),
    is_directory: kind === "d",
    is_expanded: false,
    is_binary: false,
    is_match: false,
    is_previewed: false,
    selection_state: "none",
    size: 0,
    has_children: false,
    children_loaded: true,
  };
}

function stateFor(current_path: string, search_query: string): TreeLessState {
  const {
    rows: _rows,
    rows_start: _start,
    rows_total: _total,
    rows_revision: _revision,
    located: _located,
    last_processed_session: _session,
    last_processed_seq: _seq,
    ...state
  } = makeValidUiState() as AppState;
  return { ...state, current_path, search_query };
}

describe("[IPC Contracts] the limits of the window", () => {
  it("are the same on both sides", () => {
    expect(MAX_WINDOW_ROWS).toBe(scenario.limits.max_rows);
    expect(DEFAULT_WINDOW_ROWS).toBe(scenario.limits.default_rows);
  });

  it("are what the schema accepts at most", () => {
    const snapshot = (count: number) => {
      const row = makeValidUiState().rows[0];
      return UiStateSchema.safeParse({
        ...makeValidUiState(),
        rows: Array.from({ length: count }, () => row),
        rows_total: count,
      }).success;
    };
    expect(snapshot(scenario.limits.max_rows)).toBe(true);
    expect(snapshot(scenario.limits.max_rows + 1)).toBe(false);
  });
});

describe("[IPC Contracts] the model of the host's windowing", () => {
  it("covers a scenario worth replaying", () => {
    const posted = (kind: Step["action"]["kind"]) =>
      new Set(
        scenario.steps.flatMap((step) => {
          if (step.action.kind !== kind) return [];
          try {
            const message = JSON.parse(step.action.message) as {
              command?: unknown;
            };
            return [String(message.command)];
          } catch {
            return ["(not JSON)"];
          }
        }),
      );
    // What the model carries out or refuses itself ...
    expect(posted("command")).toEqual(
      new Set([
        "setViewport",
        "locateRow",
        "initialize",
        "noSuchCommand",
        "(not JSON)",
      ]),
    );
    // ... and what it only takes the number of.
    expect(posted("shows")).toEqual(
      new Set([
        "initialize",
        "setExpansion",
        "toggleSelection",
        "updateFilters",
      ]),
    );
    expect(scenario.steps.length).toBeGreaterThan(20);
    expect(scenario.lists.expanded.length).toBeGreaterThan(MAX_WINDOW_ROWS);
  });

  it("answers every step of the host's scenario as the host did", () => {
    const lists = Object.fromEntries(
      Object.entries(scenario.lists).map(([name, entries]) => [
        name,
        entries.map(rowOf),
      ]),
    );
    const model = new TreeHostModel();
    let lastRevision: number | null = null;

    for (const step of scenario.steps) {
      // The message is posted to the model exactly as it was to the host.
      const answered = model.receive(step.action.message);
      if (step.action.kind === "shows") {
        // A new array every time, as with snapshots of the host: the model
        // must tell another list from the same list by its rows.
        model.show(
          stateFor(step.action.current_path, step.action.search_query),
          [...lists[step.action.list]],
        );
      } else {
        expect(answered, step.name).toBe(true);
      }

      const snapshot = model.snapshot();
      if (!snapshot) throw new Error(`${step.name}: no snapshot`);
      // What the model sends is a snapshot the UI accepts.
      const parsed = UiStateSchema.safeParse(snapshot);
      expect(parsed.error?.issues ?? [], step.name).toEqual([]);

      const actual: Expectation = {
        rows_start: snapshot.rows_start,
        rows_count: snapshot.rows.length,
        rows_total: snapshot.rows_total,
        rows_revision_changed:
          step.expect.rows_revision_changed === null
            ? null
            : snapshot.rows_revision !== lastRevision,
        located: snapshot.located,
        first_path: snapshot.rows[0]?.path ?? null,
        last_path: snapshot.rows[snapshot.rows.length - 1]?.path ?? null,
        last_processed_session: snapshot.last_processed_session,
        last_processed_seq: snapshot.last_processed_seq,
      };
      expect(actual, step.name).toEqual(step.expect);
      // Where the contract does not say, the next step starts afresh.
      lastRevision =
        step.expect.rows_revision_changed === null
          ? null
          : snapshot.rows_revision;
    }
  });
});
