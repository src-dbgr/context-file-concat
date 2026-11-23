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
  const collections: Array<{ set: Mock<(d: Decoration[]) => void> }> = [];

  const model = {
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
    }),
    getValue: () => value,
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
      await m.showGeneratedContent("out", 3);
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
      await m.showGeneratedContent("foo foo foo", 3);
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
