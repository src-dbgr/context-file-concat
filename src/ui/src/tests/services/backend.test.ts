/* @vitest-environment jsdom */

// `post()`: the one place where the UI hands a command to the host, and the
// one place that numbers the commands.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

type Message = { seq: number; command: string; payload: unknown };

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

    expect(posted).toEqual([
      { seq: 1, command: "initialize", payload: null },
      {
        seq: 2,
        command: "setExpansion",
        payload: { path: "/repo/src", expanded: true },
      },
      { seq: 3, command: "selectAll", payload: null },
    ]);
  });

  it("returns the number of the command it has posted", () => {
    expect(post("initialize")).toBe(1);
    expect(post("toggleSelection", "/repo/a.ts")).toBe(2);
    expect(posted.map((message) => message.seq)).toEqual([1, 2]);
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
    it("is not posted, and its number is not given to another command", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      const blocked = post("setViewport", { start: 0, count: 0 });
      const next = post("selectAll");

      expect(warn).toHaveBeenCalledTimes(1);
      expect(blocked).toBe(1);
      expect(next).toBe(2);
      // The host never sees 1; the snapshot that accounts for 2 accounts for
      // it as well, so nothing waits for it forever.
      expect(posted).toEqual([{ seq: 2, command: "selectAll", payload: null }]);
    });

    it("is posted as it is in a production build", () => {
      vi.stubEnv("DEV", false);
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      post("setViewport", { start: 0, count: 0 });

      expect(warn).not.toHaveBeenCalled();
      // The host refuses it and answers under its number.
      expect(posted).toEqual([
        { seq: 1, command: "setViewport", payload: { start: 0, count: 0 } },
      ]);
    });
  });
});
