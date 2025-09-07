import { describe, it, expect, beforeEach } from "vitest";
import {
  clearExpansionMemory,
  recordDirExpanded,
  recordBulkExpanded,
  applyExpansionMemory,
} from "$lib/modules/treeExpansion";
import type { TreeNode } from "$lib/types";

function d(
  path: string,
  { expanded = false, children = [] as TreeNode[] } = {}
): TreeNode {
  return {
    path,
    name: path.split(/[\\/]/).pop() || path,
    is_directory: true,
    is_expanded: expanded,
    is_binary: false,
    is_match: false,
    is_previewed: false,
    selection_state: "none",
    children,
    size: 0,
  };
}

function f(path: string): TreeNode {
  return {
    path,
    name: path.split(/[\\/]/).pop() || path,
    is_directory: false,
    is_expanded: false,
    is_binary: false,
    is_match: false,
    is_previewed: false,
    selection_state: "none",
    children: [],
    size: 123,
  };
}

describe("treeExpansion memory", () => {
  beforeEach(() => {
    clearExpansionMemory();
  });

  it("applies remembered single directory state", () => {
    const root = d("/root", { expanded: false, children: [d("/root/a")] });
    recordDirExpanded("/root", true);

    const applied = applyExpansionMemory([root])[0];
    expect(applied.is_expanded).toBe(true);
    expect(applied.children.length).toBe(1);
  });

  it("bulk marks all directories expanded", () => {
    const tree: TreeNode[] = [
      d("/r", {
        expanded: false,
        children: [d("/r/sub", { expanded: false }), f("/r/file.txt")],
      }),
    ];
    recordBulkExpanded(tree, true);
    const applied = applyExpansionMemory(tree);

    const r = applied[0];
    expect(r.is_expanded).toBe(true);
    expect(r.children[0].is_expanded).toBe(true); // /r/sub

    // file nodes unaffected
    const file = r.children.find((c: TreeNode) => !c.is_directory)!;
    expect(file.is_expanded).toBe(false);
  });

  it("clearExpansionMemory resets overrides", () => {
    const node = d("/x", { expanded: false });
    recordDirExpanded("/x", true);
    clearExpansionMemory();

    const applied = applyExpansionMemory([node])[0];
    expect(applied.is_expanded).toBe(false);
  });

  it("applyExpansionMemory returns clones (no mutation of input)", () => {
    const input = d("/clone", {
      expanded: false,
      children: [d("/clone/sub")],
    });
    recordDirExpanded("/clone", true);

    const [out] = applyExpansionMemory([input]);
    expect(out).not.toBe(input);
    expect(out.children[0]).not.toBe(input.children[0]);
    expect(input.is_expanded).toBe(false);
    expect(out.is_expanded).toBe(true);
  });
});
