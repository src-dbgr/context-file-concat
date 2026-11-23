import { editorInstance } from "../stores/app.js";
import { getUndoManagerForElement } from "./undo.js";
import type { FocusContext } from "../types.js";
import { get } from "svelte/store";
import { toast } from "../stores/toast.js";
import { flashCopyFeedback } from "../stores/preview.js";
import { unchangedOutput } from "./editor.js";
import { translate } from "../i18n/index.js";

async function readFromClipboardWithFallback(): Promise<string | null> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    console.warn("Clipboard read API failed.");
    return prompt(translate("clipboard.pastePrompt"));
  }
}

async function copyWithFallback(content: string): Promise<boolean> {
  if (!content) return false;
  try {
    await navigator.clipboard.writeText(content);
    return true;
  } catch {
    console.warn("Clipboard API failed, trying legacy fallback.");
    const textArea = document.createElement("textarea");
    textArea.value = content;
    textArea.style.position = "fixed";
    textArea.style.left = "-9999px";
    document.body.appendChild(textArea);
    textArea.focus();
    textArea.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      document.body.removeChild(textArea);
    }
  }
}

export async function handleCopy(context: FocusContext) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  let textToCopy = "";
  let statusMessage = translate("clipboard.copyFailed");
  const statusEl = document.querySelector(".status-text");

  if (
    isInNormalInputField &&
    (activeEl instanceof HTMLInputElement ||
      activeEl instanceof HTMLTextAreaElement)
  ) {
    const selection = activeEl.value.substring(
      activeEl.selectionStart ?? 0,
      activeEl.selectionEnd ?? 0,
    );
    textToCopy = selection || activeEl.value;
    if (textToCopy) statusMessage = translate("clipboard.copiedInput");
  } else if (isEditorFocused) {
    const editor = get(editorInstance);
    if (!editor) return;

    const model = editor.getModel();
    if (!model) return;

    const selection = editor.getSelection();
    const hasSelection = !!selection && !selection.isEmpty();
    // A generated output the user has not changed is copied as the host
    // produced it (the text a save writes), not as the editor holds it:
    // the editor gives every line the same line end.
    const original = unchangedOutput();
    const copiesEverything = () =>
      !hasSelection || selection.equalsRange(model.getFullModelRange());

    if (original !== null && copiesEverything()) {
      textToCopy = original;
      statusMessage = translate("clipboard.copiedEditorAll");
    } else if (hasSelection) {
      textToCopy = model.getValueInRange(selection);
      statusMessage = translate("clipboard.copiedEditorSelection");
    } else {
      textToCopy = model.getValue();
      statusMessage = translate("clipboard.copiedEditorAll");
    }
  }

  const success = await copyWithFallback(textToCopy);

  if (statusEl) {
    statusEl.textContent = success
      ? statusMessage
      : translate("clipboard.copyFailed");
  }

  // Toast feedback (non-intrusive, timed)
  if (success) {
    toast.success(translate("toast.copied"));
  } else {
    toast.error(translate("toast.copy_failed"));
  }

  // Visual feedback on the Copy button (rendered by PreviewPanel)
  if (isEditorFocused) flashCopyFeedback(success ? "copied" : "failed");
}

export async function handlePaste(context: FocusContext) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  const statusEl = document.querySelector(".status-text");
  if (!statusEl) return;

  if (
    isInNormalInputField &&
    (activeEl instanceof HTMLInputElement ||
      activeEl instanceof HTMLTextAreaElement)
  ) {
    getUndoManagerForElement(activeEl).recordState(true);
    activeEl.focus();

    const text = await readFromClipboardWithFallback();
    if (text) {
      document.execCommand("insertText", false, text);
      statusEl.textContent = translate("clipboard.pasted");
      toast.success(translate("toast.pasted"));
      activeEl.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      statusEl.textContent = translate("clipboard.pasteFailed");
      toast.warning(translate("toast.paste_empty"));
    }
  } else if (isEditorFocused) {
    const clipboardText = await readFromClipboardWithFallback();

    if (clipboardText === null) {
      statusEl.textContent = translate("clipboard.pasteCancelled");
      toast.info(translate("toast.paste_cancelled"));
      return;
    }
    if (!clipboardText) {
      statusEl.textContent = translate("clipboard.pasteEmpty");
      toast.warning(translate("toast.paste_empty"));
      return;
    }

    const editor = get(editorInstance);
    if (!editor) return;
    const selection = editor.getSelection();
    if (selection) {
      editor.executeEdits("paste", [{ range: selection, text: clipboardText }]);
      statusEl.textContent = translate("clipboard.pasted");
      toast.success(translate("toast.pasted_editor"));
    }
  } else {
    statusEl.textContent = translate("clipboard.pasteUnsupported");
    toast.error(translate("toast.paste_unsupported"));
  }
}

export async function handleCut(context: FocusContext) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  let success = false;
  const statusEl = document.querySelector(".status-text");
  if (!statusEl) return;

  if (
    isInNormalInputField &&
    (activeEl instanceof HTMLInputElement ||
      activeEl instanceof HTMLTextAreaElement)
  ) {
    getUndoManagerForElement(activeEl).recordState(true);
    const textToCopy = activeEl.value.substring(
      activeEl.selectionStart ?? 0,
      activeEl.selectionEnd ?? 0,
    );
    const copied = await copyWithFallback(textToCopy);
    if (copied) {
      document.execCommand("delete");
      success = true;
    }
  } else if (isEditorFocused) {
    const editor = get(editorInstance);
    if (!editor) return;
    const model = editor.getModel();
    if (!model) return;
    const selection = editor.getSelection();

    if (selection && !selection.isEmpty()) {
      const textToCopy = model.getValueInRange(selection);
      const copied = await copyWithFallback(textToCopy);
      if (copied) {
        editor.executeEdits("cut", [{ range: selection, text: "" }]);
        success = true;
      }
    }
  }

  statusEl.textContent = translate(
    success ? "clipboard.cutOk" : "clipboard.cutFailed",
  );
  if (success) toast.success(translate("toast.cut_ok"));
  else toast.error(translate("toast.cut_failed"));
}
