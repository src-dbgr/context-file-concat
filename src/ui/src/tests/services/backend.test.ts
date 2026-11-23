/* @vitest-environment jsdom */

// `post()`: the one place where the UI hands a command to the host, and the
// one place that numbers the commands.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

type Message = {
  session: number;
  seq: number;
  command: string;
  payload: unknown;
};

/** The messages without the session, which is another one for every load. */
const numbered = (messages: Message[]) =>
  messages.map(({ seq, command, payload }) => ({ seq, command, payload }));

let posted: Message[];
let post: (typeof import("$lib/services/backend"))["post"];

beforeEach(async () => {
  posted = [];
  window.ipc = {
    postMessage: (message) => posted.push(JSON.parse(message) as Message),
  };
  // A `post()` that has not posted anything yet.
  vi.resetModules();
  ({ post } = await import("$lib/services/backend"));
});

afterEach(() => {
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

describe("post", () => {
  it("numbers the commands from 1, in the order they are posted", () => {
    post("initialize");
    post("setExpansion", { path: "/repo/src", expanded: true });
    post("selectAll");

    expect(numbered(posted)).toEqual([
      { seq: 1, command: "initialize", payload: null },
      {
        seq: 2,
        command: "setExpansion",
        payload: { path: "/repo/src", expanded: true },
      },
      { seq: 3, command: "selectAll", payload: null },
    ]);
    // An envelope is the name of the message, a command and its payload.
    for (const message of posted) {
      expect(Object.keys(message).sort()).toEqual([
        "command",
        "payload",
        "seq",
        "session",
      ]);
    }
  });

  it("returns the name of the command it has posted", () => {
    const first = post("initialize");
    const second = post("toggleSelection", "/repo/a.ts");

    expect(first).toEqual({ session: posted[0].session, seq: 1 });
    expect(second).toEqual({ session: posted[1].session, seq: 2 });
  });

  it("names one session for as long as the UI is loaded", () => {
    post("initialize");
    post("selectAll");
    post("deselectAll");

    const sessions = new Set(posted.map((message) => message.session));
    expect(sessions.size).toBe(1);
    const [session] = sessions;
    // What the host reads: a positive integer a number holds exactly.
    expect(Number.isSafeInteger(session)).toBe(true);
    expect(session).toBeGreaterThan(0);
  });

  it("is another session when the UI is loaded anew", async () => {
    post("initialize");
    vi.resetModules();
    const reloaded = await import("$lib/services/backend");
    reloaded.post("initialize");

    expect(posted[1].session).not.toBe(posted[0].session);
    // And it counts from 1 again.
    expect(posted.map((message) => message.seq)).toEqual([1, 1]);
  });

  it("keeps the number out of the payload", () => {
    post("updateFilters", {
      searchQuery: "a",
      extensionFilter: "",
      contentSearchQuery: "",
    });
    expect(posted[0].payload).toEqual({
      searchQuery: "a",
      extensionFilter: "",
      contentSearchQuery: "",
    });
  });

  it("counts on, whichever component posts", async () => {
    // Every importer gets the same module, so there is one sequence.
    const again = await import("$lib/services/backend");
    post("selectAll");
    again.post("deselectAll");
    expect(posted.map((message) => message.seq)).toEqual([1, 2]);
  });

  describe("a command that is not valid (development builds)", () => {
    it("is not posted and uses up no number", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      post("selectAll");
      const blocked = post("setViewport", { start: 0, count: 0 });
      const next = post("deselectAll");

      expect(warn).toHaveBeenCalledTimes(1);
      // Nothing was posted, so there is nothing to wait for ...
      expect(blocked).toBeNull();
      // ... and the host sees every number: none is skipped.
      expect(next?.seq).toBe(2);
      expect(numbered(posted)).toEqual([
        { seq: 1, command: "selectAll", payload: null },
        { seq: 2, command: "deselectAll", payload: null },
      ]);
    });

    it("is posted as it is in a production build", () => {
      vi.stubEnv("DEV", false);
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      post("setViewport", { start: 0, count: 0 });

      expect(warn).not.toHaveBeenCalled();
      // The host refuses it and answers under its number.
      expect(numbered(posted)).toEqual([
        { seq: 1, command: "setViewport", payload: { start: 0, count: 0 } },
      ]);
    });
  });
});
