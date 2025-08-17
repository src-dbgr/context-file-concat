import { commands as allCommands } from "./commands.js";
import type { FocusContext } from "../types.js";

function getFocusContext(): FocusContext {
  const activeEl = document.activeElement as HTMLElement | null;
  const isEditorFocused = !!(activeEl && activeEl.closest(".monaco-editor"));
  const isInNormalInputField = !!(
    activeEl &&
    (activeEl.tagName === "INPUT" || activeEl.tagName === "TEXTAREA") &&
    !isEditorFocused
  );
  return { activeEl, isEditorFocused, isInNormalInputField };
}

function globalKeydownHandler(e: KeyboardEvent) {
  const context = getFocusContext();

  const command = allCommands.find((cmd) => {
    if (context.isEditorFocused && !cmd.worksInEditor) {
      return false;
    }
    const worksInCurrentContext =
      context.isEditorFocused || context.isInNormalInputField;
    return worksInCurrentContext && cmd.matches(e);
  });

  if (command) {
    e.preventDefault();
    command.execute(e, context);
    return;
  }

  if (context.isEditorFocused) {
    return;
  }

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
    const isPrintableChar = e.key.length === 1 && !e.ctrlKey && !e.metaKey;
    if (SAFE_KEYS_ALLOWLIST.includes(e.key) || isPrintableChar) {
      return;
    }
  }

  if (
    !e.metaKey &&
    !e.ctrlKey &&
    !e.altKey &&
    e.key.length > 1 &&
    e.key !== "Shift"
  ) {
    console.warn(
      `Blocked unhandled key: "${e.key}" to prevent potential crash.`
    );
    e.preventDefault();
  }
}

export function setupGlobalKeyboardListeners() {
  document.addEventListener("keydown", globalKeydownHandler, true);
}
