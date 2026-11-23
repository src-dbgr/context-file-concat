/**
 * Keeps what the user is typing apart from what the host reports.
 *
 * Every snapshot of the host replaces the app state. A field that is bound to
 * the state directly is therefore reset by any snapshot that arrives while the
 * user is typing: a scan progress update, the answer to another command, or a
 * snapshot that was produced before the host has seen the latest input. A
 * field with a draft shows a local value instead, and asks its `HostDraft`
 * for every snapshot whether the host's value may be shown:
 *
 * - While the user is editing (changed, not sent yet), the local value stays.
 * - After the value has been sent, it stays until the host has processed the
 *   command that carried it. Which snapshot that is, is not guessed: `post()`
 *   returns the number of the command, and every snapshot states the number
 *   of the last command it accounts for (`last_processed_seq`). A snapshot
 *   that has not reached the number was made before the host got to the
 *   command and says nothing about it.
 * - From the first snapshot that has reached the number on, the host's value
 *   is shown, whatever it is: the host is the source of truth. It is the sent
 *   value unless the host has decided otherwise (it refused the command, or a
 *   later command changed the value).
 *
 * There is no timer and no comparison of values in this: the host answers
 * every command, and its answer says so itself.
 *
 * The class holds no UI state; it only answers the question.
 */

import type { CommandSeq } from "$lib/ipc/schema";

export class HostDraft {
  private editing = false;
  /** The command that carried the local value; `null`: nothing to wait for. */
  private awaiting: CommandSeq | null = null;

  /** The user has changed the field; the change has not been sent yet. */
  edit(): void {
    this.editing = true;
  }

  /**
   * The local value has been sent to the host, with the command `seq` (what
   * `post()` returned for it).
   */
  sent(seq: CommandSeq): void {
    this.editing = false;
    this.awaiting = seq;
  }

  /** Gives up the local value: the next host value is shown. */
  reset(): void {
    this.editing = false;
    this.awaiting = null;
  }

  /** Whether the user has changed the field without the change being sent. */
  get isEditing(): boolean {
    return this.editing;
  }

  /**
   * A snapshot has arrived that accounts for the commands up to
   * `lastProcessedSeq`. Returns whether the field is to show the host's
   * value; if not, the field keeps its local value.
   */
  accepts(lastProcessedSeq: CommandSeq): boolean {
    if (this.awaiting !== null) {
      if (lastProcessedSeq < this.awaiting) return false;
      this.awaiting = null;
    }
    return !this.editing;
  }
}
