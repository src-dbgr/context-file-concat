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
 * - The wait for the acknowledgement is limited. A host that has not reported
 *   the sent value by then has decided differently (another command changed
 *   the value in the meantime), and the host is the source of truth, so its
 *   value is shown again. The wait ends by a timer, not by the next snapshot:
 *   the snapshot that carried the host's decision may have been the last one,
 *   and it was skipped. `onAckTimeout` tells the field to look at the host's
 *   current value again.
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
   * The wait for an acknowledgement has run out. The field is to ask
   * `accepts()` again with the latest host value.
   */
  onAckTimeout?: () => void;
  ackTimeoutMs?: number;
};

export class HostDraft<T> {
  private editing = false;
  private awaiting: {
    value: T;
    timer: ReturnType<typeof setTimeout>;
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
    const timer = setTimeout(() => {
      this.awaiting = null;
      this.onAckTimeout?.();
    }, this.ackTimeoutMs);
    this.awaiting = { value, timer };
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
      if (!this.equals(host, this.awaiting.value)) return false;
      this.stopWaiting();
    }
    return !this.editing;
  }

  private stopWaiting(): void {
    if (this.awaiting) clearTimeout(this.awaiting.timer);
    this.awaiting = null;
  }
}
