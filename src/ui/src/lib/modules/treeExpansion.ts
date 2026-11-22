// Directory expansion intents.
//
// The backend is the single source of truth for which directories are expanded
// (`expanded_dirs`). It only sends the children of expanded directories, so an
// expansion that existed in the UI alone would show an empty directory. This
// module therefore keeps no state: it translates a user gesture into the one
// backend command that changes the expansion, and the next `render()` snapshot
// carries the result (`is_expanded` plus the now visible children).

import { post } from "$lib/services/backend";
import { getState } from "$lib/stores/app";
import type { TreeNode } from "$lib/types";

/** The part of a directory node the expansion intents depend on. */
export type ExpandableNode = Pick<
  TreeNode,
  "path" | "is_expanded" | "children_loaded"
>;

/**
 * True if the directory's children must be fetched before they can be shown.
 *
 * While a scan is running the backend refuses lazy loads: the scan itself
 * delivers the children, so nothing has to be requested.
 */
export function needsLazyLoad(
  node: ExpandableNode,
  isScanning: boolean = getState().is_scanning,
): boolean {
  return node.children_loaded === false && !isScanning;
}

/**
 * Expand a directory. A directory whose children are not loaded yet is loaded
 * instead; the backend expands it as part of that load.
 */
export function expandDirectory(node: ExpandableNode): void {
  if (needsLazyLoad(node)) {
    post("loadDirectoryLevel", node.path);
  } else if (!node.is_expanded) {
    post("toggleExpansion", node.path);
  }
}

/** Collapse a directory. */
export function collapseDirectory(node: ExpandableNode): void {
  if (node.is_expanded) post("toggleExpansion", node.path);
}

/**
 * Flip a directory between expanded and collapsed. A directory without loaded
 * children has nothing to show either way, so toggling it loads it.
 */
export function toggleDirectory(node: ExpandableNode): void {
  if (needsLazyLoad(node)) {
    post("loadDirectoryLevel", node.path);
  } else {
    post("toggleExpansion", node.path);
  }
}

/**
 * Expand or collapse every directory with a single backend command, which is
 * answered with one snapshot instead of one per directory.
 */
export function setAllExpanded(expand: boolean): void {
  post("expandCollapseAll", expand);
}
