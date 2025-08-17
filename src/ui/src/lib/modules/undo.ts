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

  public recordState(force = false) {
    if (this.coalesceTimeoutId) {
      clearTimeout(this.coalesceTimeoutId);
      this.coalesceTimeoutId = null;
    }

    const record = () => {
      const currentState = this.getCurrentState();
      const lastState = this.undoStack[this.undoStack.length - 1];

      if (
        lastState &&
        lastState.value === currentState.value &&
        lastState.selectionStart === currentState.selectionStart
      ) {
        return;
      }

      this.undoStack.push(currentState);
      this.redoStack = [];
    };

    if (force) {
      record();
    } else {
      this.coalesceTimeoutId = window.setTimeout(record, COALESCE_TIMEOUT);
    }
  }

  public undo() {
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
  element: HTMLInputElement | HTMLTextAreaElement
): UndoManager {
  if (!elementUndoManagers.has(element)) {
    elementUndoManagers.set(element, new UndoManager(element));
  }
  return elementUndoManagers.get(element)!;
}
