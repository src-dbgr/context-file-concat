// Encapsulates communication with the Rust backend.
// Now with compile-time typed commands and dev-only runtime validation.
import {
  AnyCommandMessageSchema,
  type CommandSeq,
  type CommandName,
  type NullaryCommandName,
  type NonNullCommandName,
  type PayloadFor,
} from "$lib/ipc/schema";

/**
 * The number of the last command: the UI numbers the commands it posts, from
 * 1 on, in the order it posts them. This is the only place that counts.
 */
let lastSeq: CommandSeq = 0;

/**
 * Send an IPC command to the backend.
 *
 * Overloads enforce correct payloads at compile-time:
 * - Nullary commands: `post("selectAll")`
 * - Commands with payload: `post("loadFilePreview", "/path/to/file")`
 *
 * The message is the command in an envelope with its number:
 * `{"seq": n, "command": name, "payload": payload | null}`. The number is a
 * matter of the transport, not of any payload. It is what `post` returns: a
 * command has no result, its effect arrives with a snapshot, and a snapshot
 * whose `last_processed_seq` has reached that number accounts for the
 * command. Who does not wait for anything ignores it.
 */
export function post(command: NullaryCommandName): CommandSeq;
export function post<T extends NonNullCommandName>(
  command: T,
  payload: PayloadFor<T>,
): CommandSeq;
export function post(command: CommandName, payload?: unknown): CommandSeq {
  const seq = ++lastSeq;
  let message: { command: CommandName; payload: unknown } = {
    command,
    payload: payload ?? null,
  };

  // Dev: validate {command, payload} against the union (adds defaults where
  // defined). Prod: no Zod cost on the hot path.
  if (import.meta.env.DEV) {
    const parsed = AnyCommandMessageSchema.safeParse(message);
    if (!parsed.success) {
      console.warn(
        "[IPC] Blocked invalid command:",
        message,
        parsed.error.flatten(),
      );
      // The number is used up all the same: the next command the host
      // processes accounts for it, so nobody waits for it forever.
      return seq;
    }
    message = parsed.data as typeof message;
  }

  window.ipc.postMessage(JSON.stringify({ seq, ...message }));
  return seq;
}
