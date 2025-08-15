import { state } from "../state.js"; // Import state for Monaco access

const WORD_BREAK_LEFT = /[\s.,;()\[\]{}<>"']|$/;
const WORD_BREAK_RIGHT = /^[\s.,;()\[\]{}<>"']/;

/**
 * Finds the boundary of the next/previous word from a given position.
 * @param {string} value - The text content.
 * @param {number} position - The starting cursor position.
 * @param {'forward' | 'backward'} direction - The direction to search.
 * @returns {number} The new cursor position.
 */
function findWordBoundary(value, position, direction) {
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

/**
 * Moves the cursor word by word.
 * @param {HTMLInputElement|HTMLTextAreaElement} element
 * @param {'forward' | 'backward'} direction
 */
export function moveWord(element, direction) {
  const newPos = findWordBoundary(
    element.value,
    element.selectionStart,
    direction
  );
  element.setSelectionRange(newPos, newPos);
}

/**
 * Selects text word by word.
 * @param {HTMLInputElement|HTMLTextAreaElement} element
 * @param {'forward' | 'backward'} direction
 */
export function selectWord(element, direction) {
  if (direction === "forward") {
    const newPos = findWordBoundary(
      element.value,
      element.selectionEnd,
      "forward"
    );
    element.setSelectionRange(element.selectionStart, newPos);
  } else {
    const newPos = findWordBoundary(
      element.value,
      element.selectionStart,
      "backward"
    );
    element.setSelectionRange(newPos, element.selectionEnd);
  }
}

/**
 * Deletes the word behind the cursor.
 * @param {HTMLInputElement|HTMLTextAreaElement} element
 */
export function deleteWordBackward(element) {
  const start = element.selectionStart;
  const end = element.selectionEnd;

  if (start !== end) {
    element.value = element.value.slice(0, start) + element.value.slice(end);
    element.setSelectionRange(start, start);
  } else {
    const newPos = findWordBoundary(element.value, start, "backward");
    element.value = element.value.slice(0, newPos) + element.value.slice(start);
    element.setSelectionRange(newPos, newPos);
  }
}

/**
 * Deletes from the cursor to the beginning of the line.
 * @param {HTMLInputElement|HTMLTextAreaElement} element
 */
export function deleteLineBackward(element) {
  const start = element.selectionStart;
  const end = element.selectionEnd;
  element.value = element.value.slice(0, 0) + element.value.slice(end);
  element.setSelectionRange(0, 0);
}

/**
 * Selects all text in the currently focused context (input field or editor).
 * @param {object} context
 */
export function selectAll(context) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  if (isInNormalInputField && activeEl.select) {
    activeEl.select();
  } else if (isEditorFocused) {
    const editor = state.getEditor();
    const model = editor.getModel();
    if (editor && model) {
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
