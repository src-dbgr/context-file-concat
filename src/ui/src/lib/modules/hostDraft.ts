/**
 * Keeps what the user is typing apart from what the host reports.
 *
 * Every snapshot of the host replaces the app state. A field that is bound to
 * the state directly is therefore reset by any snapshot that arrives while the
 * user is typing: a scan progress update, the answer to another command, or a
 * snapshot that was produced before the host has seen the latest input. A
 * field with a draft shows a local value instead, and asks its `HostDraft`
 * for every host value whether it may be shown:
 *
 * - While the user is editing (changed, not sent yet), the local value stays.
 * - After the value has been sent, host values that differ from it are taken
 *   for snapshots from before the host has processed it and are skipped,
 *   until the host reports the sent value (the acknowledgement).
 * - The wait for the acknowledgement is limited, for the case that the host
 *   has decided differently (another command changed the value in the
 *   meantime). Such a decision always comes with a snapshot, and the host is
 *   the source of truth, so its value is shown again. What ends the wait is
 *   therefore a host value that is newer than the send, never the time alone:
 *   - If differing host values have arrived (and were skipped) by the time
 *     the wait runs out, the last of them is taken for the host's decision.
 *     It may have been the last snapshot, so the wait ends by a timer and
 *     `onAckTimeout` tells the field to look at the host's current value
 *     again.
 *   - If the host has reported nothing since the send (it is still busy with
 *     the command, e.g. a long content search on a host that answers only at
 *     its end), the value the field knows is older than the send. The local
 *     value stays, and the next host value that arrives is shown.
 *
 * `accepts()` is to be called for every value the host reports, also when it
 * equals the one before: that a report has arrived is what the draft goes by.
 *
 * The class holds no UI state; it only answers the question and reports the
 * end of the wait.
 */

/** How long a sent value is protected from differing host values. */
export const ACK_TIMEOUT_MS = 1500;

export type HostDraftOptions<T> = {
  /** When a host value counts as the acknowledgement of a sent value. */
  equals?: (a: T, b: T) => boolean;
  /**
   * The wait for an acknowledgement has run out and the host has reported a
   * different value since the send. The field is to ask `accepts()` again
   * with the latest host value.
   */
  onAckTimeout?: () => void;
  ackTimeoutMs?: number;
};

export class HostDraft<T> {
  private editing = false;
  private awaiting: {
    value: T;
    /** Runs while differing host values are skipped; `null` once expired. */
    timer: ReturnType<typeof setTimeout> | null;
    /** A differing host value has arrived since the send. */
    reported: boolean;
  } | null = null;

  private readonly equals: (a: T, b: T) => boolean;
  private readonly onAckTimeout: (() => void) | undefined;
  private readonly ackTimeoutMs: number;

  constructor(options: HostDraftOptions<T> = {}) {
    this.equals = options.equals ?? Object.is;
    this.onAckTimeout = options.onAckTimeout;
    this.ackTimeoutMs = options.ackTimeoutMs ?? ACK_TIMEOUT_MS;
  }

  /** The user has changed the field; the change has not been sent yet. */
  edit(): void {
    this.editing = true;
  }

  /** `value` has been sent to the host. */
  sent(value: T): void {
    this.editing = false;
    this.stopWaiting();
    const awaiting = {
      value,
      reported: false,
      timer: setTimeout(() => this.expire(awaiting), this.ackTimeoutMs),
    };
    this.awaiting = awaiting;
  }

  /**
   * Gives up the local value: the next host value is shown. It also ends a
   * running wait, which makes it the cleanup when the field goes away.
   */
  reset(): void {
    this.editing = false;
    this.stopWaiting();
  }

  /** Whether the user has changed the field without the change being sent. */
  get isEditing(): boolean {
    return this.editing;
  }

  /**
   * A host value has arrived. Returns whether the field is to show it; if not,
   * the field keeps its local value.
   */
  accepts(host: T): boolean {
    if (this.awaiting) {
      // Expired without a report: this one is newer than the send.
      const expired = this.awaiting.timer === null;
      if (!expired && !this.equals(host, this.awaiting.value)) {
        this.awaiting.reported = true;
        return false;
      }
      this.stopWaiting();
    }
    return !this.editing;
  }

  private expire(awaiting: NonNullable<typeof this.awaiting>): void {
    if (this.awaiting !== awaiting) return;
    if (awaiting.reported) {
      this.awaiting = null;
      this.onAckTimeout?.();
    } else {
      awaiting.timer = null;
    }
  }

  private stopWaiting(): void {
    if (this.awaiting?.timer) clearTimeout(this.awaiting.timer);
    this.awaiting = null;
  }
}
