// Encapsulates communication with the Rust backend.
// Now with compile-time typed commands and dev-only runtime validation.
import {
  AnyCommandMessageSchema,
  type CommandName,
  type NullaryCommandName,
  type NonNullCommandName,
  type PayloadFor,
} from "$lib/ipc/schema";

/**
 * Send an IPC command to the backend.
 *
 * Overloads enforce correct payloads at compile-time:
 * - Nullary commands: `post("selectAll")`
 * - Commands with payload: `post("toggleExpansion", "/path/to/dir")`
 */
export function post(command: NullaryCommandName): void;
export function post<T extends NonNullCommandName>(
  command: T,
  payload: PayloadFor<T>
): void;
export function post(command: CommandName, payload?: unknown): void {
  // Dev: validate {command, payload} against the union (adds defaults where defined).
  if (import.meta.env.DEV) {
    const candidate = {
      command,
      payload: payload ?? null,
    } as const;

    const parsed = AnyCommandMessageSchema.safeParse(candidate);
    if (!parsed.success) {
      console.warn(
        "[IPC] Blocked invalid command:",
        candidate,
        parsed.error.flatten()
      );
      return;
    }
    window.ipc.postMessage(JSON.stringify(parsed.data));
    return;
  }

  // Prod: no Zod cost on the hot path.
  window.ipc.postMessage(
    JSON.stringify({
      command,
      payload: payload ?? null,
    })
  );
}
