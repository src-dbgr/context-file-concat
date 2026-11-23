/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach } from "vitest";
import { commands } from "$lib/modules/commands";
import { getUndoManagerForElement } from "$lib/modules/undo";
import type { FocusContext } from "$lib/types";

function makeInput(value: string): HTMLInputElement {
  const el = document.createElement("input");
  el.value = value;
  document.body.appendChild(el);
  el.focus();
  return el;
}

function makeContextFor(el: HTMLElement): FocusContext {
  return {
    activeEl: el,
    isEditorFocused: false,
    isInNormalInputField: true,
  };
}

function findCommandByEvent(e: KeyboardEvent) {
  return commands.find((c) => c.matches(e));
}

describe("commands.ts – keyboard command matching & execution (inputs)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("Cmd/Ctrl+A selects all text in a standard input", () => {
    const el = makeInput("hello world");
    el.setSelectionRange(0, 0);

    const evt = new KeyboardEvent("keydown", {
      key: "a",
      metaKey: true,
    });
    const cmd = findCommandByEvent(evt);
    expect(cmd).toBeTruthy();

    cmd!.execute(evt, makeContextFor(el));

    expect(el.selectionStart).toBe(0);
    expect(el.selectionEnd).toBe(el.value.length);
  });

  it("Cmd/Ctrl+Z performs undo using the UndoManager", () => {
    const el = makeInput("a");
    const um = getUndoManagerForElement(el);

    el.value = "ab";
    // Force a snapshot synchronously (no timers)
    um.recordState(true);

    const evt = new KeyboardEvent("keydown", {
      key: "z",
      metaKey: true,
    });
    const cmd = findCommandByEvent(evt);
    expect(cmd).toBeTruthy();

    cmd!.execute(evt, makeContextFor(el));
    expect(el.value).toBe("a");
  });

  it("Cmd/Ctrl+Shift+Z performs redo", () => {
    const el = makeInput("start");
    const um = getUndoManagerForElement(el);

    el.value = "mid";
    um.recordState(true);

    // Undo once
    {
      const evtUndo = new KeyboardEvent("keydown", { key: "z", metaKey: true });
      const cmdUndo = findCommandByEvent(evtUndo)!;
      cmdUndo.execute(evtUndo, makeContextFor(el));
      expect(el.value).toBe("start");
    }

    // Redo via Shift+Z
    {
      const evtRedo = new KeyboardEvent("keydown", {
        key: "z",
        metaKey: true,
        shiftKey: true,
      });
      const cmdRedo = findCommandByEvent(evtRedo)!;
      cmdRedo.execute(evtRedo, makeContextFor(el));
      expect(el.value).toBe("mid");
    }
  });

  it("Alt+Backspace deletes a word backward", () => {
    const el = makeInput("foo bar baz");
    el.setSelectionRange(el.value.length, el.value.length);

    const evt = new KeyboardEvent("keydown", {
      key: "Backspace",
      altKey: true,
    });
    const cmd = findCommandByEvent(evt);
    expect(cmd).toBeTruthy();

    cmd!.execute(evt, makeContextFor(el));
    // Implementation keeps the trailing space after deleting the last word
    expect(el.value).toBe("foo bar ");
  });

  describe("deleting with the keyboard tells the field's listeners", () => {
    function inputEvents(el: HTMLInputElement): InputEvent[] {
      const events: InputEvent[] = [];
      // On the document: the event has to bubble, as a native one does.
      document.addEventListener("input", (e) => {
        if (e.target === el) events.push(e as InputEvent);
      });
      return events;
    }

    function press(el: HTMLInputElement, init: KeyboardEventInit) {
      const evt = new KeyboardEvent("keydown", init);
      const cmd = findCommandByEvent(evt);
      expect(cmd).toBeTruthy();
      cmd!.execute(evt, makeContextFor(el));
    }

    it("Alt+Backspace dispatches one input event (deleteWordBackward)", () => {
      const el = makeInput("foo bar baz");
      el.setSelectionRange(11, 11);
      const events = inputEvents(el);

      press(el, { key: "Backspace", altKey: true });

      expect(el.value).toBe("foo bar ");
      expect(events).toHaveLength(1);
      expect(events[0]).toBeInstanceOf(InputEvent);
      expect(events[0].inputType).toBe("deleteWordBackward");
      expect(events[0].bubbles).toBe(true);
      expect(el.selectionStart).toBe(8);
      expect(el.selectionEnd).toBe(8);
    });

    it("Ctrl+Backspace does the same", () => {
      const el = makeInput("foo bar");
      el.setSelectionRange(7, 7);
      const events = inputEvents(el);

      press(el, { key: "Backspace", ctrlKey: true });

      expect(el.value).toBe("foo ");
      expect(events.map((e) => e.inputType)).toEqual(["deleteWordBackward"]);
    });

    it("Alt+Backspace with a selection removes the selection (deleteContentBackward)", () => {
      const el = makeInput("foo bar baz");
      el.setSelectionRange(4, 8);
      const events = inputEvents(el);

      press(el, { key: "Backspace", altKey: true });

      expect(el.value).toBe("foo baz");
      expect(el.selectionStart).toBe(4);
      expect(events.map((e) => e.inputType)).toEqual(["deleteContentBackward"]);
    });

    it("Cmd+Backspace deletes to the start and dispatches deleteSoftLineBackward", () => {
      const el = makeInput("foo bar baz");
      el.setSelectionRange(8, 8);
      const events = inputEvents(el);

      press(el, { key: "Backspace", metaKey: true });

      expect(el.value).toBe("baz");
      expect(el.selectionStart).toBe(0);
      expect(el.selectionEnd).toBe(0);
      expect(events.map((e) => e.inputType)).toEqual([
        "deleteSoftLineBackward",
      ]);
    });

    it("dispatches nothing when there is nothing to delete", () => {
      const el = makeInput("foo");
      el.setSelectionRange(0, 0);
      const events = inputEvents(el);

      press(el, { key: "Backspace", altKey: true });
      press(el, { key: "Backspace", metaKey: true });

      expect(el.value).toBe("foo");
      expect(events).toEqual([]);
    });
  });

  it("Alt+ArrowLeft/Right move by word; with Shift they select by word", () => {
    const el = makeInput("alpha beta");
    // Place caret after 'alpha ' (index 6)
    el.setSelectionRange(6, 6);

    // Move backward one word (to start of 'alpha')
    {
      const evt = new KeyboardEvent("keydown", {
        key: "ArrowLeft",
        altKey: true,
      });
      const cmd = findCommandByEvent(evt)!;
      cmd.execute(evt, makeContextFor(el));
      expect(el.selectionStart).toBe(0);
      expect(el.selectionEnd).toBe(0);
    }

    // Move forward selecting the next word
    {
      const evt = new KeyboardEvent("keydown", {
        key: "ArrowRight",
        altKey: true,
        shiftKey: true,
      });
      const cmd = findCommandByEvent(evt)!;
      cmd.execute(evt, makeContextFor(el));
      // Selection includes the trailing space of the word boundary per implementation
      expect(el.selectionStart).toBe(0);
      expect(el.selectionEnd).toBe(6);
    }
  });
});
