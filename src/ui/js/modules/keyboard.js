import { state } from "../state.js";
import {
  copyToClipboard,
  copySelectedTextToClipboard,
  handleSafePaste,
} from "./clipboard.js";

function isInMonacoFindWidget() {
  const activeEl = document.activeElement;
  if (!activeEl) return false;
  // A simple but often effective check.
  return activeEl.closest(".find-widget") ? true : false;
}

function handleSelectAll(
  isInMonacoFind,
  isInNormalInputField,
  isEditorFocused
) {
  const activeEl = document.activeElement;
  if (isInMonacoFind && activeEl && activeEl.select) {
    activeEl.select();
  } else if (isInNormalInputField && activeEl && activeEl.select) {
    activeEl.select();
  } else if (isEditorFocused) {
    const editor = state.getEditor();
    const model = editor.getModel();
    if (model) editor.setSelection(model.getFullModelRange());
  }
}

function handleHomeEnd(key, isEditorFocused, isInMonacoFind) {
  if (!isEditorFocused || isInMonacoFind) return;
  const editor = state.getEditor();
  const position = editor.getPosition();
  if (!position) return;

  if (key === "Home") {
    editor.setPosition({ lineNumber: position.lineNumber, column: 1 });
  } else if (key === "End") {
    const lineLength = editor.getModel().getLineLength(position.lineNumber);
    editor.setPosition({
      lineNumber: position.lineNumber,
      column: lineLength + 1,
    });
  }
}

function globalKeydownHandler(e) {
  const editor = state.getEditor();
  if (!editor) return;

  const isFindCommand = (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f";
  const isCopyCommand = (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c";
  const isPasteCommand =
    (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v";
  const isSelectAllCommand =
    (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a";

  const activeEl = document.activeElement;
  const isEditorFocused = activeEl && activeEl.closest(".monaco-editor");
  const isInMonacoFind = isInMonacoFindWidget();
  const isInNormalInputField =
    activeEl &&
    (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") &&
    !isEditorFocused;

  const hasEditorSelection = !editor.getSelection().isEmpty();

  if (isCopyCommand) {
    e.preventDefault();
    if (isEditorFocused && hasEditorSelection && !isInMonacoFind) {
      copySelectedTextToClipboard();
    } else if (isEditorFocused) {
      copyToClipboard();
    }
    return;
  }

  if (isPasteCommand) {
    e.preventDefault();
    handleSafePaste(isInMonacoFind, isEditorFocused);
    return;
  }

  if (isSelectAllCommand) {
    e.preventDefault();
    handleSelectAll(isInMonacoFind, isInNormalInputField, isEditorFocused);
    return;
  }

  // Block other potentially problematic shortcuts, but allow navigation.
  const shouldBlock =
    (e.metaKey || e.ctrlKey) &&
    e.key.length === 1 &&
    !"fcvax".includes(e.key.toLowerCase());

  if (shouldBlock && !isFindCommand) {
    e.preventDefault();
    return;
  }

  if (e.key === "Home" || e.key === "End") {
    if (isEditorFocused && !isInMonacoFind) {
      e.preventDefault();
      handleHomeEnd(e.key, isEditorFocused, isInMonacoFind);
    }
  }
}

export function setupGlobalKeyboardListeners() {
  // We use a single, global listener in the "capture" phase to handle all key events.
  document.addEventListener("keydown", globalKeydownHandler, true);
}
