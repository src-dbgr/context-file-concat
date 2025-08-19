import { editorInstance } from "../stores/app.js";
import type { FocusContext } from "../types.js";
import { get } from "svelte/store";
import * as monaco from "monaco-editor";

function findWordBoundary(
  value: string,
  position: number,
  direction: "forward" | "backward"
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
  direction: "forward" | "backward"
) {
  const newPos = findWordBoundary(
    element.value,
    element.selectionStart ?? 0,
    direction
  );
  element.setSelectionRange(newPos, newPos);
}

export function selectWord(
  element: HTMLInputElement | HTMLTextAreaElement,
  direction: "forward" | "backward"
) {
  if (direction === "forward") {
    const newPos = findWordBoundary(
      element.value,
      element.selectionEnd ?? 0,
      "forward"
    );
    element.setSelectionRange(element.selectionStart, newPos);
  } else {
    const newPos = findWordBoundary(
      element.value,
      element.selectionStart ?? 0,
      "backward"
    );
    element.setSelectionRange(newPos, element.selectionEnd);
  }
}

export function deleteWordBackward(
  element: HTMLInputElement | HTMLTextAreaElement
) {
  const start = element.selectionStart ?? 0;
  const end = element.selectionEnd ?? 0;

  if (start !== end) {
    element.value = element.value.slice(0, start) + element.value.slice(end);
    element.setSelectionRange(start, start);
  } else {
    const newPos = findWordBoundary(element.value, start, "backward");
    element.value = element.value.slice(0, newPos) + element.value.slice(start);
    element.setSelectionRange(newPos, newPos);
  }
}

export function deleteLineBackward(
  element: HTMLInputElement | HTMLTextAreaElement
) {
  const end = element.selectionEnd ?? 0;
  element.value = element.value.slice(end);
  element.setSelectionRange(0, 0);
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
      editor.setSelection(
        new monaco.Selection(
          1,
          1,
          model.getLineCount(),
          model.getLineMaxColumn(model.getLineCount())
        )
      );
    }
  }
}
