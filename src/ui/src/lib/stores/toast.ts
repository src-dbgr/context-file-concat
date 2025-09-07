// Toast store (Svelte 5+ compatible, framework-agnostic module)
// - Queue with max length
// - Timed auto-dismiss with pause/resume on hover
// - Simple helpers: info/success/warning/error
// - Accessible by rendering component with aria-live=polite

import { writable } from "svelte/store";

export type ToastVariant = "info" | "success" | "warning" | "error";

export interface ToastInput {
  message: string;
  title?: string;
  variant?: ToastVariant;
  /**
   * Milliseconds before auto-dismiss. Set to 0 or negative to disable auto-dismiss.
   * Defaults to 3500ms for info/success, 5000ms for warning/error.
   */
  duration?: number;
}

export interface Toast extends Required<ToastInput> {
  id: number;
  createdAt: number;
}

const MAX_TOASTS = 4;

let _id = 1;
const { subscribe, set, update } = writable<Toast[]>([]);

// Internal timers: id -> timer handle
const runningTimers = new Map<number, number>();
// Remaining time when paused: id -> ms
const remaining = new Map<number, number>();
// When a timer started (for computing remaining on pause): id -> epoch ms
const startedAt = new Map<number, number>();

function normalize(input: ToastInput): Toast {
  const v = input.variant ?? "info";
  const defaultDuration = v === "warning" || v === "error" ? 5000 : 3500;
  return {
    id: _id++,
    title: input.title ?? "",
    message: input.message,
    variant: v,
    duration:
      typeof input.duration === "number"
        ? Math.max(0, input.duration)
        : defaultDuration,
    createdAt: Date.now(),
  };
}

function startTimer(t: Toast) {
  if (t.duration <= 0) return;

  // on resume, prefer remaining; else full duration
  const msRemaining = remaining.get(t.id) ?? t.duration;
  startedAt.set(t.id, Date.now());
  const handle = window.setTimeout(() => dismiss(t.id), msRemaining);
  runningTimers.set(t.id, handle);
}

function stopTimer(id: number) {
  const handle = runningTimers.get(id);
  if (handle) {
    clearTimeout(handle);
    runningTimers.delete(id);
  }
}

function push(input: ToastInput): number {
  const t = normalize(input);

  update((list) => {
    const next = [...list, t];
    while (next.length > MAX_TOASTS) {
      const removed = next.shift();
      if (removed) {
        stopTimer(removed.id);
        remaining.delete(removed.id);
        startedAt.delete(removed.id);
      }
    }
    return next;
  });

  startTimer(t);
  return t.id;
}

function dismiss(id: number) {
  stopTimer(id);
  remaining.delete(id);
  startedAt.delete(id);
  update((list) => list.filter((t) => t.id !== id));
}

function clear() {
  // Clear all timers and state
  for (const id of runningTimers.keys()) clearTimeout(runningTimers.get(id)!);
  runningTimers.clear();
  remaining.clear();
  startedAt.clear();
  set([]);
}

function pause(id: number) {
  const start = startedAt.get(id);
  stopTimer(id);
  if (start) {
    const tNow = Date.now();
    const elapsed = tNow - start;
    // fallback: if toast not found or no startedAt, do nothing
    let dur = 0;
    const unsub = subscribe((list) => {
      const t = list.find((x) => x.id === id);
      if (t) dur = t.duration;
    });
    unsub();
    const rem = Math.max(0, (remaining.get(id) ?? dur) - elapsed);
    remaining.set(id, rem);
  }
}

function resume(id: number) {
  // only if we actually paused previously
  const rem = remaining.get(id);
  if (rem === undefined) return;

  // find the toast to ensure it exists
  let toast: Toast | undefined;
  const unsub = subscribe((list) => (toast = list.find((x) => x.id === id)));
  unsub();
  if (!toast) return;

  startedAt.set(id, Date.now());
  const handle = window.setTimeout(() => dismiss(id), rem);
  runningTimers.set(id, handle);
}

function info(
  message: string,
  opts: Partial<Omit<ToastInput, "message">> = {}
) {
  return push({ message, variant: "info", ...opts });
}
function success(
  message: string,
  opts: Partial<Omit<ToastInput, "message">> = {}
) {
  return push({ message, variant: "success", ...opts });
}
function warning(
  message: string,
  opts: Partial<Omit<ToastInput, "message">> = {}
) {
  return push({ message, variant: "warning", ...opts });
}
function error(
  message: string,
  opts: Partial<Omit<ToastInput, "message">> = {}
) {
  return push({ message, variant: "error", ...opts });
}

export const toasts = { subscribe };
export const toast = {
  push,
  dismiss,
  clear,
  pause,
  resume,
  info,
  success,
  warning,
  error,
};
export type { Toast as ToastItem };
