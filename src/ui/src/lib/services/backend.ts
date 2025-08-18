// Encapsulates communication with the Rust backend.
// Now with runtime validation for outgoing messages.

import { AnyCommandMessageSchema } from "$lib/ipc/schema";

export function post(command: string, payload: unknown = null) {
  const message = { command, payload } as const;

  const parsed = AnyCommandMessageSchema.safeParse(message);
  if (!parsed.success) {
    // Block invalid messages to avoid backend regressions.
    console.warn(
      "[IPC] Blocked invalid command:",
      message,
      parsed.error.flatten()
    );
    return;
  }

  window.ipc.postMessage(JSON.stringify(parsed.data));
}
