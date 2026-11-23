// Directory expansion intents.
//
// The backend is the single source of truth for which directories are expanded
// (`expanded_dirs`). The children of a directory are rows only while it is
// expanded there, so an expansion that existed in the UI alone would show an
// empty directory. This module therefore keeps no state: it translates a user
// gesture into the one backend command that changes the expansion, and the
// next `render()` snapshot carries the result (`is_expanded` plus the rows
// that are now part of the list).
//
// Every gesture is sent as the state it should lead to (`setExpansion`), never
// as "flip it". A gesture can fire again before the snapshot
// that answers it has been rendered (key repeat, double click); the repeated
// event still sees the old node. Naming the target makes the repetition
// harmless, whereas a second toggle would undo the first.

import { post } from "$lib/services/backend";
import type { TreeRow } from "$lib/types";

/** The part of a directory row the expansion intents depend on. */
export type ExpandableNode = Pick<
  TreeRow,
  "path" | "is_expanded" | "children_loaded"
>;

/**
 * True if expanding the directory has to fetch its children first.
 *
 * Only a collapsed directory is loaded: the load is what expands it. A
 * directory that is already expanded without loaded children (expanded during
 * a scan that was cancelled) is not loaded again by a gesture; it can be
 * collapsed like any other, and is loaded when it is expanded the next time.
 *
 * This holds while a scan is running as well. The scan delivers the tree piece
 * by piece, but it gets to a directory when it gets to it; the user who expands
 * the directory wants to see it now. The backend lists that one level at once
 * and the scan goes on (it does not add the level a second time).
 */
export function needsLazyLoad(node: ExpandableNode): boolean {
  return !node.is_expanded && !node.children_loaded;
}

function setExpanded(node: ExpandableNode, expanded: boolean): void {
  post("setExpansion", { path: node.path, expanded });
}

/**
 * Expand a directory. A directory whose children are not loaded yet is loaded
 * instead; the backend expands it as part of that load.
 */
export function expandDirectory(node: ExpandableNode): void {
  if (node.is_expanded) return;
  if (needsLazyLoad(node)) {
    post("loadDirectoryLevel", node.path);
  } else {
    setExpanded(node, true);
  }
}

/** Collapse a directory. */
export function collapseDirectory(node: ExpandableNode): void {
  if (node.is_expanded) setExpanded(node, false);
}

/**
 * Flip a directory to the opposite of what the user currently sees (chevron,
 * name, Enter): an expanded directory is collapsed, loaded or not; a collapsed
 * one is expanded, which loads it first if necessary.
 *
 * The target is derived from the rendered node, so two activations before the
 * next snapshot ask for the same state twice instead of flipping twice.
 */
export function toggleDirectory(node: ExpandableNode): void {
  if (node.is_expanded) {
    setExpanded(node, false);
  } else {
    expandDirectory(node);
  }
}

/**
 * Expand or collapse every directory with a single backend command, which is
 * answered with one snapshot instead of one per directory.
 */
export function setAllExpanded(expand: boolean): void {
  post("expandCollapseAll", expand);
}
