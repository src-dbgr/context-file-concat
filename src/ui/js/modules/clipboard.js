import { state } from "../state.js";
import { getUndoManagerForElement } from "./undo.js";
import { elements } from "../dom.js";

async function readFromClipboardWithFallback() {
  try {
    return await navigator.clipboard.readText();
  } catch (err) {
    console.warn("Clipboard read API failed.", err);
    return prompt("Could not access clipboard. Please paste your text here:");
  }
}

async function copyWithFallback(content) {
  if (!content) return false;
  try {
    await navigator.clipboard.writeText(content);
    return true;
  } catch (err) {
    console.warn("Clipboard API failed, trying legacy fallback.", err);
    const textArea = document.createElement("textarea");
    textArea.value = content;
    textArea.style.position = "fixed";
    textArea.style.left = "-9999px";
    document.body.appendChild(textArea);
    textArea.focus();
    textArea.select();
    try {
      return document.execCommand("copy");
    } catch (e) {
      return false;
    } finally {
      document.body.removeChild(textArea);
    }
  }
}

export async function handleCopy(context) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  let textToCopy = "";
  let statusMessage = "✗ Failed to copy.";

  if (isInNormalInputField) {
    const selection = activeEl.value.substring(
      activeEl.selectionStart,
      activeEl.selectionEnd
    );
    textToCopy = selection || activeEl.value;
    if (textToCopy) statusMessage = `✓ Copied text from input field.`;
  } else if (isEditorFocused) {
    const editor = state.getEditor();
    const model = editor.getModel();
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

  // Update the main status bar at the bottom
  document.querySelector(".status-text").textContent = success
    ? statusMessage
    : "✗ Failed to copy to clipboard.";

  // Provide visual feedback directly on the copy button itself.
  // This logic is specific to the button click context (isEditorFocused).
  if (isEditorFocused && elements.copyBtn) {
    if (success) {
      elements.copyBtn.innerHTML = `... Copied!`;
      elements.copyBtn.style.backgroundColor = "#4caf50"; // Success green
      elements.copyBtn.style.color = "#2c2e33"; // Success green
    } else {
      elements.copyBtn.innerHTML = `... Failed`;
      elements.copyBtn.style.backgroundColor = "#e54b4b"; // Error red
    }

    // Reset the button after 2 seconds
    setTimeout(() => {
      elements.copyBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg> Copy`;
      elements.copyBtn.style.backgroundColor = "";
      elements.copyBtn.style.color = "#d4d4d4";
    }, 1000);
  }
}

export async function handlePaste(context) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  if (isInNormalInputField)
    getUndoManagerForElement(activeEl).recordState(true);

  if (isInNormalInputField) {
    activeEl.focus();
    const success = document.execCommand("paste");
    if (success) {
      document.querySelector(".status-text").textContent = `✓ Content pasted.`;
      activeEl.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      const text = await readFromClipboardWithFallback();
      if (text) activeEl.value += text;
    }
  } else if (isEditorFocused) {
    const clipboardText = await readFromClipboardWithFallback();

    if (clipboardText === null) {
      document.querySelector(".status-text").textContent =
        "Status: Paste cancelled.";
      return;
    }
    if (!clipboardText) {
      document.querySelector(".status-text").textContent =
        "Status: Clipboard is empty.";
      return;
    }

    const editor = state.getEditor();
    const selection = editor.getSelection();
    editor.executeEdits("paste", [{ range: selection, text: clipboardText }]);
    document.querySelector(".status-text").textContent = `✓ Content pasted.`;
  } else {
    document.querySelector(".status-text").textContent =
      "✗ Paste not supported here.";
  }
}

export async function handleCut(context) {
  const { activeEl, isEditorFocused, isInNormalInputField } = context;
  if (isInNormalInputField)
    getUndoManagerForElement(activeEl).recordState(true);

  let success = false;

  if (isInNormalInputField) {
    activeEl.focus();
    success = document.execCommand("cut");
  } else if (isEditorFocused) {
    const editor = state.getEditor();
    const model = editor.getModel();
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

  document.querySelector(".status-text").textContent = success
    ? `✓ Text cut to clipboard.`
    : `✗ Cut failed.`;
}
