// Encapsulates communication with the Rust backend.
// Now with compile-time typed commands and dev-only runtime validation.
import {
  AnyCommandMessageSchema,
  type CommandId,
  type CommandName,
  type CommandSeq,
  type NullaryCommandName,
  type NonNullCommandName,
  type PayloadFor,
  type SessionId,
} from "$lib/ipc/schema";

/**
 * A number for this load of the UI: random, a positive safe integer. A UI that
 * is loaded anew (a reload) draws another one, so the host's snapshots can say
 * whose commands they account for.
 */
function drawSession(): SessionId {
  const [high, low] = crypto.getRandomValues(new Uint32Array(2));
  // 53 bits: what a number holds exactly. 0 stands for "no UI yet".
  return (high & 0x1fffff) * 2 ** 32 + low || 1;
}

/** Who posts: this load of the UI. */
const session: SessionId = drawSession();

/**
 * The number of the last command that was posted: the UI numbers them from 1
 * on, in the order it posts them. This is the only place that counts.
 */
let lastSeq: CommandSeq = 0;

/**
 * Send an IPC command to the backend.
 *
 * Overloads enforce correct payloads at compile-time:
 * - Nullary commands: `post("selectAll")`
 * - Commands with payload: `post("loadFilePreview", "/path/to/file")`
 *
 * The message is the command in an envelope with its name:
 * `{"session": s, "seq": n, "command": name, "payload": payload | null}`.
 * The name is a matter of the transport, not of any payload. It is what `post`
 * returns: a command has no result, its effect arrives with a snapshot, and a
 * snapshot that names this session and a `seq` that has reached the command's
 * accounts for the command. Who does not wait for anything ignores it.
 *
 * `null` means that nothing was posted: the command was not valid (checked in
 * development builds only). It uses up no number, and there will be no answer
 * to wait for.
 */
export function post(command: NullaryCommandName): CommandId | null;
export function post<T extends NonNullCommandName>(
  command: T,
  payload: PayloadFor<T>,
): CommandId | null;
export function post(
  command: CommandName,
  payload?: unknown,
): CommandId | null {
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
      return null;
    }
    message = parsed.data as typeof message;
  }

  // Counted only once it is certain that the command is posted: the host has
  // to see every number, or a snapshot could never account for it.
  const id: CommandId = { session, seq: lastSeq + 1 };
  window.ipc.postMessage(JSON.stringify({ ...id, ...message }));
  lastSeq = id.seq;
  return id;
}
