/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
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
  };
}

function dir(
  path: string,
  expanded = true,
  children: TreeNode[] = []
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
  };
}

function seedState(
  tree: TreeNode[],
  overrides: Partial<Parameters<typeof appState.set>[0]> = {}
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
    selected_files_count: 0,
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

  it("bulk expand/collapse toggles only directories that differ", async () => {
    const subA = dir("/repo/a", false, [file("/repo/a/1.txt")]); // collapsed
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
    await fireEvent.click(collapseAll);

    expect(post).toHaveBeenCalledWith("toggleExpansion", "/repo/a");
    expect(post).toHaveBeenCalledWith("toggleExpansion", "/repo/b");
  });

  it("shows placeholder without current_path and supports keyboard activation", async () => {
    seedState([], { current_path: null });

    render(FileTree);

    const chooseBtn = await screen.findByRole("button", {
      name: /choose directory/i,
    });
    await fireEvent.keyDown(chooseBtn, { key: "Enter" });

    expect(post).toHaveBeenCalledWith("selectDirectory");
  });
});
