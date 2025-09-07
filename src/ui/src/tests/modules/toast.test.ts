/* @vitest-environment jsdom */
import { describe, it, expect, vi, beforeEach } from "vitest";
import { toasts, toast, type ToastItem } from "$lib/stores/toast";

function readToasts(): ToastItem[] {
  let list: ToastItem[] = [];
  const unsub = toasts.subscribe((v) => (list = v));
  unsub();
  return list;
}

describe("toast store", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    toast.clear();
  });

  it("auto-dismisses info/success toasts after default duration", () => {
    const id = toast.success("Saved!");
    expect(readToasts().some((t) => t.id === id)).toBe(true);

    vi.advanceTimersByTime(4000);
    expect(readToasts().some((t) => t.id === id)).toBe(false);
  });

  it("manual dismiss removes a toast immediately", () => {
    const id = toast.info("Hello");
    expect(readToasts().length).toBe(1);
    toast.dismiss(id);
    expect(readToasts().length).toBe(0);
  });

  it("pause() then resume() respects remaining time", () => {
    const id = toast.warning("Careful!");
    vi.advanceTimersByTime(2000);
    toast.pause(id);

    vi.advanceTimersByTime(10000);
    expect(readToasts().some((t) => t.id === id)).toBe(true);

    toast.resume(id);
    vi.advanceTimersByTime(4000);
    expect(readToasts().some((t) => t.id === id)).toBe(false);
  });

  it("clear() nukes everything and timers", () => {
    toast.info("A");
    toast.success("B");
    expect(readToasts().length).toBe(2);
    toast.clear();
    expect(readToasts().length).toBe(0);
  });
});
