import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { ACK_TIMEOUT_MS, HostDraft } from "$lib/modules/hostDraft";

describe("HostDraft", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("shows host values while the user is not editing", () => {
    const draft = new HostDraft<string>();
    expect(draft.accepts("a")).toBe(true);
    expect(draft.accepts("b")).toBe(true);
    expect(draft.isEditing).toBe(false);
  });

  it("keeps the local value while the user is editing", () => {
    const onAckTimeout = vi.fn();
    const draft = new HostDraft<string>({ onAckTimeout });
    draft.edit();
    expect(draft.isEditing).toBe(true);
    expect(draft.accepts("host")).toBe(false);
    // Not limited in time: the user may think as long as they like.
    vi.advanceTimersByTime(ACK_TIMEOUT_MS * 10);
    expect(draft.accepts("host")).toBe(false);
    expect(onAckTimeout).not.toHaveBeenCalled();
  });

  it("skips stale host values until the sent value is acknowledged", () => {
    const draft = new HostDraft<string>();
    draft.edit();
    draft.sent("abc");

    expect(draft.accepts("")).toBe(false); // produced before the host saw "abc"
    expect(draft.accepts("ab")).toBe(false);
    expect(draft.accepts("abc")).toBe(true); // the acknowledgement
    expect(draft.accepts("changed by the host")).toBe(true);
  });

  it("keeps the newer text when an older send is acknowledged", () => {
    const draft = new HostDraft<string>();
    draft.edit();
    draft.sent("a");
    draft.edit(); // typing on: "ab"

    expect(draft.accepts("a")).toBe(false);
    draft.sent("ab");
    expect(draft.accepts("a")).toBe(false);
    expect(draft.accepts("ab")).toBe(true);
  });

  it("lets the host win when the acknowledgement does not arrive", () => {
    const draft = new HostDraft<string>();
    draft.sent("abc");

    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    expect(draft.accepts("")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(draft.accepts("")).toBe(true);
    expect(draft.accepts("anything")).toBe(true);
  });

  it("reports the end of the wait by itself, without a host value arriving", () => {
    const onAckTimeout = vi.fn();
    const draft = new HostDraft<string>({ onAckTimeout });
    draft.sent("abc");
    // The host's decision; skipped, and nothing follows it.
    expect(draft.accepts("")).toBe(false);

    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    expect(onAckTimeout).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onAckTimeout).toHaveBeenCalledTimes(1);
    expect(draft.accepts("")).toBe(true);
  });

  it("does not report a wait that ended otherwise", () => {
    const onAckTimeout = vi.fn();
    const draft = new HostDraft<string>({ onAckTimeout });

    draft.sent("acknowledged");
    expect(draft.accepts("acknowledged")).toBe(true);

    draft.sent("given up");
    draft.reset();

    vi.advanceTimersByTime(ACK_TIMEOUT_MS * 2);
    expect(onAckTimeout).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("waits the full time for the value that was sent last", () => {
    const onAckTimeout = vi.fn();
    const draft = new HostDraft<string>({ onAckTimeout });
    draft.sent("a");
    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    draft.sent("ab");

    vi.advanceTimersByTime(ACK_TIMEOUT_MS - 1);
    expect(onAckTimeout).not.toHaveBeenCalled();
    expect(draft.accepts("a")).toBe(false);
    vi.advanceTimersByTime(1);
    expect(onAckTimeout).toHaveBeenCalledTimes(1);
  });

  it("does not let an expired wait override an edit in progress", () => {
    const draft = new HostDraft<string>();
    draft.sent("abc");
    draft.edit();
    vi.advanceTimersByTime(ACK_TIMEOUT_MS);
    expect(draft.accepts("")).toBe(false);
    draft.sent("abcd");
    expect(draft.accepts("abcd")).toBe(true);
  });

  it("reset gives up the local value", () => {
    const draft = new HostDraft<string>();
    draft.edit();
    draft.sent("abc");
    draft.edit();
    draft.reset();
    expect(draft.isEditing).toBe(false);
    expect(draft.accepts("host")).toBe(true);
  });

  it("uses the given equality and timeout", () => {
    const onAckTimeout = vi.fn();
    const draft = new HostDraft<{ q: string }>({
      equals: (a, b) => a.q === b.q,
      onAckTimeout,
      ackTimeoutMs: 10,
    });
    draft.sent({ q: "x" });
    expect(draft.accepts({ q: "y" })).toBe(false);
    expect(draft.accepts({ q: "x" })).toBe(true);

    draft.sent({ q: "z" });
    vi.advanceTimersByTime(10);
    expect(onAckTimeout).toHaveBeenCalledTimes(1);
  });
});
