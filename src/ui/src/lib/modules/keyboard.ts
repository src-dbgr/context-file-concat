import { commands as allCommands } from "./commands.js";

/**
 * Gathers information about the current focus context within the application.
 * This helps determine which keyboard shortcuts should be active.
 * @returns {{
 * activeEl: HTMLElement | null,
 * isEditorFocused: boolean,
 * isInNormalInputField: boolean
 * }}
 */
function getFocusContext() {
  const activeEl = document.activeElement;
  const isEditorFocused = !!(activeEl && activeEl.closest(".monaco-editor"));
  const isInNormalInputField = !!(
    activeEl &&
    (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") &&
    !isEditorFocused
  );
  return { activeEl, isEditorFocused, isInNormalInputField };
}

/**
 * The single, global keydown handler that intercepts and dispatches all commands.
 * It implements a "Zero Trust" policy: any key event that is not explicitly
 * defined as a command or known to be safe for the current context will be
 * blocked by default to prevent unexpected browser behavior or crashes (e.g., from an unhandled Escape key).
 * @param {KeyboardEvent} e The keyboard event.
 */
function globalKeydownHandler(e) {
  const context = getFocusContext();

  // Find the first command that matches the keyboard event and the current focus context.
  const command = allCommands.find((cmd) => {
    // A command is only considered if it's applicable in the current context.
    if (context.isEditorFocused && !cmd.worksInEditor) {
      return false;
    }
    const worksInCurrentContext =
      context.isEditorFocused || context.isInNormalInputField;

    return worksInCurrentContext && cmd.matches(e);
  });

  if (command) {
    // If an explicit command is found, execute it and prevent the default browser action.
    e.preventDefault();
    command.execute(e, context);
    return; // Command executed, our job is done.
  }

  // --- CATCH-ALL SAFETY NET ---
  // If no specific command was found, apply default safety rules.

  // If the editor has focus, we trust it completely with unhandled keys.
  if (context.isEditorFocused) {
    return;
  }

  // If a normal input has focus, allow only safe, expected typing and navigation keys.
  if (context.isInNormalInputField) {
    const SAFE_KEYS_ALLOWLIST = [
      "ArrowUp",
      "ArrowDown",
      "ArrowLeft",
      "ArrowRight",
      "Backspace",
      "Delete",
      "Tab",
      "Home",
      "End",
      "Enter",
    ];

    // A printable character (e.g., 'a', 'Z', '7', '$') without Ctrl/Meta modifiers.
    const isPrintableChar = e.key.length === 1 && !e.ctrlKey && !e.metaKey;

    if (SAFE_KEYS_ALLOWLIST.includes(e.key) || isPrintableChar) {
      // This is a normal typing or navigation key; let the browser handle it.
      return;
    }
  }

  // If we reach this point, the event is unhandled and not in a safe context.
  // We block it to prevent potential crashes or unwanted browser actions.
  console.warn(`Blocked unhandled key: "${e.key}" to prevent potential crash.`);
  e.preventDefault();
}

/**
 * Attaches the global keyboard listener to the document.
 */
export function setupGlobalKeyboardListeners() {
  // We use a single, global listener in the "capture" phase to handle all key events robustly.
  document.addEventListener("keydown", globalKeydownHandler, true);
}
