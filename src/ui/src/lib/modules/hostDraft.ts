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
 * - The wait for the acknowledgement is limited. A host that keeps reporting
 *   something else has decided differently (another command changed the value
 *   in the meantime, or the directory was closed), and the host is the source
 *   of truth, so its value is shown again.
 *
 * The class holds no UI state and no timers; it only answers the question.
 */

/** How long a sent value is protected from differing host values. */
export const ACK_TIMEOUT_MS = 1500;

export class HostDraft<T> {
  private editing = false;
  private awaiting: { value: T; until: number } | null = null;

  constructor(
    private readonly equals: (a: T, b: T) => boolean = Object.is,
    private readonly now: () => number = () => Date.now(),
    private readonly ackTimeoutMs: number = ACK_TIMEOUT_MS,
  ) {}

  /** The user has changed the field; the change has not been sent yet. */
  edit(): void {
    this.editing = true;
  }

  /** `value` has been sent to the host. */
  sent(value: T): void {
    this.editing = false;
    this.awaiting = { value, until: this.now() + this.ackTimeoutMs };
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
   * A host value has arrived. Returns whether the field is to show it; if not,
   * the field keeps its local value.
   */
  accepts(host: T): boolean {
    if (this.awaiting) {
      const acknowledged = this.equals(host, this.awaiting.value);
      if (acknowledged || this.now() >= this.awaiting.until) {
        this.awaiting = null;
      } else {
        return false;
      }
    }
    return !this.editing;
  }
}
