// Keeps directory expansion state stable across backend renders.
// We key by absolute node.path, which is unique per node.

import type { TreeNode } from "$lib/types";

const expansionMemory = new Map<string, boolean>();

export function clearExpansionMemory() {
  expansionMemory.clear();
}

/** Record a single directory's expanded state. */
export function recordDirExpanded(path: string, expanded: boolean) {
  expansionMemory.set(path, expanded);
}

/** Record expanded/collapsed for all directories in a subtree. */
export function recordBulkExpanded(nodes: TreeNode[], expanded: boolean) {
  const stack = [...nodes];
  while (stack.length) {
    const n = stack.pop()!;
    if (n.is_directory) {
      expansionMemory.set(n.path, expanded);
      if (n.children?.length) stack.push(...n.children);
    }
  }
}

/** Apply remembered expansion to an incoming tree. Returns a cloned tree. */
export function applyExpansionMemory(nodes: TreeNode[]): TreeNode[] {
  return nodes.map((n) => applyToNode(n));
}

function applyToNode(n: TreeNode): TreeNode {
  const remembered = expansionMemory.get(n.path);
  const is_expanded = n.is_directory
    ? (remembered ?? n.is_expanded)
    : n.is_expanded;

  const children = n.children?.length
    ? n.children.map((c) => applyToNode(c))
    : [];

  // Return a shallow clone with possibly updated expansion and cloned children
  return { ...n, is_expanded, children };
}
