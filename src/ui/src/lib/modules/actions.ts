import { editorInstance } from "../stores/app.js";
import type { FocusContext } from "../types.js";
import { get } from "svelte/store";

function findWordBoundary(
  value: string,
  position: number,
  direction: "forward" | "backward",
): number {
  let i = position;

  if (direction === "forward") {
    while (i < value.length && /\w/.test(value.charAt(i))) {
      i++;
    }
    while (i < value.length && /\W/.test(value.charAt(i))) {
      i++;
    }
    return i;
  } else {
    while (i > 0 && /\W/.test(value.charAt(i - 1))) {
      i--;
    }
    while (i > 0 && /\w/.test(value.charAt(i - 1))) {
      i--;
    }
    return i;
  }
}

export function moveWord(
  element: HTMLInputElement | HTMLTextAreaElement,
  direction: "forward" | "backward",
) {
  const newPos = findWordBoundary(
    element.value,
    element.selectionStart ?? 0,
    direction,
  );
  element.setSelectionRange(newPos, newPos);
}

export function selectWord(
  element: HTMLInputElement | HTMLTextAreaElement,
  direction: "forward" | "backward",
) {
  if (direction === "forward") {
    const newPos = findWordBoundary(
      element.value,
      element.selectionEnd ?? 0,
      "forward",
    );
    element.setSelectionRange(element.selectionStart, newPos);
  } else {
    const newPos = findWordBoundary(
      element.value,
      element.selectionStart ?? 0,
      "backward",
    );
    element.setSelectionRange(newPos, element.selectionEnd);
  }
}

type TextField = HTMLInputElement | HTMLTextAreaElement;

/** The kinds of deletion the keyboard commands perform (InputEvent.inputType). */
type DeleteInputType =
  "deleteContentBackward" | "deleteWordBackward" | "deleteSoftLineBackward";

/**
 * Replaces the text of a field the way an edit by the user does: the caret is
 * placed and an `input` event is dispatched. Setting `value` from script fires
 * no event, so without it nothing that listens to the field (Svelte bindings,
 * the filter debounce, the undo history) would learn about the change.
 *
 * Nothing is dispatched if the text stays the same, as for a native edit.
 */
function applyDeletion(
  element: TextField,
  value: string,
  caret: number,
  inputType: DeleteInputType,
): void {
  const changed = element.value !== value;
  if (changed) element.value = value;
  element.setSelectionRange(caret, caret);
  if (changed) {
    element.dispatchEvent(
      new InputEvent("input", { bubbles: true, inputType }),
    );
  }
}

/** Removes the selection, if there is one. Returns whether it did. */
function deleteSelection(element: TextField): boolean {
  const start = element.selectionStart ?? 0;
  const end = element.selectionEnd ?? 0;
  if (start === end) return false;
  const min = Math.min(start, end);
  const max = Math.max(start, end);
  applyDeletion(
    element,
    element.value.slice(0, min) + element.value.slice(max),
    min,
    "deleteContentBackward",
  );
  return true;
}

export function deleteWordBackward(element: TextField) {
  if (deleteSelection(element)) return;

  const start = element.selectionStart ?? 0;
  const newPos = findWordBoundary(element.value, start, "backward");
  applyDeletion(
    element,
    element.value.slice(0, newPos) + element.value.slice(start),
    newPos,
    "deleteWordBackward",
  );
}

export function deleteWordLeft(el: TextField) {
  if (deleteSelection(el)) return;

  // Caret only: delete the word to the left (skipping whitespace first).
  const start = el.selectionStart ?? 0;
  const value = el.value;
  let i = start;
  while (i > 0 && /\s/.test(value[i - 1])) i--;
  while (i > 0 && /[\p{L}\p{N}_]/u.test(value[i - 1])) i--;

  applyDeletion(
    el,
    value.slice(0, i) + value.slice(start),
    i,
    "deleteWordBackward",
  );
}

export function deleteLineBackward(element: TextField) {
  const end = element.selectionEnd ?? 0;
  applyDeletion(element, element.value.slice(end), 0, "deleteSoftLineBackward");
}

export function selectAll(context: FocusContext) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;

  if (
    isInNormalInputField &&
    (activeEl instanceof HTMLInputElement ||
      activeEl instanceof HTMLTextAreaElement)
  ) {
    activeEl.select();
  } else if (isEditorFocused) {
    const editor = get(editorInstance);
    if (!editor) return;
    const model = editor.getModel();
    if (model) {
      const lastLine = model.getLineCount();
      // A plain range: no Monaco import needed on the initial path.
      editor.setSelection({
        startLineNumber: 1,
        startColumn: 1,
        endLineNumber: lastLine,
        endColumn: model.getLineMaxColumn(lastLine),
      });
    }
  }
}
