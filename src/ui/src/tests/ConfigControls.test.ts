/* @vitest-environment jsdom */

// The controls that edit the configuration and the filters (Sidebar, StatusBar).
//
// The host owns the configuration. The UI shows the last snapshot and sends
// one intent per edit; it never sends its copy of the configuration back. The
// tests play the host by hand. `post()` numbers the commands, and a snapshot
// states the number of the last command it accounts for:
//
// - `hostAnswers()` is a snapshot of a host that has processed everything the
//   UI has posted so far;
// - `hostSends()` is a snapshot from before that (scan progress that was on
//   its way, the answer to an earlier command): it accounts for no more than
//   the last `hostAnswers()` did.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import Sidebar from "$lib/components/Sidebar.svelte";
import StatusBar from "$lib/components/StatusBar.svelte";
import { appState, getState } from "$lib/stores/app";
import { deleteLineBackward, deleteWordBackward } from "$lib/modules/actions";
import { UiStateSchema } from "$lib/ipc/schema";
import type { AppState, Config } from "$lib/types";
import { makeValidUiState } from "./contracts/factories";
import { validateCommand } from "./contracts/helpers";

vi.mock("$lib/services/backend", () => ({
  post: vi.fn(),
}));
import { post } from "$lib/services/backend";

type Overrides = Partial<Omit<AppState, "config">> & {
  config?: Partial<Config>;
};

/** This load of the UI, as `post()` names it. */
const SESSION = 4711;
/** The number `post()` gave the last command. */
let lastPosted = 0;
/** The number of the last command (of this UI) the host has processed. */
let processed = 0;

function snapshot(overrides: Overrides = {}): AppState {
  const base = UiStateSchema.parse(makeValidUiState());
  return {
    ...base,
    last_processed_session: processed === 0 ? 0 : SESSION,
    last_processed_seq: processed,
    ...overrides,
    config: { ...base.config, ...overrides.config },
  };
}

/**
 * A snapshot of the host arrives: the store is replaced, as `render()` does.
 * It accounts for the commands the host had processed before (see above).
 */
async function hostSends(overrides: Overrides = {}) {
  appState.set(snapshot(overrides));
  await tick();
}

/** The host has processed every command posted so far and sends a snapshot. */
async function hostAnswers(overrides: Overrides = {}) {
  processed = lastPosted;
  await hostSends(overrides);
}

function input(id: string): HTMLInputElement {
  const el = document.getElementById(id);
  if (!(el instanceof HTMLInputElement)) throw new Error(`no input #${id}`);
  return el;
}

async function type(el: HTMLInputElement, value: string) {
  await fireEvent.input(el, { target: { value } });
}

const posted = () => vi.mocked(post).mock.calls;

beforeEach(() => {
  lastPosted = 0;
  processed = 0;
  vi.mocked(post).mockReset();
  // Like the real `post()`: every command gets the next number.
  vi.mocked(post).mockImplementation(() => ({
    session: SESSION,
    seq: ++lastPosted,
  }));
  appState.set(snapshot());
});

describe("Sidebar: ignore patterns are edited with intents", () => {
  it("sends one addIgnorePattern per chip, never the config", async () => {
    render(Sidebar);
    const chips = () =>
      Array.from(
        document.querySelectorAll<HTMLButtonElement>(".common-pattern-chip"),
      );
    const [first, second] = chips().map((chip) => chip.textContent!.trim());

    // Two quick clicks: no snapshot arrives in between, so the UI still shows
    // the configuration from before the first click.
    await fireEvent.click(chips()[0]);
    await fireEvent.click(chips()[1]);

    expect(posted()).toEqual([
      ["addIgnorePattern", { pattern: first }],
      ["addIgnorePattern", { pattern: second }],
    ]);
    // The UI does not edit its copy; the chips move when the host answers.
    expect(getState().config.ignore_patterns).toEqual([]);

    await hostSends({ config: { ignore_patterns: [first, second] } });
    expect(chips().map((chip) => chip.textContent!.trim())).not.toContain(
      first,
    );
    expect(
      Array.from(document.querySelectorAll(".current-pattern-chip span")).map(
        (el) => el.textContent,
      ),
    ).toEqual([first, second].sort((a, b) => a.localeCompare(b)));
  });

  it("adds the typed pattern trimmed, on click and on Enter", async () => {
    render(Sidebar);
    const field = input("new-ignore-pattern");

    await type(field, "  dist/  ");
    await fireEvent.click(document.getElementById("add-pattern-btn")!);
    expect(field.value).toBe("");

    await type(field, "*.bak");
    await fireEvent.keyDown(field, { key: "Enter" });

    await type(field, "   ");
    await fireEvent.keyDown(field, { key: "Enter" });

    expect(posted()).toEqual([
      ["addIgnorePattern", { pattern: "dist/" }],
      ["addIgnorePattern", { pattern: "*.bak" }],
    ]);
  });

  it("removes one pattern and clears all patterns by intent", async () => {
    appState.set(snapshot({ config: { ignore_patterns: ["a", "b"] } }));
    render(Sidebar);

    await fireEvent.click(
      document.querySelector('[aria-label="Remove pattern b"]')!,
    );
    await fireEvent.click(document.getElementById("delete-all-patterns-btn")!);

    expect(posted()).toEqual([
      ["removeIgnorePattern", { pattern: "b" }],
      ["clearIgnorePatterns"],
    ]);
  });

  it("removes a blank pattern of an imported config with a valid command", async () => {
    appState.set(snapshot({ config: { ignore_patterns: ["a", "  "] } }));
    render(Sidebar);

    await fireEvent.click(
      document.querySelector('[aria-label="Remove pattern   "]')!,
    );

    expect(posted()).toEqual([["removeIgnorePattern", { pattern: "  " }]]);
    // What DEV and E2E builds check before sending.
    expect(() =>
      validateCommand("removeIgnorePattern", { pattern: "  " }),
    ).not.toThrow();
  });
});

describe("Sidebar and StatusBar: options are set one at a time", () => {
  it("sends the toggled option only and shows what the host reports", async () => {
    render(Sidebar);
    render(StatusBar);

    const cases = [
      ["case-sensitive", "case_sensitive_search"],
      ["remove-empty-dirs", "remove_empty_directories"],
      ["include-tree", "include_tree_by_default"],
      ["relative-paths", "use_relative_paths"],
    ] as const;

    for (const [id, option] of cases) {
      vi.mocked(post).mockClear();
      expect(input(id).checked).toBe(false);

      await fireEvent.click(input(id));
      expect(posted()).toEqual([["setConfigOption", { option, value: true }]]);
      // The store is the host's snapshot; a click does not write to it.
      expect(getState().config[option]).toBe(false);

      await hostAnswers({ config: { [option]: true } });
      expect(input(id).checked).toBe(true);

      vi.mocked(post).mockClear();
      await fireEvent.click(input(id));
      expect(posted()).toEqual([["setConfigOption", { option, value: false }]]);
      await hostAnswers();
      expect(input(id).checked).toBe(false);
    }
  });

  it("commits the output file name when the field is left", async () => {
    render(StatusBar);
    const field = input("output-filename");
    expect(field.value).toBe("output.txt");

    await type(field, "context.md");
    expect(posted()).toEqual([]);
    await fireEvent.change(field);

    expect(posted()).toEqual([
      ["setConfigOption", { option: "output_filename", value: "context.md" }],
    ]);
  });

  it("sends an emptied output directory as null and shows null as empty", async () => {
    appState.set(snapshot({ config: { output_directory: "/out" } }));
    render(StatusBar);
    const field = input("output-dir");
    expect(field.value).toBe("/out");

    await type(field, "");
    await fireEvent.change(field);
    expect(posted()).toEqual([
      ["setConfigOption", { option: "output_directory", value: null }],
    ]);
    await hostAnswers({ config: { output_directory: null } });
    expect(field.value).toBe("");

    vi.mocked(post).mockClear();
    await type(field, "/elsewhere");
    await fireEvent.change(field);
    expect(posted()).toEqual([
      ["setConfigOption", { option: "output_directory", value: "/elsewhere" }],
    ]);
  });

  it("shows a directory the host has picked", async () => {
    render(StatusBar);
    await hostSends({ config: { output_directory: "/picked" } });
    expect(input("output-dir").value).toBe("/picked");
  });
});

describe("text being typed survives snapshots", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("keeps the output file name while it is edited and until its command is answered", async () => {
    render(StatusBar);
    const field = input("output-filename");

    await type(field, "contex");
    // E.g. scan progress: a snapshot with the old configuration.
    await hostSends({ status_message: "Scanning..." });
    expect(field.value).toBe("contex");

    await type(field, "context.md");
    await fireEvent.change(field);
    // Produced before the host has processed the command.
    await hostSends({ status_message: "Still scanning..." });
    expect(field.value).toBe("context.md");

    await hostAnswers({ config: { output_filename: "context.md" } });
    expect(field.value).toBe("context.md");

    // Afterwards the host's value is shown again (e.g. an imported config).
    await hostSends({ config: { output_filename: "imported.txt" } });
    expect(field.value).toBe("imported.txt");
  });

  it("returns to the host's value when an edit is abandoned", async () => {
    render(StatusBar);
    const field = input("output-filename");

    await type(field, "x");
    await hostSends({ config: { output_filename: "from-host.txt" } });
    expect(field.value).toBe("x");

    // Left without a change event, as when the text was typed back.
    await fireEvent.blur(field);
    await tick();
    expect(field.value).toBe("from-host.txt");
    expect(posted()).toEqual([]);
  });

  it("does not reset a filter text when a snapshot arrives during the debounce", async () => {
    render(Sidebar);
    const search = input("search-query");
    const content = input("content-search-query");

    await type(search, "mai");
    vi.advanceTimersByTime(200);
    // The debounce has not fired; the host still knows no filter.
    await hostSends({ status_message: "Scan progress" });
    expect(search.value).toBe("mai");

    await type(search, "main");
    await type(content, "needle");
    await hostSends({ status_message: "More scan progress" });
    expect(search.value).toBe("main");
    expect(content.value).toBe("needle");
    expect(posted()).toEqual([]);

    vi.advanceTimersByTime(300);
    expect(posted()).toEqual([
      [
        "updateFilters",
        {
          searchQuery: "main",
          extensionFilter: "",
          contentSearchQuery: "needle",
        },
      ],
    ]);
  });

  it("ignores snapshots from before the filters were sent, then follows the host", async () => {
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    vi.advanceTimersByTime(300);
    expect(posted()).toHaveLength(1);

    // Still on its way when the filters were sent.
    await hostSends({ search_query: "" });
    expect(search.value).toBe("main");

    // The answer to updateFilters.
    await hostAnswers({ search_query: "main" });
    expect(search.value).toBe("main");

    // From now on the host is followed again, e.g. when it resets the filters.
    await hostSends({ search_query: "" });
    expect(search.value).toBe("");
  });

  it("does not take a snapshot for the answer because it shows the sent text", async () => {
    // What counts is the number, not the value. The host showed "main" before
    // (an earlier state), the user types it again, and a snapshot from before
    // the command is still under way: it is not the answer, and what follows
    // it is not shown yet either.
    appState.set(snapshot({ search_query: "main" }));
    render(Sidebar);
    const search = input("search-query");

    await type(search, "mai");
    vi.advanceTimersByTime(300);
    await type(search, "main");
    vi.advanceTimersByTime(300);
    expect(posted()).toHaveLength(2);

    await hostSends({ search_query: "main" });
    await hostSends({ search_query: "something older" });
    expect(search.value).toBe("main");

    await hostAnswers({ search_query: "main" });
    expect(search.value).toBe("main");
  });

  it("keeps typing on while an earlier filter text is answered", async () => {
    render(Sidebar);
    const search = input("search-query");

    await type(search, "ma");
    vi.advanceTimersByTime(300);
    await type(search, "main");
    await hostAnswers({ search_query: "ma" }); // answer to the first send
    expect(search.value).toBe("main");

    vi.advanceTimersByTime(300);
    expect(posted().map(([, payload]) => payload)).toMatchObject([
      { searchQuery: "ma" },
      { searchQuery: "main" },
    ]);
    // The first answer again (it accounts for the first command only).
    await hostSends({ search_query: "ma" });
    expect(search.value).toBe("main");
    await hostAnswers({ search_query: "main" });
    expect(search.value).toBe("main");
  });

  it("shows what the host has decided as soon as it answers", async () => {
    render(Sidebar);
    const search = input("search-query");
    const content = input("content-search-query");

    await type(content, "needle");
    vi.advanceTimersByTime(300);
    // The host has processed the command and reports other filters (it
    // refused the command, or something else has changed them since): that
    // is shown at once, without waiting for anything.
    await hostAnswers({ search_query: "from-host", content_search_query: "" });
    expect(content.value).toBe("");
    expect(search.value).toBe("from-host");
  });

  it("follows a host that answers with the filters it had before", async () => {
    render(Sidebar);
    const content = input("content-search-query");

    await type(content, "needle");
    vi.advanceTimersByTime(300);
    // The same values as before the send, from before the command...
    await hostSends({ status_message: "something else happened" });
    expect(content.value).toBe("needle");
    // ...and as the answer to it.
    await hostAnswers({ status_message: "refused" });
    expect(content.value).toBe("");
  });

  it("takes the answer to a later command for the answer to its own", async () => {
    // The answer to `updateFilters` was coalesced with the one to a command
    // posted after it: one snapshot accounts for both.
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    vi.advanceTimersByTime(300);
    lastPosted += 3; // other commands of the UI
    await hostAnswers({ search_query: "main", status_message: "all done" });
    await hostSends({ search_query: "" });
    expect(search.value).toBe("");
  });

  describe("a host that takes its time", () => {
    const AN_HOUR = 60 * 60 * 1000;

    it("keeps the typed query for as long as no answer arrives", async () => {
      render(Sidebar);
      const content = input("content-search-query");

      await type(content, "needle");
      vi.advanceTimersByTime(300);
      // Nothing is waiting to run: no timer decides about the text.
      expect(vi.getTimerCount()).toBe(0);
      vi.advanceTimersByTime(AN_HOUR);
      await hostSends({ status_message: "from before the command" });
      await tick();
      expect(content.value).toBe("needle");

      await hostAnswers({ content_search_query: "needle" });
      expect(content.value).toBe("needle");
    });

    it("sends the query along when another filter is typed meanwhile", async () => {
      render(Sidebar);
      const content = input("content-search-query");
      const search = input("search-query");

      await type(content, "needle");
      vi.advanceTimersByTime(300);
      vi.advanceTimersByTime(AN_HOUR);
      await tick();

      await type(search, "main");
      vi.advanceTimersByTime(300);
      expect(posted().map(([, payload]) => payload)).toEqual([
        { searchQuery: "", extensionFilter: "", contentSearchQuery: "needle" },
        {
          searchQuery: "main",
          extensionFilter: "",
          contentSearchQuery: "needle",
        },
      ]);
      expect(content.value).toBe("needle");
    });

    it("keeps a committed output file name", async () => {
      render(StatusBar);
      const field = input("output-filename");

      await type(field, "context.md");
      await fireEvent.change(field);
      vi.advanceTimersByTime(AN_HOUR);
      await hostSends({ status_message: "from before the command" });
      expect(field.value).toBe("context.md");
    });
  });

  it("shows the host's output file name again when it answers with the old one", async () => {
    render(StatusBar);
    const field = input("output-filename");

    await type(field, "context.md");
    await fireEvent.change(field);
    // The answer: the host has kept its value (the same as before the send).
    await hostAnswers({ status_message: "answered" });
    expect(field.value).toBe("output.txt");
  });

  it("shows the output directory the host has made of the one that was entered", async () => {
    render(StatusBar);
    const field = input("output-dir");

    await type(field, "/out/");
    await fireEvent.change(field);
    await hostSends({ status_message: "from before the command" });
    expect(field.value).toBe("/out/");
    await hostAnswers({ config: { output_directory: "/out" } });
    expect(field.value).toBe("/out");
  });

  it("does not take a snapshot made for an earlier load of the UI for the answer", async () => {
    // After a reload the host's snapshots name the UI before it, with a high
    // number, until it has processed the first command of this one.
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    vi.advanceTimersByTime(300);
    await hostSends({
      search_query: "",
      last_processed_session: 815,
      last_processed_seq: 500,
    });
    expect(search.value).toBe("main");

    await hostAnswers({ search_query: "main" });
    await hostSends({ search_query: "" });
    expect(search.value).toBe("");
  });

  it("follows the host when a text could not be posted", async () => {
    // What `post()` does with a command that is not valid (development
    // builds): nothing is posted, so no answer will come.
    vi.mocked(post).mockImplementation(() => null);
    render(StatusBar);
    const field = input("output-filename");

    await type(field, "context.md");
    await fireEvent.change(field);
    expect(field.value).toBe("context.md");
    await hostSends({ status_message: "anything" });
    expect(field.value).toBe("output.txt");
  });

  it("shows the cleared filters when the directory is cleared right after typing", async () => {
    render(Sidebar);
    const content = input("content-search-query");

    await type(content, "needle");
    vi.advanceTimersByTime(300);
    expect(posted()).toHaveLength(1);

    // "Clear directory" right behind the filters: the snapshot that follows
    // differs from what was sent.
    await hostAnswers({ current_path: "", content_search_query: "" });
    expect(content.value).toBe("");
    expect(posted()).toHaveLength(1);
  });

  it("does not apply an unsent filter text to another directory", async () => {
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    vi.advanceTimersByTime(200); // still within the debounce
    await hostSends({ current_path: "/another/project", search_query: "" });
    expect(search.value).toBe("");

    vi.advanceTimersByTime(300);
    await tick();
    expect(posted()).toEqual([]);
    expect(search.value).toBe("");
  });

  it("does not wait for the answer to a text sent to the previous directory", async () => {
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    vi.advanceTimersByTime(300);
    // The other directory is shown by a snapshot from before the command
    // (a drop that was under way): the filters belong to the directory they
    // were typed for.
    await hostSends({ current_path: "/another/project", search_query: "" });
    expect(search.value).toBe("");

    // Typing for the new directory works as usual.
    await type(search, "lib");
    vi.advanceTimersByTime(300);
    expect(posted().map(([, payload]) => payload)).toMatchObject([
      { searchQuery: "main" },
      { searchQuery: "lib" },
    ]);
  });

  it("keeps the text being typed while snapshots of the same directory arrive", async () => {
    appState.set(snapshot({ current_path: "/project" }));
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    await hostSends({ current_path: "/project", is_scanning: true });
    await hostSends({ current_path: "/project", is_scanning: false });
    expect(search.value).toBe("main");

    vi.advanceTimersByTime(300);
    expect(posted()).toHaveLength(1);
  });

  it("drops a pending filter text when the directory is closed", async () => {
    render(Sidebar);
    const search = input("search-query");

    await type(search, "main");
    await hostSends({ current_path: "", search_query: "" });
    vi.advanceTimersByTime(300);
    await tick();

    expect(posted()).toEqual([]);
    expect(search.value).toBe("");
  });

  it("sends the filters after a word or a line was deleted with the keyboard", async () => {
    appState.set(snapshot({ search_query: "foo bar" }));
    render(Sidebar);
    const search = input("search-query");
    expect(search.value).toBe("foo bar");

    // Alt+Backspace: the command changes the field from script.
    search.setSelectionRange(7, 7);
    deleteWordBackward(search);
    expect(search.value).toBe("foo ");
    // A snapshot during the debounce must not bring the text back.
    await hostSends({ search_query: "foo bar", status_message: "progress" });
    expect(search.value).toBe("foo ");

    vi.advanceTimersByTime(300);
    expect(posted()).toMatchObject([
      ["updateFilters", { searchQuery: "foo " }],
    ]);

    // Cmd+Backspace
    search.setSelectionRange(4, 4);
    deleteLineBackward(search);
    vi.advanceTimersByTime(300);
    expect(posted()).toHaveLength(2);
    expect(posted()[1]).toMatchObject(["updateFilters", { searchQuery: "" }]);
  });

  it("shows the filters of the host when nothing is being typed", async () => {
    appState.set(snapshot({ search_query: "lib", extension_filter: "rs" }));
    render(Sidebar);
    expect(input("search-query").value).toBe("lib");
    expect(input("extension-filter").value).toBe("rs");

    await hostSends({ content_search_query: "todo" });
    expect(input("search-query").value).toBe("");
    expect(input("content-search-query").value).toBe("todo");
  });
});
