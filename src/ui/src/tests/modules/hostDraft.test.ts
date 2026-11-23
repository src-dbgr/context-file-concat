import { describe, it, expect } from "vitest";
import { ACK_TIMEOUT_MS, HostDraft } from "$lib/modules/hostDraft";

function draftWithClock() {
  const clock = { now: 1000 };
  const draft = new HostDraft<string>(Object.is, () => clock.now);
  return { draft, clock };
}

describe("HostDraft", () => {
  it("shows host values while the user is not editing", () => {
    const { draft } = draftWithClock();
    expect(draft.accepts("a")).toBe(true);
    expect(draft.accepts("b")).toBe(true);
    expect(draft.isEditing).toBe(false);
  });

  it("keeps the local value while the user is editing", () => {
    const { draft, clock } = draftWithClock();
    draft.edit();
    expect(draft.isEditing).toBe(true);
    expect(draft.accepts("host")).toBe(false);
    // Not limited in time: the user may think as long as they like.
    clock.now += ACK_TIMEOUT_MS * 10;
    expect(draft.accepts("host")).toBe(false);
  });

  it("skips stale host values until the sent value is acknowledged", () => {
    const { draft } = draftWithClock();
    draft.edit();
    draft.sent("abc");

    expect(draft.accepts("")).toBe(false); // produced before the host saw "abc"
    expect(draft.accepts("ab")).toBe(false);
    expect(draft.accepts("abc")).toBe(true); // the acknowledgement
    expect(draft.accepts("changed by the host")).toBe(true);
  });

  it("keeps the newer text when an older send is acknowledged", () => {
    const { draft } = draftWithClock();
    draft.edit();
    draft.sent("a");
    draft.edit(); // typing on: "ab"

    expect(draft.accepts("a")).toBe(false);
    draft.sent("ab");
    expect(draft.accepts("a")).toBe(false);
    expect(draft.accepts("ab")).toBe(true);
  });

  it("lets the host win when the acknowledgement does not arrive", () => {
    const { draft, clock } = draftWithClock();
    draft.sent("abc");

    clock.now += ACK_TIMEOUT_MS - 1;
    expect(draft.accepts("")).toBe(false);
    clock.now += 1;
    expect(draft.accepts("")).toBe(true);
    expect(draft.accepts("anything")).toBe(true);
  });

  it("does not let an expired wait override an edit in progress", () => {
    const { draft, clock } = draftWithClock();
    draft.sent("abc");
    draft.edit();
    clock.now += ACK_TIMEOUT_MS;
    expect(draft.accepts("")).toBe(false);
    draft.sent("abcd");
    expect(draft.accepts("abcd")).toBe(true);
  });

  it("reset gives up the local value", () => {
    const { draft } = draftWithClock();
    draft.edit();
    draft.sent("abc");
    draft.edit();
    draft.reset();
    expect(draft.isEditing).toBe(false);
    expect(draft.accepts("host")).toBe(true);
  });

  it("uses the given equality for the acknowledgement", () => {
    const draft = new HostDraft<{ q: string }>(
      (a, b) => a.q === b.q,
      () => 0,
    );
    draft.sent({ q: "x" });
    expect(draft.accepts({ q: "y" })).toBe(false);
    expect(draft.accepts({ q: "x" })).toBe(true);
  });
});
