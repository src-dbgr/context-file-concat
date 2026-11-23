// A model of the host's row windowing, for the places that have no host: the
// E2E bridge (Playwright) and the fake host of the component tests. Not part
// of production builds (nothing there imports it).
//
// It is the one TypeScript implementation of what
// `src/app/view_model/rows.rs` and the commands `setViewport`, `locateRow`
// and `initialize` do: it keeps the list of all rows, the window the UI asked
// for and the last row lookup, and states them in snapshots. It reads the
// messages of the UI the way the host does (`src/app/ipc.rs`), and like the
// host it states in every snapshot the number of the last command it has
// processed (`last_processed_seq`, `src/app/correlation.rs`). That it behaves
// like the host is not a matter of care: the host records a scenario
// (`src/app/window_contract_tests.rs` →
// `tests/contracts/fixtures/tree_window_scenario.json`) and
// `tests/contracts/tree-window-scenario.test.ts` replays it against this
// model, step by step.

import {
  AnyCommandMessageSchema,
  type CommandName,
  type CommandSeq,
  type PayloadFor,
} from "$lib/ipc/schema";
import {
  DEFAULT_WINDOW_ROWS,
  MAX_WINDOW_ROWS,
  type RowWindow,
} from "$lib/modules/treeWindow";
import type { AppState, TreeRow } from "$lib/types";

/**
 * A state without the fields the model fills in: those that describe the
 * window of rows, and the number of the last command.
 */
export type TreeLessState = Omit<
  AppState,
  | "rows"
  | "rows_start"
  | "rows_total"
  | "rows_revision"
  | "located"
  | "last_processed_seq"
>;

/** A message of the UI, as far as it could be read (`IpcMessage` on the host). */
type ReadMessage =
  /** A numbered command. */
  | {
      kind: "command";
      seq: CommandSeq;
      command: {
        [K in CommandName]: { command: K; payload: PayloadFor<K> };
      }[CommandName];
    }
  /** A numbered message that is no command. */
  | { kind: "refused"; seq: CommandSeq }
  /** Not a message at all: it has no number. */
  | { kind: "unreadable" };

/**
 * Reads a message the way the host does (`IpcMessage::parse`): the number is
 * taken off the envelope, and what is left has to be a command and nothing
 * else.
 */
function readMessage(message: string): ReadMessage {
  let posted: unknown;
  try {
    posted = JSON.parse(message);
  } catch {
    return { kind: "unreadable" };
  }
  if (typeof posted !== "object" || posted === null || Array.isArray(posted)) {
    return { kind: "unreadable" };
  }
  const { seq, ...rest } = posted as Record<string, unknown>;
  if (typeof seq !== "number" || !Number.isSafeInteger(seq) || seq < 0) {
    return { kind: "unreadable" };
  }
  // A command without a payload may be posted without one.
  const parsed = AnyCommandMessageSchema.safeParse({ payload: null, ...rest });
  if (!parsed.success) return { kind: "refused", seq };
  return {
    kind: "command",
    seq,
    command: parsed.data as Extract<
      ReadMessage,
      { kind: "command" }
    >["command"],
  };
}

/** The directory `path` lies in, as the host's `Path::parent` sees it. */
function parentOf(path: string): string | null {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (cut < 0) return null;
  return cut === 0 ? trimmed.slice(0, 1) : trimmed.slice(0, cut);
}

/** What decides, besides the rows themselves, that another tree is shown. */
function viewKey(state: TreeLessState): string {
  return JSON.stringify([
    state.current_path,
    state.search_query,
    state.extension_filter,
    state.content_search_query,
  ]);
}

function sameList(a: TreeRow[], b: TreeRow[]): boolean {
  return (
    a.length === b.length &&
    a.every((row, i) => row.path === b[i].path && row.depth === b[i].depth)
  );
}

export class TreeHostModel {
  private base: TreeLessState | null = null;
  private rows: TreeRow[] = [];
  private viewport: RowWindow = { start: 0, count: DEFAULT_WINDOW_ROWS };
  private revision = 0;
  /** The path of the last lookup: every snapshot states where it is. */
  private located: { request: number; path: string | null } | null = null;
  /** The number of the last numbered message that was received. */
  private processedSeq: CommandSeq = 0;

  /**
   * The state of the host has changed by something this model does not do
   * itself (a scan, a filter, a directory expanded, a file selected): this is
   * what it shows now, and `rows` is the list of all rows.
   *
   * Like the host, the model shows another directory or other filters from
   * the top and forgets the last lookup, and it counts a list as another one
   * (`rows_revision`) if its rows or what was filtered has changed, not if
   * only the look of the rows has (selection, preview).
   */
  show(state: TreeLessState, rows: TreeRow[]): void {
    const viewChanged =
      this.base !== null && viewKey(this.base) !== viewKey(state);
    if (viewChanged) this.showFromTheTop();
    if (this.base === null || viewChanged || !sameList(this.rows, rows)) {
      this.revision += 1;
    }
    this.base = state;
    this.rows = rows;
  }

  /**
   * Takes a message of the UI, as it was posted. Returns `true` if it is one
   * the model answers with a snapshot:
   *
   * - `setViewport`, `locateRow` and `initialize`, which it carries out;
   * - a message that is refused (not a message, an unknown command, a payload
   *   that does not fit): like the host, the model then changes nothing and
   *   answers all the same.
   *
   * Every other command is one the host answers too, but with a state this
   * model does not work out: whoever uses the model says what is shown then
   * (`show`). A numbered message is processed by being received, whichever of
   * these it is, and the snapshots say so from then on.
   */
  receive(message: string): boolean {
    const read = readMessage(message);
    if (read.kind === "unreadable") return true;
    this.processedSeq = read.seq;
    if (read.kind === "refused") return true;

    const { command } = read;
    if (command.command === "setViewport") {
      this.viewport = {
        start: command.payload.start,
        count: Math.min(command.payload.count, MAX_WINDOW_ROWS),
      };
      return true;
    }
    if (command.command === "locateRow") {
      const { request, target } = command.payload;
      const path =
        target.kind === "path"
          ? target.path
          : target.kind === "parent_of"
            ? parentOf(target.path)
            : this.typeahead(target.prefix, target.from);
      this.located = { request, path };
      return true;
    }
    if (command.command === "initialize") {
      this.showFromTheTop();
      return true;
    }
    return false;
  }

  /** The number of the last command the model has processed. */
  get lastProcessedSeq(): CommandSeq {
    return this.processedSeq;
  }

  /** The snapshot of the current state; `null` before anything is shown. */
  snapshot(): AppState | null {
    if (!this.base) return null;
    const count = Math.min(this.viewport.count, this.rows.length);
    const start = Math.min(this.viewport.start, this.rows.length - count);
    return {
      ...this.base,
      rows: this.rows.slice(start, start + count),
      rows_start: start,
      rows_total: this.rows.length,
      rows_revision: this.revision,
      located: this.located && {
        request: this.located.request,
        row: this.rowOf(this.located.path),
      },
      last_processed_seq: this.processedSeq,
    };
  }

  private showFromTheTop(): void {
    this.viewport = { start: 0, count: this.viewport.count };
    this.located = null;
  }

  private rowOf(path: string | null): number | null {
    if (path === null) return null;
    const row = this.rows.findIndex((candidate) => candidate.path === path);
    return row === -1 ? null : row;
  }

  /** The path of the first row from `from` on, wrapping around, by its name. */
  private typeahead(prefix: string, from: number): string | null {
    const wanted = prefix.toLowerCase();
    const start = Math.min(from, this.rows.length);
    const inOrder = [...this.rows.slice(start), ...this.rows.slice(0, start)];
    const match = inOrder.find((row) =>
      row.name.toLowerCase().startsWith(wanted),
    );
    return match?.path ?? null;
  }
}
