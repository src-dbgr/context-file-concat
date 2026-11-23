// The host of the component tests of the file tree.
//
// What it does is not written here: it is the shared model of the host's row
// windowing (`$lib/dev/treeHostModel.ts`), the same one the E2E bridge uses,
// which a contract test holds against the real host. This wrapper adds what a
// component test needs: it records what the UI posts, and it answers when the
// test says so (`answer()`), so a test decides what the UI has seen at any
// moment: a request that is under way, a snapshot that is older than a
// request, an answer that arrives late. It numbers the commands like `post()`
// does, and its snapshots state the number of the last one, like the host's.

import { TreeHostModel } from "$lib/dev/treeHostModel";
import { UiStateSchema, type CommandId } from "$lib/ipc/schema";
import { appState } from "$lib/stores/app";
import type { AppState, TreeRow } from "$lib/types";
import { baseAppState } from "./tree";

export class FakeTreeHost {
  /** Everything the UI has posted, in order. */
  readonly received: [command: string, payload: unknown][] = [];

  private readonly model = new TreeHostModel();
  /** The UI of the test, as `post()` would name it. */
  private readonly session = 4711;
  private lastSeq = 0;

  constructor(rows: TreeRow[], overrides: Partial<AppState> = {}) {
    this.setRows(rows, overrides);
  }

  /**
   * The implementation of `post()`: numbers the command, hands the message to
   * the host and returns the number. Nothing is answered yet.
   */
  receive = (command: string, payload?: unknown): CommandId => {
    const id = { session: this.session, seq: ++this.lastSeq };
    this.received.push([command, payload ?? null]);
    this.model.receive(
      JSON.stringify({ ...id, command, payload: payload ?? null }),
    );
    return id;
  };

  /**
   * Sends the snapshot of the current state to the UI. It has to be one the
   * UI would accept from the host.
   */
  answer(): AppState {
    const snapshot = UiStateSchema.parse(this.model.snapshot()) as AppState;
    appState.set(snapshot);
    return snapshot;
  }

  /**
   * What the host shows changes (a directory was expanded, a filter applied,
   * another directory loaded, ...): `rows` is the list of all rows now.
   */
  setRows(rows: TreeRow[], overrides: Partial<AppState> = {}) {
    const {
      rows: _rows,
      rows_start: _start,
      rows_total: _total,
      rows_revision: _revision,
      located: _located,
      last_processed_session: _session,
      last_processed_seq: _seq,
      ...state
    } = baseAppState(overrides);
    this.model.show(state, rows);
  }

  /** The payloads of the given command, in the order they were posted. */
  posted(command: string): unknown[] {
    return this.received
      .filter(([name]) => name === command)
      .map(([, payload]) => payload);
  }
}
