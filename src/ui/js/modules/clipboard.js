/* global monaco */
import { state } from '../state.js';
import { elements } from '../dom.js';

function insertTextIntoElement(element, text, triggerMonacoEvents = false) {
  if (!element) return;
  const start = element.selectionStart || 0;
  const end = element.selectionEnd || 0;
  element.value = element.value.slice(0, start) + text + element.value.slice(end);
  element.selectionStart = element.selectionEnd = start + text.length;

  ["input", "change"].forEach((eventType) => {
    try {
      element.dispatchEvent(new Event(eventType, { bubbles: true }));
    } catch (e) { /* ignore */ }
  });

  if (triggerMonacoEvents) {
    try {
      element.dispatchEvent(new InputEvent("beforeinput", { bubbles: true, data: text, inputType: "insertText" }));
    } catch (e) { /* ignore */ }
  }
  element.focus();
}

async function copyWithFallback(content) {
    try {
        await navigator.clipboard.writeText(content);
        return true;
    } catch (err) {
        console.warn("Clipboard API failed, trying fallback.", err);
        const textArea = document.createElement("textarea");
        textArea.value = content;
        textArea.style.position = "fixed";
        textArea.style.left = "-9999px";
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();
        try {
            const successful = document.execCommand("copy");
            return successful;
        } catch (e) {
            return false;
        } finally {
            document.body.removeChild(textArea);
        }
    }
}

export async function copyToClipboard() {
  const editor = state.getEditor();
  if (!editor || !editor.getModel()) {
    document.querySelector(".status-text").textContent = "Error: No content to copy.";
    return;
  }

  const contentToCopy = editor.getModel().getValue();
  if (!contentToCopy) {
      document.querySelector(".status-text").textContent = "Error: No content to copy.";
      return;
  }
  
  const success = await copyWithFallback(contentToCopy);
  const statusText = document.querySelector(".status-text");
  
  if (success) {
      statusText.textContent = `✓ Complete file copied to clipboard! (${contentToCopy.length} characters)`;
      elements.copyBtn.innerHTML = `... Copied!`;
      elements.copyBtn.style.backgroundColor = "#22C55E";
  } else {
      statusText.textContent = "✗ Failed to copy to clipboard.";
      elements.copyBtn.innerHTML = `... Failed`;
      elements.copyBtn.style.backgroundColor = "#EF4444";
  }

  setTimeout(() => {
    elements.copyBtn.innerHTML = `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg> Copy`;
    elements.copyBtn.style.backgroundColor = "";
  }, 2000);
}

export async function copySelectedTextToClipboard() {
    const editor = state.getEditor();
    if (!editor) return;
    const selection = editor.getSelection();
    if (!selection || selection.isEmpty()) {
        document.querySelector(".status-text").textContent = "No text selected.";
        return;
    }

    const selectedText = editor.getModel().getValueInRange(selection);
    if (!selectedText) return;

    const success = await copyWithFallback(selectedText);
    const lines = selectedText.split('\n').length;
    document.querySelector(".status-text").textContent = success
        ? `✓ Selected text copied! (${selectedText.length} chars, ${lines} lines)`
        : "✗ Failed to copy selected text.";
}

function getClipboardViaLegacy() {
    return new Promise((resolve, reject) => {
        const tempTextarea = document.createElement("textarea");
        tempTextarea.style.position = "fixed";
        tempTextarea.style.left = "-9999px";
        document.body.appendChild(tempTextarea);
        tempTextarea.focus();
        tempTextarea.select();
        try {
            const success = document.execCommand("paste");
            const value = tempTextarea.value;
            if (success && value) resolve(value);
            else reject(new Error("Legacy clipboard read failed"));
        } catch (e) {
            reject(e);
        } finally {
            document.body.removeChild(tempTextarea);
        }
    });
}

export async function handleSafePaste(isInMonacoFindWidget, isEditorFocused) {
    const activeEl = document.activeElement;
    try {
        const clipboardText = await getClipboardViaLegacy();
        if (!clipboardText) {
            document.querySelector(".status-text").textContent = "Clipboard is empty.";
            return;
        }

        const isInNormalInputField = activeEl && (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") && !activeEl.closest(".monaco-editor");

        if (isInNormalInputField) {
            insertTextIntoElement(activeEl, clipboardText);
        } else if (isInMonacoFindWidget) {
            insertTextIntoElement(activeEl, clipboardText, true);
        } else if (isEditorFocused) {
            const editor = state.getEditor();
            const selection = editor.getSelection();
            editor.executeEdits("paste", [{ range: selection, text: clipboardText }]);
        } else {
            document.querySelector(".status-text").textContent = "✗ Paste not supported here.";
        }
    } catch (error) {
        const userText = prompt("Clipboard access failed. Please paste your text here:");
        if (userText && activeEl) insertTextIntoElement(activeEl, userText);
    }
}
