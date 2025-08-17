import * as actions from "./actions.js";
import * as clipboard from "./clipboard.js";
import { getUndoManagerForElement } from "./undo.js";
import type { FocusContext } from "../types.js";

interface Command {
  matches: (e: KeyboardEvent) => boolean;
  execute: (e: KeyboardEvent, context: FocusContext) => void;
  isUndoable: boolean;
  worksInEditor: boolean;
}

export const commands: Command[] = [
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a",
    execute: (_e, context) => actions.selectAll(context),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c",
    execute: (_e, context) => clipboard.handleCopy(context),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "x",
    execute: (_e, context) => clipboard.handleCut(context),
    isUndoable: true,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v",
    execute: (_e, context) => clipboard.handlePaste(context),
    isUndoable: true,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "z",
    execute: (_e, context) => {
      if (context.activeEl)
        getUndoManagerForElement(
          context.activeEl as HTMLInputElement | HTMLTextAreaElement
        ).undo();
    },
    isUndoable: false,
    worksInEditor: false,
  },
  {
    matches: (e) =>
      (e.metaKey || e.ctrlKey) && e.shiftKey && e.key.toLowerCase() === "z",
    execute: (_e, context) => {
      if (context.activeEl)
        getUndoManagerForElement(
          context.activeEl as HTMLInputElement | HTMLTextAreaElement
        ).redo();
    },
    isUndoable: false,
    worksInEditor: false,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "y",
    execute: (_e, context) => {
      if (context.activeEl)
        getUndoManagerForElement(
          context.activeEl as HTMLInputElement | HTMLTextAreaElement
        ).redo();
    },
    isUndoable: false,
    worksInEditor: false,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && !e.shiftKey && e.key === "ArrowLeft",
    execute: (_e, context) =>
      actions.moveWord(context.activeEl as HTMLInputElement, "backward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && !e.shiftKey && e.key === "ArrowRight",
    execute: (_e, context) =>
      actions.moveWord(context.activeEl as HTMLInputElement, "forward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && e.shiftKey && e.key === "ArrowLeft",
    execute: (_e, context) =>
      actions.selectWord(context.activeEl as HTMLInputElement, "backward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && e.shiftKey && e.key === "ArrowRight",
    execute: (_e, context) =>
      actions.selectWord(context.activeEl as HTMLInputElement, "forward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.altKey || e.ctrlKey) && e.key === "Backspace",
    execute: (_e, context) =>
      actions.deleteWordBackward(context.activeEl as HTMLInputElement),
    isUndoable: true,
    worksInEditor: true,
  },
  {
    matches: (e) => e.metaKey && e.key === "Backspace",
    execute: (_e, context) =>
      actions.deleteLineBackward(context.activeEl as HTMLInputElement),
    isUndoable: true,
    worksInEditor: true,
  },
];
