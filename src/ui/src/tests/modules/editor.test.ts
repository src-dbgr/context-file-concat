/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi, type Mock } from "vitest";

/**
 * The editor module with a fake Monaco: creation of the editor (once, also
 * under concurrent first use) and ownership of the search highlighting.
 * The module keeps state, so every test works on a fresh instance of it.
 */

const loadMonaco = vi.fn();
vi.mock("$lib/modules/monacoLoader", () => ({
  loadMonaco: () => loadMonaco() as unknown,
}));
vi.mock("$lib/services/backend", () => ({ post: vi.fn() }));

type Range = {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
};
type Decoration = {
  range: Range;
  options: { inlineClassName: string; hoverMessage: { value: string } };
};

function makeFakeMonaco() {
  let value = "";
  // Monaco's alternative version id: a new one for every change of the text,
  // the earlier one again when a change is undone.
  let nextVersion = 1;
  let version = nextVersion;
  const undoStack: Array<{ value: string; version: number }> = [];
  const collections: Array<{ set: Mock<(d: Decoration[]) => void> }> = [];

  const model = {
    getAlternativeVersionId: () => version,
    findMatches: vi.fn(
      (term: string, _scope: boolean, _regex: boolean, matchCase: boolean) => {
        const haystack = matchCase ? value : value.toLowerCase();
        const needle = matchCase ? term : term.toLowerCase();
        const matches: Array<{ range: Range }> = [];
        let at = haystack.indexOf(needle);
        while (at !== -1) {
          matches.push({
            range: {
              startLineNumber: 1,
              startColumn: at + 1,
              endLineNumber: 1,
              endColumn: at + 1 + needle.length,
            },
          });
          at = haystack.indexOf(needle, at + needle.length);
        }
        return matches;
      },
    ),
  };

  const editor = {
    setValue: vi.fn((v: string) => {
      value = v;
      version = ++nextVersion;
      undoStack.length = 0;
    }),
    getValue: () => value,
    /** What typing does: a change that can be undone. */
    type: (text: string) => {
      undoStack.push({ value, version });
      value += text;
      version = ++nextVersion;
    },
    undo: () => {
      const previous = undoStack.pop();
      if (previous) ({ value, version } = previous);
    },
    getModel: () => model,
    createDecorationsCollection: vi.fn(() => {
      const collection = { set: vi.fn<(d: Decoration[]) => void>() };
      collections.push(collection);
      return collection;
    }),
    updateOptions: vi.fn(),
    setPosition: vi.fn(),
    revealLine: vi.fn(),
    layout: vi.fn(),
  };

  const ns = {
    editor: {
      create: vi.fn(() => editor),
      setTheme: vi.fn(),
      setModelLanguage: vi.fn(),
    },
  };

  /** The decorations currently shown (the last `set` of the collection). */
  const shown = (): Decoration[] =>
    collections[0]?.set.mock.lastCall?.[0] ?? [];

  return { ns, editor, model, collections, shown };
}

/** Fresh module instances (editor module + the stores it works with). */
async function freshModules() {
  vi.resetModules();
  const editorModule = await import("$lib/modules/editor");
  const app = await import("$lib/stores/app");
  const preview = await import("$lib/stores/preview");
  return { ...editorModule, ...app, ...preview };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("editor module", () => {
  beforeEach(() => {
    loadMonaco.mockReset();
  });

  describe("what a save sends", () => {
    async function withEditor() {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();
      return { fake, m };
    }

    it("is nothing before there is an editor", async () => {
      const { m } = await withEditor();
      expect(m.outputToSave()).toBeNull();
    });

    it("names the generated document instead of sending its text", async () => {
      const { m } = await withEditor();
      await m.showGeneratedContent("generated output", 3, 7);

      expect(m.outputToSave()).toEqual({ kind: "generated", documentId: 7 });
    });

    it("sends the text once the user has edited it", async () => {
      const { fake, m } = await withEditor();
      await m.showGeneratedContent("generated output", 3, 7);
      fake.editor.type(" and a note");

      expect(m.outputToSave()).toEqual({
        kind: "edited",
        content: "generated output and a note",
      });
    });

    it("names the document again when the edits were undone", async () => {
      const { fake, m } = await withEditor();
      await m.showGeneratedContent("generated output", 3, 7);
      fake.editor.type("x");
      fake.editor.type("y");
      fake.editor.undo();
      expect(m.outputToSave()).toMatchObject({ kind: "edited" });
      fake.editor.undo();

      expect(m.outputToSave()).toEqual({ kind: "generated", documentId: 7 });
    });

    it("names the document that is shown, not an earlier one", async () => {
      const { fake, m } = await withEditor();
      await m.showGeneratedContent("first", 1, 7);
      fake.editor.type(" edited");
      await m.showGeneratedContent("second", 1, 8);

      expect(m.outputToSave()).toEqual({ kind: "generated", documentId: 8 });
    });

    it("keeps the text of the host for copying while it is unchanged", async () => {
      const { fake, m } = await withEditor();
      expect(m.unchangedOutput()).toBeNull();

      // Line ends as the files have them; an editor rewrites them.
      const text = "a.txt\r\nline\r\nb.txt\nline\n";
      await m.showGeneratedContent(text, 3, 7);
      expect(m.unchangedOutput()).toBe(text);

      fake.editor.type("x");
      expect(m.unchangedOutput()).toBeNull();
      fake.editor.undo();
      expect(m.unchangedOutput()).toBe(text);

      await m.showGeneratedContent("second", 1, 8);
      expect(m.unchangedOutput()).toBe("second");
      await m.showPreviewContent("file content", "plaintext", "", "/a.txt");
      expect(m.unchangedOutput()).toBeNull();

      await m.showGeneratedContent("third", 1, 9);
      m.clearPreview();
      expect(m.unchangedOutput()).toBeNull();
    });

    it("does not name a document the editor no longer shows", async () => {
      const { m } = await withEditor();
      await m.showGeneratedContent("generated output", 3, 7);
      await m.showPreviewContent("file content", "plaintext", "", "/a.txt");
      expect(m.outputToSave()).toEqual({
        kind: "edited",
        content: "file content",
      });

      await m.showGeneratedContent("generated output", 3, 9);
      m.clearPreview();
      expect(m.outputToSave()).toMatchObject({ kind: "edited" });
    });
  });

  /**
   * A save names the document the editor shows when the button is pressed.
   * The host may have given that document up a moment before; the event that
   * says so reaches the UI *before* the answer to the save does.
   */
  describe("a save that loses a race against the host", () => {
    async function withEditor() {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();
      const backend = await import("$lib/services/backend");
      const post = vi.mocked(backend.post);
      post.mockClear();
      return { fake, m, post };
    }
    // Line ends as the files have them: the editor would rewrite them.
    const shown = "a.txt\r\nline\r\nb.txt\nline\n";

    it("normally names the document and sends no text", async () => {
      const { m, post } = await withEditor();
      await m.showGeneratedContent(shown, 3, 7);

      m.saveOutput();

      expect(post.mock.calls).toEqual([
        ["saveFile", { kind: "generated", documentId: 7 }],
      ]);
    });

    it("(a) a later generation finished on the host: saves what was shown", async () => {
      const { m, post } = await withEditor();
      await m.showGeneratedContent(shown, 3, 7);

      m.saveOutput();
      // The new output arrives, then the answer to the save of the old one.
      await m.showGeneratedContent("second output", 2, 8);
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(true);

      expect(post.mock.calls).toEqual([
        ["saveFile", { kind: "generated", documentId: 7 }],
        ["saveFile", { kind: "edited", content: shown }],
      ]);
    });

    it("(b) a file preview took the place of the output: saves what was shown", async () => {
      const { m, post } = await withEditor();
      await m.showGeneratedContent(shown, 3, 7);

      m.saveOutput();
      await m.showPreviewContent("file content", "plaintext", "", "/a.txt");
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(true);

      expect(post.mock.lastCall).toEqual([
        "saveFile",
        { kind: "edited", content: shown },
      ]);
    });

    it("retries once, and only the save it has sent", async () => {
      const { m, post } = await withEditor();
      await m.showGeneratedContent(shown, 3, 7);
      m.saveOutput();

      expect(m.retrySaveOfUnavailableDocument(6)).toBe(false);
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(false);
      expect(post).toHaveBeenCalledTimes(1);

      m.saveOutput();
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(true);
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(false);
      expect(post).toHaveBeenCalledTimes(3);
    });

    it("keeps nothing once the save is answered, or for an edited text", async () => {
      const { fake, m, post } = await withEditor();
      await m.showGeneratedContent(shown, 3, 7);
      m.saveOutput();
      m.saveSettled();
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(false);

      fake.editor.type("x");
      m.saveOutput();
      expect(post.mock.lastCall?.[1]).toMatchObject({ kind: "edited" });
      expect(m.retrySaveOfUnavailableDocument(7)).toBe(false);
    });
  });

  describe("creation of the editor", () => {
    it("creates one editor when two previews arrive before Monaco is loaded", async () => {
      const fake = makeFakeMonaco();
      const loading = deferred<typeof fake.ns>();
      loadMonaco.mockReturnValue(loading.promise);
      const m = await freshModules();

      const first = m.showPreviewContent("one", "plaintext", "", "/a.txt");
      const second = m.showPreviewContent("two", "plaintext", "", "/b.txt");
      loading.resolve(fake.ns);
      await Promise.all([first, second]);

      expect(loadMonaco).toHaveBeenCalledTimes(1);
      expect(fake.ns.editor.create).toHaveBeenCalledTimes(1);
      // Both previews went into that editor, in the order they were asked for.
      expect(fake.editor.getValue()).toBe("two");
    });

    it("reuses the editor for later content", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("one", "plaintext", "", "/a.txt");
      await m.showGeneratedContent("out", 3, 1);
      await m.initEditor();

      expect(fake.ns.editor.create).toHaveBeenCalledTimes(1);
    });

    it("does not keep a failed creation: the next call tries again", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockRejectedValueOnce(new Error("chunk failed to load"));
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await expect(
        m.showPreviewContent("one", "plaintext", "", "/a.txt"),
      ).rejects.toThrow("chunk failed to load");
      expect(fake.ns.editor.create).not.toHaveBeenCalled();

      await m.showPreviewContent("one", "plaintext", "", "/a.txt");
      expect(fake.ns.editor.create).toHaveBeenCalledTimes(1);
      expect(fake.editor.getValue()).toBe("one");
    });

    it("maps the language of the file to the one Monaco highlights", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("<p/>", "plaintext", "", "/App.svelte");
      expect(fake.ns.editor.setModelLanguage).toHaveBeenLastCalledWith(
        fake.model,
        "html",
      );

      await m.showPreviewContent("fn main() {}", "rust", "", "/main.rs");
      expect(fake.ns.editor.setModelLanguage).toHaveBeenLastCalledWith(
        fake.model,
        "rust",
      );
    });
  });

  describe("search highlighting", () => {
    it("highlights the matches of the term passed with the preview", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");

      expect(fake.shown()).toHaveLength(2);
      expect(fake.shown()[0].options.inlineClassName).toBe("search-highlight");
      expect(fake.shown()[0].range.startColumn).toBe(1);
    });

    it("keeps one set of decorations, replaced on every change", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      m.appState.update((s) => ({ ...s, content_search_query: "bar" }));
      await m.showPreviewContent("bar bar bar", "plaintext", "bar", "/b.txt");

      // One collection for the life of the editor – no second bookkeeping
      // that could leave stale highlights behind.
      expect(fake.editor.createDecorationsCollection).toHaveBeenCalledTimes(1);
      expect(fake.collections).toHaveLength(1);
      expect(fake.shown()).toHaveLength(3);
    });

    it("follows a change of the content search while a file is shown", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      expect(fake.shown()).toHaveLength(2);

      m.appState.update((s) => ({ ...s, content_search_query: "bar" }));
      expect(fake.shown()).toHaveLength(1);
      expect(fake.shown()[0].range.startColumn).toBe(5);

      m.appState.update((s) => ({ ...s, content_search_query: "" }));
      expect(fake.shown()).toEqual([]);
    });

    it("follows a change of the case sensitivity", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();
      m.appState.update((s) => ({ ...s, content_search_query: "foo" }));

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      expect(fake.shown()).toHaveLength(2);

      m.appState.update((s) => ({
        ...s,
        config: { ...s.config, case_sensitive_search: true },
      }));
      expect(fake.shown()).toHaveLength(1);
    });

    it("does not recompute for a snapshot that leaves the search as it is", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      const searches = fake.model.findMatches.mock.calls.length;
      const updates = fake.collections[0].set.mock.calls.length;

      m.appState.update((s) => ({ ...s, status_message: "Status: Ready." }));
      m.appState.update((s) => ({ ...s, selected_files_count: 3 }));

      expect(fake.model.findMatches.mock.calls.length).toBe(searches);
      expect(fake.collections[0].set.mock.calls.length).toBe(updates);
      expect(fake.shown()).toHaveLength(2);
    });

    it("shows no highlights in the generated output, whatever the search", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      await m.showGeneratedContent("foo foo foo", 3, 1);
      expect(fake.shown()).toEqual([]);

      m.appState.update((s) => ({ ...s, content_search_query: "foo" }));
      expect(fake.shown()).toEqual([]);
      expect(fake.collections).toHaveLength(1);
    });

    it("removes the highlights when the preview is cleared", async () => {
      const fake = makeFakeMonaco();
      loadMonaco.mockResolvedValue(fake.ns);
      const m = await freshModules();

      await m.showPreviewContent("foo bar Foo", "plaintext", "foo", "/a.txt");
      m.clearPreview();
      expect(fake.shown()).toEqual([]);

      m.appState.update((s) => ({ ...s, content_search_query: "bar" }));
      expect(fake.shown()).toEqual([]);
    });
  });
});
