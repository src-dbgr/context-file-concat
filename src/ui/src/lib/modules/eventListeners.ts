import { elements } from "../dom.js";
import { post } from "../services/backend.js";
import { appState, getState, editorInstance } from "../stores/app.js";
import type { TreeNode } from "../types";
import { get } from "svelte/store";
import { recordBulkExpanded } from "./treeExpansion";

/**
 * Safe event binding helper to avoid duplicate listeners on HMR.
 */
const bound = new WeakMap<EventTarget, Set<string>>();
function on(
  el: EventTarget | null,
  type: string,
  fn: EventListenerOrEventListenerObject
) {
  if (!el) return;
  let set = bound.get(el);
  if (!set) {
    set = new Set();
    bound.set(el, set);
  }
  if (set.has(type)) return;
  el.addEventListener(type, fn);
  set.add(type);
}

/** Clone helpers to update expansion state locally */
function updateAllExpanded(nodes: TreeNode[], expanded: boolean): TreeNode[] {
  return nodes.map((n) =>
    n.is_directory
      ? {
          ...n,
          is_expanded: expanded,
          children: n.children ? updateAllExpanded(n.children, expanded) : [],
        }
      : n
  );
}

export function setupEventListeners() {
  // Generate / Cancel
  on(elements.generateBtn, "click", () => {
    if (getState().is_generating) post("cancelGeneration");
    else post("generatePreview");
  });

  // Save
  on(elements.saveBtn, "click", () => {
    const editor = get(editorInstance);
    if (editor) post("saveFile", editor.getValue());
  });

  // Select / Deselect all (backend)
  on(elements.selectAllBtn, "click", () => post("selectAll"));
  on(elements.deselectAllBtn, "click", () => post("deselectAll"));

  // Expand / Collapse all: instant local UI + remember + best-effort backend
  on(elements.expandAllBtn, "click", () => {
    appState.update((s) => {
      const tree = updateAllExpanded(s.tree, true);
      // Persist desired state so future renders keep it
      recordBulkExpanded(tree, true);
      return { ...s, tree };
    });
    post("expandAll");
  });

  on(elements.collapseAllBtn, "click", () => {
    appState.update((s) => {
      const tree = updateAllExpanded(s.tree, false);
      recordBulkExpanded(tree, false);
      return { ...s, tree };
    });
    post("collapseAll");
  });

  // Undo/redo support for plain inputs
  on(document.body, "focusin", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      import("./undo.js").then(({ getUndoManagerForElement }) =>
        getUndoManagerForElement(target)
      );
    }
  });

  on(document.body, "input", (e: Event) => {
    const target = e.target as HTMLElement;
    if (
      target instanceof HTMLInputElement ||
      target instanceof HTMLTextAreaElement
    ) {
      import("./undo.js").then(({ getUndoManagerForElement }) =>
        getUndoManagerForElement(target).recordState()
      );
    }
  });
}
