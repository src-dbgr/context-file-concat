interface EditorState {
  value: string;
  selectionStart: number;
  selectionEnd: number;
}

const COALESCE_TIMEOUT = 500;

class UndoManager {
  private element: HTMLInputElement | HTMLTextAreaElement;
  private undoStack: EditorState[] = [];
  private redoStack: EditorState[] = [];
  private coalesceTimeoutId: number | null = null;

  constructor(element: HTMLInputElement | HTMLTextAreaElement) {
    this.element = element;
    this.recordState(true);
  }

  /**
   * Records the text of the field as a step that can be undone, at once
   * (`force`) or after the typing has paused.
   *
   * A step is a change of the *text*. A state that differs from the last one
   * in its selection only is no new step and leaves the redo history alone:
   * restoring a state (undo, redo) fires an `input` event like typing does,
   * so it ends up here as well, and whoever re-renders the field in between
   * may have moved the caret. Counting that as an edit dropped the redo
   * history half a second after every undo.
   */
  public recordState(force = false) {
    this.cancelPendingRecord();
    if (force) {
      this.record();
    } else {
      this.coalesceTimeoutId = window.setTimeout(() => {
        this.coalesceTimeoutId = null;
        this.record();
      }, COALESCE_TIMEOUT);
    }
  }

  private record() {
    const currentState = this.getCurrentState();
    const lastState = this.undoStack[this.undoStack.length - 1];

    if (lastState && lastState.value === currentState.value) {
      // Same text: remember where the caret is now, nothing else.
      lastState.selectionStart = currentState.selectionStart;
      lastState.selectionEnd = currentState.selectionEnd;
      return;
    }

    this.undoStack.push(currentState);
    this.redoStack = [];
  }

  private cancelPendingRecord(): boolean {
    if (this.coalesceTimeoutId === null) return false;
    clearTimeout(this.coalesceTimeoutId);
    this.coalesceTimeoutId = null;
    return true;
  }

  /** Typing that has not been recorded yet becomes a step before undo/redo. */
  private flushPendingRecord() {
    if (this.cancelPendingRecord()) this.record();
  }

  public undo() {
    this.flushPendingRecord();
    if (this.undoStack.length <= 1) return;

    const currentState = this.undoStack.pop();
    if (currentState) {
      this.redoStack.push(currentState);
    }

    const stateToRestore = this.undoStack[this.undoStack.length - 1];
    if (stateToRestore) {
      this.applyState(stateToRestore);
    }
  }

  public redo() {
    this.flushPendingRecord();
    const stateToRestore = this.redoStack.pop();
    if (stateToRestore) {
      this.undoStack.push(stateToRestore);
      this.applyState(stateToRestore);
    }
  }

  private getCurrentState(): EditorState {
    return {
      value: this.element.value,
      selectionStart: this.element.selectionStart ?? 0,
      selectionEnd: this.element.selectionEnd ?? 0,
    };
  }

  private applyState(state: EditorState) {
    this.element.value = state.value;
    this.element.setSelectionRange(state.selectionStart, state.selectionEnd);
    this.element.dispatchEvent(new Event("input", { bubbles: true }));
  }
}

const elementUndoManagers = new Map<HTMLElement, UndoManager>();

export function getUndoManagerForElement(
  element: HTMLInputElement | HTMLTextAreaElement,
): UndoManager {
  if (!elementUndoManagers.has(element)) {
    elementUndoManagers.set(element, new UndoManager(element));
  }
  return elementUndoManagers.get(element)!;
}
