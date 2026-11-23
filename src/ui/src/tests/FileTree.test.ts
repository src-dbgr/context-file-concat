/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import { tick } from "svelte";
import FileTree from "$lib/components/FileTree.svelte";
import { appState } from "$lib/stores/app";
import type { AppState } from "$lib/types";
import {
  baseAppState,
  collapsedDir,
  dir,
  file,
  rowsOf,
  treeFields,
  type TestNode,
} from "./helpers/tree";

// Mock backend IPC post() to assert commands fired by UI
vi.mock("$lib/services/backend", () => ({
  post: vi.fn(),
}));
import { post } from "$lib/services/backend";

// ---- Helpers ----------------------------------------------------------------
// Trees are written down nested and seeded as the rows the host lists for
// them (see helpers/tree.ts): all of them, unless a test is about the window.
function row(container: HTMLElement, path: string): HTMLElement {
  const el = container.querySelector<HTMLElement>(
    `.tree-item-container[data-path="${path}"]`,
  );
  if (!el) throw new Error(`no row for ${path}`);
  return el;
}

function rowPaths(container: HTMLElement): string[] {
  return Array.from(
    container.querySelectorAll<HTMLElement>(".tree-item-container"),
  ).map((el) => el.getAttribute("data-path") ?? "");
}

function expansionCommands(): unknown[][] {
  return vi
    .mocked(post)
    .mock.calls.filter(([command]) =>
      ["setExpansion", "loadDirectoryLevel", "expandCollapseAll"].includes(
        command,
      ),
    );
}

function seedState(tree: TestNode[], overrides: Partial<AppState> = {}) {
  appState.set(baseAppState({ ...treeFields(rowsOf(tree)), ...overrides }));
}

// ---- Tests ------------------------------------------------------------------
describe("FileTree", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("virtualizes long lists (renders fewer DOM rows than total items)", async () => {
    const manyFiles: TestNode[] = [];
    for (let i = 0; i < 120; i++)
      manyFiles.push(file(`/repo/src/file-${i}.txt`));
    const root = dir("/repo/src", true, manyFiles);

    seedState([root]);

    const { container } = render(FileTree);

    // Ensure the viewport has a measurable height (jsdom hack)
    const tree = await screen.findByRole("tree");
    Object.defineProperty(tree, "clientHeight", {
      value: 280,
      configurable: true,
    });

    // Trigger re-measure
    window.dispatchEvent(new Event("resize"));

    // Give the component a tick to react
    await new Promise((r) => setTimeout(r, 10));

    const virtualRows = container.querySelectorAll(".virtual-scroll-item");
    expect(virtualRows.length).toBeGreaterThan(0);
    expect(virtualRows.length).toBeLessThan(120);
  });

  it("toolbar buttons fire selection commands", async () => {
    seedState([
      dir("/repo/src", true, [file("/repo/src/a.ts"), file("/repo/src/b.ts")]),
    ]);

    render(FileTree);

    const selectAllBtn = screen.getByRole("button", { name: /^select all$/i });
    const deselectAllBtn = screen.getByRole("button", {
      name: /^deselect all$/i,
    });

    await fireEvent.click(selectAllBtn);
    await fireEvent.click(deselectAllBtn);

    expect(post).toHaveBeenCalledWith("selectAll");
    expect(post).toHaveBeenCalledWith("deselectAll");
  });

  it("bulk expand/collapse sends exactly one expandCollapseAll per click", async () => {
    const nested = dir("/repo/a/nested", false, [file("/repo/a/nested/x.txt")]);
    const subA = dir("/repo/a", false, [file("/repo/a/1.txt"), nested]); // collapsed
    const subB = dir("/repo/b", true, [file("/repo/b/2.txt")]); // expanded
    seedState([subA, subB]);

    render(FileTree);

    const expandAll = await screen.findByRole("button", {
      name: /expand all/i,
    });
    const collapseAll = await screen.findByRole("button", {
      name: /collapse all/i,
    });

    await fireEvent.click(expandAll);
    expect(post).toHaveBeenCalledTimes(1);
    expect(post).toHaveBeenLastCalledWith("expandCollapseAll", true);

    await fireEvent.click(collapseAll);
    expect(post).toHaveBeenCalledTimes(2);
    expect(post).toHaveBeenLastCalledWith("expandCollapseAll", false);

    // No per-directory toggles: each one would trigger a full tree render.
    const commands = vi.mocked(post).mock.calls.map(([command]) => command);
    expect(commands).not.toContain("setExpansion");
  });

  // ---- Collapsed directories: their children are no rows -------------------

  it("renders a collapsed directory without children as an expandable row", async () => {
    seedState([
      collapsedDir("/repo/src", { selection_state: "partial" }),
      file("/repo/README.md"),
    ]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    expect(rowPaths(container)).toEqual(["/repo/src", "/repo/README.md"]);
    const src = row(container, "/repo/src");
    expect(src).toHaveAttribute("aria-expanded", "false");
    // The chevron is there although no child was sent...
    const chevron = src.querySelector(".arrow");
    expect(chevron).not.toBeNull();
    expect(chevron).not.toHaveClass("expanded");
    // It is a control without text; its name says what it does to which row.
    expect(chevron).toHaveAttribute("aria-label", "Expand src");
    // ...and the selection of the hidden descendants is shown.
    expect(src.querySelector("input[type=checkbox]")).toBePartiallyChecked();
  });

  it("shows the selection state the backend computed for collapsed directories", async () => {
    seedState([
      collapsedDir("/repo/all", { selection_state: "full" }),
      collapsedDir("/repo/some", { selection_state: "partial" }),
      collapsedDir("/repo/none", { selection_state: "none" }),
    ]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    // A native checkbox: its own state is what a screen reader is told.
    const checkbox = (path: string) =>
      row(container, path).querySelector<HTMLInputElement>(
        "input[type=checkbox]",
      )!;
    expect(checkbox("/repo/all")).toBeChecked();
    expect(checkbox("/repo/all")).not.toBePartiallyChecked();
    expect(checkbox("/repo/some")).toBePartiallyChecked();
    expect(checkbox("/repo/none")).not.toBeChecked();
    expect(checkbox("/repo/none")).not.toBePartiallyChecked();
    for (const path of ["/repo/all", "/repo/some", "/repo/none"]) {
      expect(checkbox(path)).not.toHaveAttribute("aria-checked");
    }
  });

  it("expanding a collapsed directory asks the backend and changes nothing locally", async () => {
    seedState([collapsedDir("/repo/src")]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(row(container, "/repo/src").querySelector(".arrow")!);

    expect(expansionCommands()).toEqual([
      ["setExpansion", { path: "/repo/src", expanded: true }],
    ]);
    // No optimistic expansion: the row stays collapsed until the backend answers.
    expect(row(container, "/repo/src")).toHaveAttribute(
      "aria-expanded",
      "false",
    );
    expect(rowPaths(container)).toEqual(["/repo/src"]);
  });

  it("clicking the directory name toggles it as well", async () => {
    seedState([collapsedDir("/repo/src")]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(
      row(container, "/repo/src").querySelector(".file-name")!,
    );

    expect(expansionCommands()).toEqual([
      ["setExpansion", { path: "/repo/src", expanded: true }],
    ]);
  });

  it("a double click on a collapsed directory expands it, it does not toggle twice", async () => {
    seedState([collapsedDir("/repo/src")]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    // The snapshot answering the first click has not arrived yet, so the second
    // click hits the same collapsed row.
    const arrow = row(container, "/repo/src").querySelector(".arrow")!;
    await fireEvent.click(arrow);
    await fireEvent.click(arrow);

    expect(expansionCommands()).toEqual([
      ["setExpansion", { path: "/repo/src", expanded: true }],
      ["setExpansion", { path: "/repo/src", expanded: true }],
    ]);
  });

  it("a click on an expanded directory collapses it", async () => {
    seedState([dir("/repo/src", true, [file("/repo/src/a.ts")])]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(row(container, "/repo/src").querySelector(".arrow")!);

    expect(expansionCommands()).toEqual([
      ["setExpansion", { path: "/repo/src", expanded: false }],
    ]);
  });

  it("a click collapses a directory that is expanded but not loaded", async () => {
    // Expanded while a scan ran that was cancelled: nothing to show, but the
    // chevron points down and must close again.
    seedState(
      [dir("/repo/vendor", true, []), dir("/repo/other", true, [])].map(
        (d) => ({ ...d, has_children: true, children_loaded: false }),
      ),
      { is_fully_scanned: false },
    );

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(
      row(container, "/repo/vendor").querySelector(".arrow")!,
    );
    await fireEvent.click(
      row(container, "/repo/other").querySelector(".file-name")!,
    );

    expect(vi.mocked(post).mock.calls).toEqual([
      ["setExpansion", { path: "/repo/vendor", expanded: false }],
      ["setExpansion", { path: "/repo/other", expanded: false }],
    ]);
  });

  it("expanding a directory whose children are not loaded requests them", async () => {
    seedState(
      [
        collapsedDir("/repo/vendor", {
          has_children: false,
          children_loaded: false,
        }),
      ],
      { is_fully_scanned: false },
    );

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(
      row(container, "/repo/vendor").querySelector(".arrow")!,
    );

    // The backend expands the directory as part of the load.
    expect(expansionCommands()).toEqual([
      ["loadDirectoryLevel", "/repo/vendor"],
    ]);
  });

  it("requests a lazy load while a scan is running too", async () => {
    seedState(
      [
        collapsedDir("/repo/vendor", {
          has_children: false,
          children_loaded: false,
        }),
      ],
      { is_scanning: true, is_fully_scanned: false },
    );

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    await fireEvent.click(
      row(container, "/repo/vendor").querySelector(".arrow")!,
    );

    // The scan fills the tree as it goes; the level the user asks for is
    // listed at once.
    expect(expansionCommands()).toEqual([
      ["loadDirectoryLevel", "/repo/vendor"],
    ]);
  });

  it("follows the backend's expansion state in both directions", async () => {
    const collapsed = () => [collapsedDir("/repo/src"), file("/repo/z.txt")];
    const expanded = () => [
      dir("/repo/src", true, [
        collapsedDir("/repo/src/core"),
        file("/repo/src/main.rs"),
      ]),
      file("/repo/z.txt"),
    ];
    seedState(collapsed());

    const { container } = render(FileTree);
    await screen.findByRole("tree");
    await fireEvent.click(row(container, "/repo/src").querySelector(".arrow")!);

    // The backend answers with the children.
    seedState(expanded());
    await waitFor(() =>
      expect(rowPaths(container)).toEqual([
        "/repo/src",
        "/repo/src/core",
        "/repo/src/main.rs",
        "/repo/z.txt",
      ]),
    );
    expect(row(container, "/repo/src")).toHaveAttribute(
      "aria-expanded",
      "true",
    );
    expect(row(container, "/repo/src/core")).toHaveAttribute("aria-level", "2");

    // A later snapshot collapses it again (e.g. "collapse all"): although the
    // user expanded it here, nothing in the UI keeps it open.
    seedState(collapsed());
    await waitFor(() =>
      expect(rowPaths(container)).toEqual(["/repo/src", "/repo/z.txt"]),
    );
    expect(row(container, "/repo/src")).toHaveAttribute(
      "aria-expanded",
      "false",
    );
  });

  it("shows directories the backend expanded on its own (search auto-expand)", async () => {
    seedState([collapsedDir("/repo/src")]);
    const { container } = render(FileTree);
    await screen.findByRole("tree");

    const hit = { ...file("/repo/src/deep/hit.rs"), is_match: true };
    seedState([dir("/repo/src", true, [dir("/repo/src/deep", true, [hit])])], {
      search_query: "hit",
      tree_files_count: 1,
      tree_dirs_count: 2,
    });

    await waitFor(() =>
      expect(rowPaths(container)).toEqual([
        "/repo/src",
        "/repo/src/deep",
        "/repo/src/deep/hit.rs",
      ]),
    );
    expect(
      row(container, "/repo/src/deep/hit.rs").querySelector(".file-name"),
    ).toHaveClass("is-match");
    expect(post).not.toHaveBeenCalled();
  });

  // ---- Keyboard ------------------------------------------------------------

  describe("keyboard expansion", () => {
    async function mountFocusedOn(tree: TestNode[], overrides = {}) {
      seedState(tree, overrides);
      const view = render(FileTree);
      const treeEl = await screen.findByRole("tree");
      // The first row gets the roving focus once the tree is mounted.
      await waitFor(() =>
        expect(
          view.container.querySelector('.tree-item-container[tabindex="0"]'),
        ).not.toBeNull(),
      );
      return { ...view, treeEl };
    }

    it("ArrowRight expands a collapsed directory via the backend", async () => {
      const { treeEl } = await mountFocusedOn([collapsedDir("/repo/src")]);

      await fireEvent.keyDown(treeEl, { key: "ArrowRight" });

      expect(expansionCommands()).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
      ]);
    });

    it("ArrowRight key repeat keeps asking to expand, it never collapses", async () => {
      const { treeEl } = await mountFocusedOn([collapsedDir("/repo/src")]);

      // Key repeat: the snapshot answering the first event is still pending.
      await fireEvent.keyDown(treeEl, { key: "ArrowRight" });
      await fireEvent.keyDown(treeEl, { key: "ArrowRight", repeat: true });
      await fireEvent.keyDown(treeEl, { key: "ArrowRight", repeat: true });

      expect(expansionCommands()).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["setExpansion", { path: "/repo/src", expanded: true }],
      ]);
    });

    it("ArrowRight loads a directory whose children are not loaded", async () => {
      const { treeEl } = await mountFocusedOn(
        [collapsedDir("/repo/vendor", { children_loaded: false })],
        { is_fully_scanned: false },
      );

      await fireEvent.keyDown(treeEl, { key: "ArrowRight" });

      expect(expansionCommands()).toEqual([
        ["loadDirectoryLevel", "/repo/vendor"],
      ]);
    });

    it("ArrowRight on an expanded directory moves to its first child", async () => {
      const { treeEl, container } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);

      await fireEvent.keyDown(treeEl, { key: "ArrowRight" });

      expect(expansionCommands()).toEqual([]);
      await waitFor(() =>
        expect(row(container, "/repo/src/a.ts")).toHaveAttribute(
          "tabindex",
          "0",
        ),
      );
    });

    it("Delete ignores the focused row, as its ignore button does", async () => {
      const { treeEl } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);
      vi.mocked(post).mockClear();

      await fireEvent.keyDown(treeEl, { key: "Delete" });

      expect(vi.mocked(post).mock.calls).toEqual([
        ["addIgnorePath", "/repo/src"],
      ]);
    });

    it("Delete does nothing as a repeat of a held key or with a modifier", async () => {
      const { treeEl } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);
      vi.mocked(post).mockClear();

      for (const init of [
        { repeat: true },
        { shiftKey: true },
        { metaKey: true },
        { ctrlKey: true },
        { altKey: true },
      ]) {
        await fireEvent.keyDown(treeEl, { key: "Delete", ...init });
      }
      // Backspace is the "delete" of a Mac keyboard: not this key.
      await fireEvent.keyDown(treeEl, { key: "Backspace" });

      expect(vi.mocked(post).mock.calls).toEqual([]);
    });

    it("names a row by its name, and the ignore button by the key of the platform", async () => {
      const { container } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);

      expect(screen.getByRole("treeitem", { name: "src" })).toBe(
        row(container, "/repo/src"),
      );
      expect(screen.getByRole("treeitem", { name: "a.ts" })).toBe(
        row(container, "/repo/src/a.ts"),
      );
      // jsdom is no Mac.
      expect(
        row(container, "/repo/src/a.ts").querySelector(".ignore-btn"),
      ).toHaveAttribute("title", "Add this file to ignore patterns (Delete)");
    });

    it("has one tab stop: the focused row, none of the controls in the rows", async () => {
      const { treeEl, container } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);

      const stops = () =>
        [treeEl, ...treeEl.querySelectorAll<HTMLElement>("*")].filter(
          (el) => el.tabIndex >= 0,
        );
      expect(stops()).toEqual([row(container, "/repo/src")]);
      // The controls can still take the focus (a click puts it there).
      for (const control of treeEl.querySelectorAll(
        ".arrow, input, .file-name, .ignore-btn",
      )) {
        expect(control).toHaveAttribute("tabindex", "-1");
      }
      expect(treeEl.querySelector(".ignore-btn")).toHaveAttribute(
        "aria-keyshortcuts",
        "Delete",
      );
    });

    it("ArrowLeft collapses an expanded directory via the backend", async () => {
      const { treeEl } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);

      await fireEvent.keyDown(treeEl, { key: "ArrowLeft" });

      expect(expansionCommands()).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("ArrowLeft key repeat keeps asking to collapse, it never expands", async () => {
      const { treeEl } = await mountFocusedOn([
        dir("/repo/src", true, [file("/repo/src/a.ts")]),
      ]);

      await fireEvent.keyDown(treeEl, { key: "ArrowLeft" });
      await fireEvent.keyDown(treeEl, { key: "ArrowLeft", repeat: true });

      expect(expansionCommands()).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: false }],
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("ArrowLeft on a collapsed directory sends nothing", async () => {
      const { treeEl } = await mountFocusedOn([collapsedDir("/repo/src")]);

      await fireEvent.keyDown(treeEl, { key: "ArrowLeft" });

      expect(post).not.toHaveBeenCalled();
    });

    it("Enter toggles a directory and Space selects it", async () => {
      const { treeEl } = await mountFocusedOn([collapsedDir("/repo/src")]);

      await fireEvent.keyDown(treeEl, { key: "Enter" });
      await fireEvent.keyDown(treeEl, { key: " " });

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["toggleDirectorySelection", "/repo/src"],
      ]);
    });

    describe("on a directory that is expanded but not loaded", () => {
      const expandedUnloaded = (): TestNode[] => [
        {
          ...dir("/repo/vendor", true, []),
          has_children: true,
          children_loaded: false,
        },
      ];
      const mount = () =>
        mountFocusedOn(expandedUnloaded(), { is_fully_scanned: false });

      it("Enter collapses it", async () => {
        const { treeEl } = await mount();
        await fireEvent.keyDown(treeEl, { key: "Enter" });
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/vendor", expanded: false }],
        ]);
      });

      it("ArrowLeft collapses it", async () => {
        const { treeEl } = await mount();
        await fireEvent.keyDown(treeEl, { key: "ArrowLeft" });
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/vendor", expanded: false }],
        ]);
      });

      it("ArrowRight sends nothing (it is expanded and has no rows to enter)", async () => {
        const { treeEl, container } = await mount();
        await fireEvent.keyDown(treeEl, { key: "ArrowRight" });
        expect(post).not.toHaveBeenCalled();
        expect(row(container, "/repo/vendor")).toHaveAttribute("tabindex", "0");
      });

      it("is loaded when it is expanded again after the collapse", async () => {
        const { treeEl } = await mount();
        await fireEvent.keyDown(treeEl, { key: "Enter" });
        // The host answers: collapsed, still not loaded.
        appState.update((s) => ({
          ...s,
          rows: s.rows.map((n) => ({ ...n, is_expanded: false })),
        }));
        await tick();
        await fireEvent.keyDown(treeEl, { key: "Enter" });

        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/vendor", expanded: false }],
          ["loadDirectoryLevel", "/repo/vendor"],
        ]);
      });
    });

    describe("a key is handled once, wherever the focus is inside the row", () => {
      const tree = (): TestNode[] => [
        file("/repo/a.txt"),
        collapsedDir("/repo/src"),
        file("/repo/z.txt"),
      ];

      /** Focus a part of a row and press a key on it, as the browser does. */
      async function pressOn(el: Element, key: string) {
        if (!(el instanceof HTMLElement)) throw new Error("not focusable");
        el.focus();
        expect(document.activeElement).toBe(el);
        const notPrevented = await fireEvent.keyDown(el, { key });
        // Handled by the tree: the default (page scroll on Space) is off.
        expect(notPrevented).toBe(false);
      }
      const part = (container: HTMLElement, path: string, selector: string) =>
        row(container, path).querySelector(selector)!;

      /**
       * Press a key on a control that acts on it by itself. The tree leaves
       * the key alone (nothing posted, default not prevented); the browser
       * then activates the control, which is the click sent here.
       */
      async function activate(el: Element, key: string) {
        if (!(el instanceof HTMLElement)) throw new Error("not focusable");
        el.focus();
        const notPrevented = await fireEvent.keyDown(el, { key });
        expect(notPrevented).toBe(true);
        expect(vi.mocked(post).mock.calls).toEqual([]);
        await fireEvent.click(el);
      }

      for (const key of ["Enter", " "]) {
        it(`${JSON.stringify(key)} on the ignore button of a directory ignores it, once`, async () => {
          const { container } = await mountFocusedOn(tree());
          await activate(part(container, "/repo/src", ".ignore-btn"), key);
          expect(vi.mocked(post).mock.calls).toEqual([
            ["addIgnorePath", "/repo/src"],
          ]);
        });

        it(`${JSON.stringify(key)} on the ignore button of a file ignores it, once`, async () => {
          const { container } = await mountFocusedOn(tree());
          await activate(part(container, "/repo/z.txt", ".ignore-btn"), key);
          expect(vi.mocked(post).mock.calls).toEqual([
            ["addIgnorePath", "/repo/z.txt"],
          ]);
        });
      }

      it("Space on the checkbox of a directory toggles its selection once", async () => {
        const { container } = await mountFocusedOn(tree());
        await activate(
          part(container, "/repo/src", 'input[type="checkbox"]'),
          " ",
        );
        expect(vi.mocked(post).mock.calls).toEqual([
          ["toggleDirectorySelection", "/repo/src"],
        ]);
      });

      it("Space on the checkbox of a file toggles its selection once", async () => {
        const { container } = await mountFocusedOn(tree());
        await activate(
          part(container, "/repo/z.txt", 'input[type="checkbox"]'),
          " ",
        );
        expect(vi.mocked(post).mock.calls).toEqual([
          ["toggleSelection", "/repo/z.txt"],
        ]);
      });

      it("Enter on a checkbox still acts on the row", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(
          part(container, "/repo/z.txt", 'input[type="checkbox"]'),
          "Enter",
        );
        expect(vi.mocked(post).mock.calls).toEqual([
          ["loadFilePreview", "/repo/z.txt"],
        ]);
      });

      it("the other keys of the tree still work on a control", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(
          part(container, "/repo/src", ".ignore-btn"),
          "ArrowRight",
        );
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/src", expanded: true }],
        ]);
      });

      it("Space on a focused directory name selects it and does not expand it", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/src", ".file-name"), " ");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["toggleDirectorySelection", "/repo/src"],
        ]);
      });

      it("Enter on a focused directory name toggles it once", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/src", ".file-name"), "Enter");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/src", expanded: true }],
        ]);
      });

      it("Space and Enter on a focused arrow act like on the row", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/src", ".arrow"), " ");
        await pressOn(part(container, "/repo/src", ".arrow"), "Enter");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["toggleDirectorySelection", "/repo/src"],
          ["setExpansion", { path: "/repo/src", expanded: true }],
        ]);
      });

      it("Space on a focused file name selects it and does not open it", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/z.txt", ".file-name"), " ");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["toggleSelection", "/repo/z.txt"],
        ]);
      });

      it("Enter on a focused file name opens it once", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/z.txt", ".file-name"), "Enter");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["loadFilePreview", "/repo/z.txt"],
        ]);
      });

      it("arrow keys work from a focused name", async () => {
        const { container } = await mountFocusedOn(tree());
        await pressOn(part(container, "/repo/src", ".file-name"), "ArrowRight");
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/src", expanded: true }],
        ]);
        await pressOn(part(container, "/repo/src", ".file-name"), "ArrowDown");
        await waitFor(() =>
          expect(row(container, "/repo/z.txt")).toHaveAttribute(
            "tabindex",
            "0",
          ),
        );
      });

      it("the pointer still works on name and arrow", async () => {
        const { container } = await mountFocusedOn(tree());
        await fireEvent.click(part(container, "/repo/src", ".arrow"));
        await fireEvent.click(part(container, "/repo/src", ".file-name"));
        await fireEvent.click(part(container, "/repo/z.txt", ".file-name"));
        expect(vi.mocked(post).mock.calls).toEqual([
          ["setExpansion", { path: "/repo/src", expanded: true }],
          ["setExpansion", { path: "/repo/src", expanded: true }],
          ["loadFilePreview", "/repo/z.txt"],
        ]);
      });
    });

    it("navigates only over the rows that were sent", async () => {
      const { treeEl, container } = await mountFocusedOn([
        collapsedDir("/repo/a"),
        collapsedDir("/repo/b"),
        file("/repo/c.txt"),
      ]);

      await fireEvent.keyDown(treeEl, { key: "End" });
      await waitFor(() =>
        expect(row(container, "/repo/c.txt")).toHaveAttribute("tabindex", "0"),
      );
      await fireEvent.keyDown(treeEl, { key: "ArrowUp" });
      await waitFor(() =>
        expect(row(container, "/repo/b")).toHaveAttribute("tabindex", "0"),
      );
    });
  });

  // ---- Stats ---------------------------------------------------------------

  describe("stats", () => {
    const stats = (container: HTMLElement) =>
      (container.querySelector(".stats")?.textContent ?? "")
        .replace(/\s+/g, " ")
        .trim();

    it("come from the backend counters, not from the nodes that were sent", async () => {
      // One collapsed row on screen, 1,234 files in 56 folders behind it.
      seedState([collapsedDir("/repo/src")], {
        selected_files_count: 7,
        tree_files_count: 1234,
        tree_dirs_count: 56,
      });

      const { container } = render(FileTree);
      await screen.findByRole("tree");

      await waitFor(() =>
        expect(stats(container)).toBe(
          "Files selected: 7 of total files 1234 • Folders: 56",
        ),
      );
    });

    it("do not change when a directory is expanded", async () => {
      const counters = { tree_files_count: 3, tree_dirs_count: 1 };
      seedState([collapsedDir("/repo/src")], counters);
      const { container } = render(FileTree);
      await screen.findByRole("tree");
      await waitFor(() =>
        expect(stats(container)).toContain("of total files 3"),
      );

      seedState(
        [
          dir("/repo/src", true, [
            file("/repo/src/a.ts"),
            file("/repo/src/b.ts"),
            file("/repo/src/c.ts"),
          ]),
        ],
        counters,
      );

      await waitFor(() => expect(rowPaths(container)).toHaveLength(4));
      expect(stats(container)).toBe(
        "Files selected: 0 of total files 3 • Folders: 1",
      );
    });

    it("keep the unfiltered totals as baseline while a filter is active", async () => {
      seedState([collapsedDir("/repo/src")], {
        tree_files_count: 100,
        tree_dirs_count: 10,
      });
      const { container } = render(FileTree);
      await screen.findByRole("tree");
      await waitFor(() =>
        expect(stats(container)).toContain("of total files 100"),
      );

      // The backend narrows the tree: the counters now describe the filtered tree.
      seedState([collapsedDir("/repo/src")], {
        search_query: "main",
        tree_files_count: 4,
        tree_dirs_count: 2,
      });

      await waitFor(() =>
        expect(stats(container)).toMatch(
          /^Files selected: 0 of total files 100 • Folders: 10 • \S+: 4$/,
        ),
      );

      // Filter cleared: the baseline follows the counters again.
      seedState([collapsedDir("/repo/src")], {
        tree_files_count: 101,
        tree_dirs_count: 10,
      });
      await waitFor(() =>
        expect(stats(container)).toBe(
          "Files selected: 0 of total files 101 • Folders: 10",
        ),
      );
    });
  });

  it("shows placeholder without current_path and supports keyboard activation", async () => {
    seedState([], { current_path: "" });

    render(FileTree);

    const chooseBtn = await screen.findByRole("button", {
      name: /choose directory/i,
    });
    await fireEvent.keyDown(chooseBtn, { key: "Enter" });

    expect(post).toHaveBeenCalledWith("selectDirectory");
  });
});

describe("FileTree – focus when the tree appears", () => {
  /** Lets the deferred focus (requestAnimationFrame) of the tree run. */
  const nextFrame = () =>
    new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));

  const tree = () => [
    dir("/repo/src", true, [file("/repo/src/a.ts")]),
    file("/repo/README.md"),
  ];

  beforeEach(async () => {
    vi.clearAllMocks();
    // Let a deferred focus of a tree of an earlier test run out first.
    await nextFrame();
    await nextFrame();
    document.body.innerHTML = "";
    (document.activeElement as HTMLElement | null)?.blur?.();
  });

  it("focuses the first row when nothing else has the focus", async () => {
    seedState([]);
    const { container } = render(FileTree);
    seedState(tree());
    await tick();
    await nextFrame();
    await nextFrame();

    expect(document.activeElement).toBe(row(container, "/repo/src"));
  });

  it("leaves the focus where the user has put it in the meantime", async () => {
    const elsewhere = document.createElement("button");
    document.body.appendChild(elsewhere);

    seedState([]);
    render(FileTree);
    seedState(tree());
    await tick();
    // Before the deferred focus of the tree runs:
    elsewhere.focus();
    await nextFrame();
    await nextFrame();

    expect(document.activeElement).toBe(elsewhere);
  });

  it("leaves the focus on a control of a row", async () => {
    seedState([]);
    const { container } = render(FileTree);
    seedState(tree());
    await tick();
    const ignore = row(container, "/repo/README.md").querySelector<HTMLElement>(
      ".ignore-btn",
    )!;
    ignore.focus();
    await nextFrame();
    await nextFrame();

    expect(document.activeElement).toBe(ignore);
  });

  it("still moves the focus on keyboard navigation", async () => {
    seedState([]);
    const { container } = render(FileTree);
    seedState(tree());
    await tick();
    await nextFrame();
    await nextFrame();
    const first = row(container, "/repo/src");
    expect(document.activeElement).toBe(first);

    await fireEvent.keyDown(first, { key: "ArrowDown" });
    await nextFrame();
    await nextFrame();

    expect(document.activeElement).toBe(row(container, "/repo/src/a.ts"));
  });
});
