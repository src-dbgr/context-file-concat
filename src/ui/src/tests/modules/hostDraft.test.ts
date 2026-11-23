import { describe, it, expect, afterEach, vi } from "vitest";
import { HostDraft } from "$lib/modules/hostDraft";

// `accepts(n)` stands for a snapshot that accounts for the commands up to n;
// `sent(n)` for the local value having been posted with command n.

describe("HostDraft", () => {
  afterEach(() => vi.useRealTimers());

  it("shows host values while the user is not editing", () => {
    const draft = new HostDraft();
    expect(draft.accepts(0)).toBe(true);
    expect(draft.accepts(3)).toBe(true);
    expect(draft.isEditing).toBe(false);
  });

  it("keeps the local value while the user is editing", () => {
    const draft = new HostDraft();
    draft.edit();
    expect(draft.isEditing).toBe(true);
    // Whatever the host has processed: the edit has not been sent.
    expect(draft.accepts(0)).toBe(false);
    expect(draft.accepts(1_000)).toBe(false);
  });

  it("keeps the sent value until the host has processed its command", () => {
    const draft = new HostDraft();
    draft.edit();
    draft.sent(5);

    expect(draft.accepts(3)).toBe(false); // from before the command
    expect(draft.accepts(4)).toBe(false);
    expect(draft.accepts(5)).toBe(true); // the answer
    expect(draft.accepts(5)).toBe(true);
    expect(draft.accepts(9)).toBe(true);
  });

  it("takes a snapshot that accounts for a later command for the answer too", () => {
    // The answer itself was coalesced away, or another command was faster to
    // be rendered: what counts is that the command has been processed.
    const draft = new HostDraft();
    draft.sent(5);
    expect(draft.accepts(8)).toBe(true);
  });

  it("lets the host's value win as soon as the command is processed", () => {
    // `accepts` does not look at values at all: the host may have refused the
    // command or decided differently, and that is then what is shown.
    const draft = new HostDraft();
    draft.sent(2);
    expect(draft.accepts(1)).toBe(false);
    expect(draft.accepts(2)).toBe(true);
  });

  it("keeps the newer text when an older send is answered", () => {
    const draft = new HostDraft();
    draft.edit();
    draft.sent(1);
    draft.edit(); // typing on

    expect(draft.accepts(1)).toBe(false);
    draft.sent(2);
    expect(draft.accepts(1)).toBe(false);
    expect(draft.accepts(2)).toBe(true);
  });

  it("waits for the last send only", () => {
    const draft = new HostDraft();
    draft.sent(3);
    draft.sent(7);
    expect(draft.accepts(3)).toBe(false);
    expect(draft.accepts(6)).toBe(false);
    expect(draft.accepts(7)).toBe(true);
  });

  it("does not let time decide anything", () => {
    vi.useFakeTimers();
    const draft = new HostDraft();
    draft.sent(4);

    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(vi.getTimerCount()).toBe(0);
    expect(draft.accepts(3)).toBe(false);
    expect(draft.accepts(4)).toBe(true);
  });

  it("is not unsettled by a snapshot that goes back behind the answer", () => {
    // The wait is over once the answer was seen.
    const draft = new HostDraft();
    draft.sent(4);
    expect(draft.accepts(4)).toBe(true);
    expect(draft.accepts(3)).toBe(true);
  });

  it("follows a host that numbers anew", () => {
    // After a reload the UI counts from 1 again; a snapshot of the host from
    // before carries a high number and is shown, which is what a UI that has
    // just started wants.
    const draft = new HostDraft();
    expect(draft.accepts(500)).toBe(true);
    draft.sent(1);
    expect(draft.accepts(1)).toBe(true);
  });

  describe("reset", () => {
    it("gives up an edit", () => {
      const draft = new HostDraft();
      draft.edit();
      draft.reset();
      expect(draft.isEditing).toBe(false);
      expect(draft.accepts(0)).toBe(true);
    });

    it("gives up the wait for an answer", () => {
      const draft = new HostDraft();
      draft.sent(9);
      draft.reset();
      expect(draft.accepts(0)).toBe(true);
    });
  });
});
