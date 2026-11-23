// A stand-in for the part of the host the file tree talks to: it owns the
// list of all rows and answers `setViewport` and `locateRow` with snapshots
// that hold a window of it (as `src/app/view_model/rows.rs` does).
//
// It answers when the test says so (`answer()`), so a test decides what the UI
// has seen at any moment: a request that is under way, a snapshot that is
// older than a request, an answer that arrives late.

import { appState } from "$lib/stores/app";
import type { AppState, TreeRow } from "$lib/types";
import { baseAppState, treeFields } from "./tree";

type LocateTarget =
  | { kind: "path"; path: string }
  | { kind: "parent_of"; path: string }
  | { kind: "typeahead"; prefix: string; from: number };

export class FakeTreeHost {
  /** Everything the UI has posted, in order. */
  readonly received: [command: string, payload: unknown][] = [];

  private viewport = { start: 0, count: 100 };
  private revision = 1;
  /** The path of the last lookup; every snapshot states where it is. */
  private located: { request: number; path: string | null } | null = null;

  constructor(
    private rows: TreeRow[],
    private overrides: Partial<AppState> = {},
  ) {}

  /** The implementation of `post()`: takes a command, answers nothing yet. */
  receive = (command: string, payload?: unknown): void => {
    this.received.push([command, payload ?? null]);
    if (command === "setViewport") {
      this.viewport = payload as { start: number; count: number };
    } else if (command === "locateRow") {
      const { request, target } = payload as {
        request: number;
        target: LocateTarget;
      };
      this.located = { request, path: this.pathOf(target) };
    }
  };

  /** Sends the snapshot of the current state to the UI. */
  answer(): AppState {
    const located = this.located && {
      request: this.located.request,
      row: this.rowOfPath(this.located.path),
    };
    const snapshot = baseAppState({
      ...treeFields(this.rows, { ...this.viewport, revision: this.revision }),
      located,
      ...this.overrides,
    });
    appState.set(snapshot);
    return snapshot;
  }

  /** The list changes (a directory was expanded, a filter applied, ...). */
  setRows(rows: TreeRow[], overrides: Partial<AppState> = this.overrides) {
    this.rows = rows;
    this.overrides = overrides;
    this.revision += 1;
  }

  /** The payloads of the given command, in the order they were posted. */
  posted(command: string): unknown[] {
    return this.received
      .filter(([name]) => name === command)
      .map(([, payload]) => payload);
  }

  private rowOfPath(path: string | null): number | null {
    const row = this.rows.findIndex((candidate) => candidate.path === path);
    return row === -1 ? null : row;
  }

  private pathOf(target: LocateTarget): string | null {
    if (target.kind === "path") return target.path;
    if (target.kind === "parent_of") {
      const child = this.rowOfPath(target.path);
      if (child === null) return null;
      for (let row = child - 1; row >= 0; row--) {
        if (this.rows[row].depth < this.rows[child].depth) {
          return this.rows[row].path;
        }
      }
      return null;
    }
    const prefix = target.prefix.toLowerCase();
    const order = [
      ...this.rows.slice(target.from),
      ...this.rows.slice(0, target.from),
    ];
    return (
      order.find((row) => row.name.toLowerCase().startsWith(prefix))?.path ??
      null
    );
  }
}
