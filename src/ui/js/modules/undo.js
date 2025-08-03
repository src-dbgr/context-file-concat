/**
 * @typedef {object} EditorState
 * @property {string} value
 * @property {number} selectionStart
 * @property {number} selectionEnd
 */

const COALESCE_TIMEOUT = 500; // ms to group text changes

/**
 * Manages the undo/redo history for a single text input element.
 */
class UndoManager {
  constructor(element) {
    this.element = element;
    /** @type {EditorState[]} */
    this.undoStack = [];
    /** @type {EditorState[]} */
    this.redoStack = [];
    this.coalesceTimeoutId = null;

    // Start with the initial state
    this.recordState(true);
  }

  /**
   * Records the current state of the input element onto the undo stack.
   * @param {boolean} force - If true, records immediately. Otherwise, coalesces rapid changes.
   */
  recordState(force = false) {
    if (this.coalesceTimeoutId) {
      clearTimeout(this.coalesceTimeoutId);
      this.coalesceTimeoutId = null;
    }

    const record = () => {
      const currentState = this.getCurrentState();
      const lastState = this.undoStack[this.undoStack.length - 1];

      // Don't record if nothing has changed
      if (
        lastState &&
        lastState.value === currentState.value &&
        lastState.selectionStart === currentState.selectionStart
      ) {
        return;
      }

      this.undoStack.push(currentState);
      // A new action clears the redo stack
      this.redoStack = [];
    };

    if (force) {
      record();
    } else {
      // Coalesce typing changes to avoid one undo step per character
      this.coalesceTimeoutId = setTimeout(record, COALESCE_TIMEOUT);
    }
  }

  /**
   * Reverts the element to the previous state in the history.
   */
  undo() {
    if (this.undoStack.length <= 1) return; // Keep the initial state

    const currentState = this.undoStack.pop();
    this.redoStack.push(currentState);

    const stateToRestore = this.undoStack[this.undoStack.length - 1];
    this.applyState(stateToRestore);
  }

  /**
   * Re-applies a state that was undone.
   */
  redo() {
    if (this.redoStack.length === 0) return;

    const stateToRestore = this.redoStack.pop();
    this.undoStack.push(stateToRestore);
    this.applyState(stateToRestore);
  }

  /**
   * Gets the current state of the associated element.
   * @returns {EditorState}
   */
  getCurrentState() {
    return {
      value: this.element.value,
      selectionStart: this.element.selectionStart,
      selectionEnd: this.element.selectionEnd,
    };
  }

  /**
   * Applies a given state to the element.
   * @param {EditorState} state
   */
  applyState(state) {
    this.element.value = state.value;
    this.element.setSelectionRange(state.selectionStart, state.selectionEnd);
    // Dispatch event so any UI frameworks can react
    this.element.dispatchEvent(new Event("input", { bubbles: true }));
  }
}

/** @type {Map<HTMLElement, UndoManager>} */
const elementUndoManagers = new Map();

/**
 * Gets or creates an UndoManager for a given HTML element.
 * @param {HTMLElement} element
 * @returns {UndoManager}
 */
export function getUndoManagerForElement(element) {
  if (!elementUndoManagers.has(element)) {
    elementUndoManagers.set(element, new UndoManager(element));
  }
  return elementUndoManagers.get(element);
}
