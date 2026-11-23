import { describe, it, expect, afterEach, vi } from "vitest";
import { HostDraft } from "$lib/modules/hostDraft";

// `accepts(host(n))` stands for a snapshot that accounts for the commands of
// this UI up to n; `sent(mine(n))` for the local value having been posted with
// command n.

/** This load of the UI, and the one before it. */
const SESSION = 4711;
const EARLIER_SESSION = 815;

const mine = (seq: number) => ({ session: SESSION, seq });
const host = (seq: number, session = SESSION) => ({
  last_processed_session: session,
  last_processed_seq: seq,
});

describe("HostDraft", () => {
  afterEach(() => vi.useRealTimers());

  it("shows host values while the user is not editing", () => {
    const draft = new HostDraft();
    expect(draft.accepts(host(0))).toBe(true);
    expect(draft.accepts(host(3))).toBe(true);
    expect(draft.isEditing).toBe(false);
  });

  it("keeps the local value while the user is editing", () => {
    const draft = new HostDraft();
    draft.edit();
    expect(draft.isEditing).toBe(true);
    // Whatever the host has processed: the edit has not been sent.
    expect(draft.accepts(host(0))).toBe(false);
    expect(draft.accepts(host(1_000))).toBe(false);
  });

  it("keeps the sent value until the host has processed its command", () => {
    const draft = new HostDraft();
    draft.edit();
    draft.sent(mine(5));

    expect(draft.accepts(host(3))).toBe(false); // from before the command
    expect(draft.accepts(host(4))).toBe(false);
    expect(draft.accepts(host(5))).toBe(true); // the answer
    expect(draft.accepts(host(5))).toBe(true);
    expect(draft.accepts(host(9))).toBe(true);
  });

  it("takes a snapshot that accounts for a later command for the answer too", () => {
    // The answer itself was coalesced away, or another command was faster to
    // be rendered: what counts is that the command has been processed.
    const draft = new HostDraft();
    draft.sent(mine(5));
    expect(draft.accepts(host(8))).toBe(true);
  });

  it("lets the host's value win as soon as the command is processed", () => {
    // `accepts` does not look at values at all: the host may have refused the
    // command or decided differently, and that is then what is shown.
    const draft = new HostDraft();
    draft.sent(mine(2));
    expect(draft.accepts(host(1))).toBe(false);
    expect(draft.accepts(host(2))).toBe(true);
  });

  it("keeps the newer text when an older send is answered", () => {
    const draft = new HostDraft();
    draft.edit();
    draft.sent(mine(1));
    draft.edit(); // typing on

    expect(draft.accepts(host(1))).toBe(false);
    draft.sent(mine(2));
    expect(draft.accepts(host(1))).toBe(false);
    expect(draft.accepts(host(2))).toBe(true);
  });

  it("waits for the last send only", () => {
    const draft = new HostDraft();
    draft.sent(mine(3));
    draft.sent(mine(7));
    expect(draft.accepts(host(3))).toBe(false);
    expect(draft.accepts(host(6))).toBe(false);
    expect(draft.accepts(host(7))).toBe(true);
  });

  it("does not let time decide anything", () => {
    vi.useFakeTimers();
    const draft = new HostDraft();
    draft.sent(mine(4));

    vi.advanceTimersByTime(60 * 60 * 1000);
    expect(vi.getTimerCount()).toBe(0);
    expect(draft.accepts(host(3))).toBe(false);
    expect(draft.accepts(host(4))).toBe(true);
  });

  it("is not unsettled by a snapshot that goes back behind the answer", () => {
    // The wait is over once the answer was seen.
    const draft = new HostDraft();
    draft.sent(mine(4));
    expect(draft.accepts(host(4))).toBe(true);
    expect(draft.accepts(host(3))).toBe(true);
  });

  describe("a UI that was loaded anew", () => {
    it("shows what the host reports while it waits for nothing", () => {
      // The snapshots still name the UI before it, with high numbers.
      const draft = new HostDraft();
      expect(draft.accepts(host(500, EARLIER_SESSION))).toBe(true);
      expect(draft.accepts(host(0, 0))).toBe(true);
    });

    it("does not take a snapshot for the UI before it for an answer", () => {
      // Its own commands count from 1 again. A snapshot that accounts for
      // command 500 of the earlier UI says nothing about command 1 of this
      // one, however high its number is.
      const draft = new HostDraft();
      draft.sent(mine(1));
      expect(draft.accepts(host(500, EARLIER_SESSION))).toBe(false);
      expect(draft.accepts(host(1, EARLIER_SESSION))).toBe(false);
      expect(draft.accepts(host(0, 0))).toBe(false);
      expect(draft.accepts(host(1))).toBe(true);
    });
  });

  it("waits for nothing when nothing was posted", () => {
    // `post()` returns null for a command it did not post (an invalid one, in
    // a development build): no answer will come, so the host's value applies.
    const draft = new HostDraft();
    draft.edit();
    draft.sent(null);
    expect(draft.isEditing).toBe(false);
    expect(draft.accepts(host(0))).toBe(true);
  });

  describe("reset", () => {
    it("gives up an edit", () => {
      const draft = new HostDraft();
      draft.edit();
      draft.reset();
      expect(draft.isEditing).toBe(false);
      expect(draft.accepts(host(0))).toBe(true);
    });

    it("gives up the wait for an answer", () => {
      const draft = new HostDraft();
      draft.sent(mine(9));
      draft.reset();
      expect(draft.accepts(host(0))).toBe(true);
    });
  });
});
