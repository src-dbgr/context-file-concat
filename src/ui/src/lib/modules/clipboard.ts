import { editorInstance } from "../stores/app.js";
import { getUndoManagerForElement } from "./undo.js";
import { elements } from "../dom.js";
import type { FocusContext } from "../types.js";
import { get } from "svelte/store";
import { toast } from "../stores/toast.js";

async function readFromClipboardWithFallback(): Promise<string | null> {
  try {
    return await navigator.clipboard.readText();
  } catch {
    console.warn("Clipboard read API failed.");
    return prompt("Could not access clipboard. Please paste your text here:");
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
  let statusMessage = "✗ Failed to copy.";
  const statusEl = document.querySelector(".status-text");

  if (
    isInNormalInputField &&
    (activeEl instanceof HTMLInputElement ||
      activeEl instanceof HTMLTextAreaElement)
  ) {
    const selection = activeEl.value.substring(
      activeEl.selectionStart ?? 0,
      activeEl.selectionEnd ?? 0
    );
    textToCopy = selection || activeEl.value;
    if (textToCopy) statusMessage = `✓ Copied text from input field.`;
  } else if (isEditorFocused) {
    const editor = get(editorInstance);
    if (!editor) return;

    const model = editor.getModel();
    if (!model) return;

    const selection = editor.getSelection();
    if (selection && !selection.isEmpty()) {
      textToCopy = model.getValueInRange(selection);
      statusMessage = `✓ Copied selected text from editor.`;
    } else {
      textToCopy = model.getValue();
      statusMessage = `✓ Copied entire editor content.`;
    }
  }

  const success = await copyWithFallback(textToCopy);

  if (statusEl) {
    statusEl.textContent = success
      ? statusMessage
      : "✗ Failed to copy to clipboard.";
  }

  // Toast feedback (non-intrusive, timed)
  if (success) {
    toast.success("Copied to clipboard");
  } else {
    toast.error("Failed to copy to clipboard");
  }

  if (isEditorFocused && elements.copyBtn) {
    if (success) {
      elements.copyBtn.innerHTML = `... Copied!`;
      elements.copyBtn.style.backgroundColor = "#4caf50";
      elements.copyBtn.style.color = "#d4d4d4";
    } else {
      elements.copyBtn.innerHTML = `... Failed`;
      elements.copyBtn.style.backgroundColor = "#e54b4b";
    }

    setTimeout(() => {
      elements.copyBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg> Copy`;
      elements.copyBtn.style.backgroundColor = "";
      elements.copyBtn.style.color = "#d4d4d4";
    }, 1000);
  }
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
      statusEl.textContent = `✓ Content pasted.`;
      toast.success("Pasted content");
      activeEl.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      statusEl.textContent = `✗ Paste failed or clipboard empty.`;
      toast.warning("Clipboard is empty");
    }
  } else if (isEditorFocused) {
    const clipboardText = await readFromClipboardWithFallback();

    if (clipboardText === null) {
      statusEl.textContent = "Status: Paste cancelled.";
      toast.info("Paste cancelled");
      return;
    }
    if (!clipboardText) {
      statusEl.textContent = "Status: Clipboard is empty.";
      toast.warning("Clipboard is empty");
      return;
    }

    const editor = get(editorInstance);
    if (!editor) return;
    const selection = editor.getSelection();
    if (selection) {
      editor.executeEdits("paste", [{ range: selection, text: clipboardText }]);
      statusEl.textContent = `✓ Content pasted.`;
      toast.success("Pasted into editor");
    }
  } else {
    statusEl.textContent = "✗ Paste not supported here.";
    toast.error("Paste not supported here");
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
      activeEl.selectionEnd ?? 0
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

  statusEl.textContent = success ? `✓ Text cut to clipboard.` : `✗ Cut failed.`;
  if (success) toast.success("Cut to clipboard");
  else toast.error("Cut failed");
}
