// Encapsulates communication with the Rust backend.
export function post(command: string, payload: unknown = null) {
  window.ipc.postMessage(JSON.stringify({ command, payload }));
}
