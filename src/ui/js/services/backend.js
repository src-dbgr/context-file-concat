// Encapsulates communication with the Rust backend.
export function post(command, payload = null) {
  // eslint-disable-next-line no-undef
  window.ipc.postMessage(JSON.stringify({ command, payload }));
}
