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

export function deleteWordLeft(el: HTMLInputElement | HTMLTextAreaElement) {
  const start = el.selectionStart ?? 0;
  const end = el.selectionEnd ?? 0;
  const value = el.value;

  // Selektion vorhanden → einfach entfernen
  if (start !== end) {
    const min = Math.min(start, end);
    const max = Math.max(start, end);
    el.value = value.slice(0, min) + value.slice(max);
    el.selectionStart = el.selectionEnd = min;
    return;
  }

  // Nur Cursor: Wort links löschen (Whitespaces zuerst überspringen)
  let i = start;
  while (i > 0 && /\s/.test(value[i - 1])) i--;
  while (i > 0 && /[\p{L}\p{N}_]/u.test(value[i - 1])) i--;

  el.value = value.slice(0, i) + value.slice(start);
  el.selectionStart = el.selectionEnd = i;
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
