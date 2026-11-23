/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/svelte";
import FileTree from "$lib/components/FileTree.svelte";
import { appState } from "$lib/stores/app";
import type { TreeNode } from "$lib/types";

// Mock backend IPC post() to assert commands fired by UI
vi.mock("$lib/services/backend", () => ({
  post: vi.fn(),
}));
import { post } from "$lib/services/backend";

// ---- Helpers ----------------------------------------------------------------
function file(path: string, size = 123): TreeNode {
  const name = path.split(/[\\/]/).pop() || path;
  return {
    path,
    name,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    children: [],
    size,
    has_children: false,
    children_loaded: true,
  };
}

function dir(
  path: string,
  expanded = true,
  children: TreeNode[] = [],
): TreeNode {
  const name = path.split(/[\\/]/).pop() || path;
  return {
    path,
    name,
    is_directory: true,
    is_expanded: expanded,
    is_binary: false,
    is_match: true,
    is_previewed: false,
    selection_state: "none",
    children,
    size: 0,
    has_children: children.length > 0,
    children_loaded: true,
  };
}

/**
 * A collapsed directory as the backend sends it: its children are not part
 * of the snapshot, only the flags describing them.
 */
function collapsedDir(
  path: string,
  overrides: Partial<TreeNode> = {},
): TreeNode {
  return {
    ...dir(path, false, []),
    has_children: true,
    ...overrides,
  };
}

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

function seedState(
  tree: TreeNode[],
  overrides: Partial<Parameters<typeof appState.set>[0]> = {},
) {
  appState.set({
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    tree,
    current_path: "/repo",
    current_config_filename: null,
    status_message: "Status: Ready.",
    total_files_found: 0,
    visible_files_count: 0,
    scan_progress: {
      files_scanned: 0,
      large_files_skipped: 0,
      current_scanning_path: "",
    },
    selected_files_count: 0,
    tree_files_count: 0,
    tree_dirs_count: 0,
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    active_ignore_patterns: [],
    config: {
      ignore_patterns: [],
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
      output_directory: "",
      output_filename: "output.txt",
    },
    ...overrides,
  });
}

// ---- Tests ------------------------------------------------------------------
describe("FileTree", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("virtualizes long lists (renders fewer DOM rows than total items)", async () => {
    const manyFiles: TreeNode[] = [];
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

  // ---- Pruned snapshot: collapsed directories arrive without children -------

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
    // ...and the selection of the hidden descendants is shown.
    expect(src.querySelector("input[type=checkbox]")).toHaveAttribute(
      "aria-checked",
      "mixed",
    );
  });

  it("shows the selection state the backend computed for collapsed directories", async () => {
    seedState([
      collapsedDir("/repo/all", { selection_state: "full" }),
      collapsedDir("/repo/some", { selection_state: "partial" }),
      collapsedDir("/repo/none", { selection_state: "none" }),
    ]);

    const { container } = render(FileTree);
    await screen.findByRole("tree");

    const checked = (path: string) =>
      row(container, path)
        .querySelector("input[type=checkbox]")
        ?.getAttribute("aria-checked");
    expect(checked("/repo/all")).toBe("true");
    expect(checked("/repo/some")).toBe("mixed");
    expect(checked("/repo/none")).toBe("false");
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

  it("does not request a lazy load while a scan is running", async () => {
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

    expect(expansionCommands()).toEqual([
      ["setExpansion", { path: "/repo/vendor", expanded: true }],
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
    async function mountFocusedOn(tree: TreeNode[], overrides = {}) {
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
