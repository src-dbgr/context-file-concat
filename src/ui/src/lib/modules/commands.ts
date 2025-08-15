import * as actions from "./actions.js";
import * as clipboard from "./clipboard.js";
import { getUndoManagerForElement } from "./undo.js";

/**
 * @typedef {object} Command
 * @property {(e: KeyboardEvent) => boolean} matches - Determines if the event triggers the command.
 * @property {(e: KeyboardEvent, context: object) => void} execute - The action to perform.
 * @property {boolean} isUndoable - If true, the state before execution will be recorded.
 * @property {boolean} worksInEditor - If true, the command is also active in the Monaco editor.
 */

/** @type {Command[]} */
/** @type {Command[]} */
export const commands = [
  // ### NEUES KOMMANDO ###
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a",
    execute: (e, context) => actions.selectAll(context),
    isUndoable: false,
    worksInEditor: true,
  },
  // Clipboard
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "c",
    execute: (e, context) => clipboard.handleCopy(context),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "x",
    execute: (e, context) => clipboard.handleCut(context),
    isUndoable: true,
    worksInEditor: true,
  },
  {
    matches: (e) => (e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v",
    execute: (e, context) => clipboard.handlePaste(context),
    isUndoable: true,
    worksInEditor: true,
  },
  // Undo/Redo
  {
    matches: (e) =>
      (e.metaKey || e.ctrlKey) && !e.shiftKey && e.key.toLowerCase() === "z",
    execute: (e, context) => getUndoManagerForElement(context.activeEl).undo(),
    isUndoable: false,
    worksInEditor: false, // Monaco has its own undo
  },
  {
    matches: (e) =>
      (e.metaKey || e.ctrlKey) &&
      (e.shiftKey || e.key.toLowerCase() === "y") &&
      e.key.toLowerCase() !== "z",
    execute: (e, context) => getUndoManagerForElement(context.activeEl).redo(),
    isUndoable: false,
    worksInEditor: false,
  },
  // Word-wise Navigation & Selection
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && !e.shiftKey && e.key === "ArrowLeft",
    execute: (e, context) => actions.moveWord(context.activeEl, "backward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && !e.shiftKey && e.key === "ArrowRight",
    execute: (e, context) => actions.moveWord(context.activeEl, "forward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && e.shiftKey && e.key === "ArrowLeft",
    execute: (e, context) => actions.selectWord(context.activeEl, "backward"),
    isUndoable: false,
    worksInEditor: true,
  },
  {
    matches: (e) =>
      (e.altKey || e.ctrlKey) && e.shiftKey && e.key === "ArrowRight",
    execute: (e, context) => actions.selectWord(context.activeEl, "forward"),
    isUndoable: false,
    worksInEditor: true,
  },
  // Deletion
  {
    matches: (e) => (e.altKey || e.ctrlKey) && e.key === "Backspace",
    execute: (e, context) => actions.deleteWordBackward(context.activeEl),
    isUndoable: true,
    worksInEditor: true,
  },
  {
    matches: (e) => e.metaKey && e.key === "Backspace", // macOS style
    execute: (e, context) => actions.deleteLineBackward(context.activeEl),
    isUndoable: true,
    worksInEditor: true,
  },
];
