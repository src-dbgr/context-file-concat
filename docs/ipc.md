# IPC Contracts

> The UI and host communicate via a simple, typed, JSON-serializable protocol. Requests are command-based; the host emits events for progress/notifications. All payloads are validated at the UI edge before dispatch.

## Principles

- **Single transport**: `$lib/services/backend.ts` exposes `post(command, ...args)`.
- **Typed schema**: `$lib/ipc/schema.ts` defines TypeScript shapes for commands/events.
- **Runtime validation**: `$lib/ipc/handlers.ts` guards inputs and normalizes outputs.
- **No untyped `any` at boundaries**.
- **Forward/Backward compatible** where reasonable (see Versioning).

## Commands (examples)

> Concrete list lives in `$lib/ipc/schema.ts`. Typical examples below:

- `selectDirectory()` → `{ path: string } | { error }`
- `toggleExpansion(path: string)` → `void` (updates reflected via state/event)
- `selectAll()` / `deselectAll()` → `void`
- `generateOutput(options)` → `{ preview: string, took_ms: number }`
- `saveOutput(path?: string)` → `{ saved_path: string } | { error }`

### Example (UI → Host)

```ts
// $lib/services/backend.ts
export async function post<T = unknown>(
  command: string,
  ...args: unknown[]
): Promise<T> {
  // transport: window.external.invoke / custom bridge / tauri-style etc.
  // In tests we mock this function.
  return await (window as any).__HOST_POST__(command, ...args);
}
````

```ts
// somewhere in a component/store
import { post } from "$lib/services/backend";

async function onSaveClick() {
  const res = await post<{ saved_path: string }>("saveOutput");
  // handle success/failure
}
```

## Events (Host → UI)

* `scanProgress` — `{ current: number, total: number }`
* `statusMessage` — `{ text: string }`
* `generationProgress` — `{ phase: "read"|"concat"|"write", ... }`
* `filePreviewReady` — `{ path, language, snippet }`

Events are consumed in `$lib/ipc/handlers.ts` and converted into store updates.

## Error Handling

* The host maps internal errors to a small, documented set (see `src/core/error.rs`).
  UI assumes the shape:

```ts
type HostError = {
  code: "E_IO" | "E_NOT_FOUND" | "E_PERM" | "E_INVALID" | "E_UNKNOWN";
  message: string;
};
```

UI commands return either `Result<T, HostError>` or throw and are caught at the boundary. **Never** leak raw Rust error strings to the UI.

## Versioning & Compatibility

* Schema version is kept in `$lib/ipc/schema.ts` (e.g. `export const IPC_VERSION = 1;`).
* On boot the UI can assert compatibility via a lightweight `handshake()` command that returns `{ host_version, ipc_version }`.
* When adding a command:

  1. Extend TS types (backwards-compatible keys).
  2. Default optional fields on both sides.
  3. Bump `IPC_VERSION` only on breaking changes.

## Adding a New Command (Checklist)

1. **Design**: add request/response types in `$lib/ipc/schema.ts`.
2. **UI guard**: implement input validation in `$lib/ipc/handlers.ts`.
3. **Host**: implement handler in `src/app/commands.rs` and wire to core.
4. **Events** (if needed): document payloads and emit via `src/app/events.rs`.
5. **Tests**:

   * Unit (mock `post`) for UI behavior.
   * Integration (if the command mutates core state).
6. **Docs**: append to this file with an example and error modes.

## Transport Notes

The exact bridge is abstracted in `backend.ts`. In CI/E2E we install a **deterministic bridge** via `$lib/dev/e2eBridge.ts` (only when allowed by `$lib/dev/e2eShim.ts`) so tests can set app state without real IPC.
