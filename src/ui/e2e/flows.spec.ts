import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  renderAsHost,
  setAppState,
  waitForBridge,
  type AppState,
  type TreeNode,
} from "./support/tree";

/**
 * Deterministic E2E:
 * - The bridge (window.__e2e) is part of the e2e build only (see vite.config.ts, main.ts)
 * - State transitions via window.__e2e.store.setAppState (no timers); the
 *   trees are written down nested and seeded as rows (see support/tree.ts)
 * - No "any", no artificial timeouts, no regression to production logic
 */

function seedTree(): TreeNode[] {
  return [
    {
      path: "/repo",
      name: "repo",
      is_directory: true,
      is_expanded: true,
      is_binary: false,
      is_match: true,
      is_previewed: false,
      selection_state: "none",
      size: 0,
      has_children: true,
      children_loaded: true,
      children: [
        {
          path: "/repo/src",
          name: "src",
          is_directory: true,
          is_expanded: true,
          is_binary: false,
          is_match: true,
          is_previewed: false,
          selection_state: "none",
          size: 0,
          has_children: true,
          children_loaded: true,
          children: [
            {
              path: "/repo/src/index.ts",
              name: "index.ts",
              is_directory: false,
              is_expanded: false,
              is_binary: false,
              is_match: true,
              is_previewed: false,
              selection_state: "none",
              size: 512,
              has_children: false,
              children_loaded: true,
              children: [],
            },
            {
              path: "/repo/src/util.ts",
              name: "util.ts",
              is_directory: false,
              is_expanded: false,
              is_binary: false,
              is_match: true,
              is_previewed: false,
              selection_state: "none",
              size: 420,
              has_children: false,
              children_loaded: true,
              children: [],
            },
          ],
        },
        {
          path: "/repo/README.md",
          name: "README.md",
          is_directory: false,
          is_expanded: false,
          is_binary: false,
          is_match: true,
          is_previewed: false,
          selection_state: "none",
          size: 256,
          has_children: false,
          children_loaded: true,
          children: [],
        },
      ],
    },
  ];
}

function baseState(): AppState {
  return {
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    tree: [],
    current_path: null,
    current_config_filename: null,
    status_message: "Status: Ready.",
    selected_files_count: 0,
    tree_files_count: 0,
    tree_dirs_count: 0,
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    active_ignore_patterns: [],
    config: {
      ignore_patterns: [],
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
      output_directory: "",
      output_filename: "output.txt",
    },
  };
}

function withRepo(): AppState {
  const s = baseState();
  s.current_path = "/repo";
  s.tree = seedTree();
  s.tree_files_count = 3;
  s.tree_dirs_count = 2;
  s.status_message = "Status: Directory selected.";
  return s;
}

/** Recursively clone & prune a tree to only nodes matching `pred` (or having matching descendants). */
function pruneTree(
  nodes: TreeNode[],
  pred: (n: TreeNode) => boolean,
): TreeNode[] {
  const result: TreeNode[] = [];
  for (const n of nodes) {
    if (n.is_directory) {
      const prunedChildren = pruneTree(n.children, pred);
      const keepNode = pred(n) || prunedChildren.length > 0;
      if (keepNode) {
        result.push({
          ...n,
          children: prunedChildren,
        });
      }
    } else {
      if (pred(n)) {
        result.push({ ...n, children: [] });
      }
    }
  }
  return result;
}

/** Mark the given file paths as selected ("full"). Returns a deep-cloned tree. */
function markSelected(nodes: TreeNode[], selected: Set<string>): TreeNode[] {
  return nodes.map((n) => {
    if (n.is_directory) {
      const children = markSelected(n.children, selected);
      // Optional: compute directory selection state (not strictly needed for the test)
      const hasFull = children.some(
        (c) =>
          c.selection_state === "full" ||
          (c.is_directory && c.selection_state === "partial"),
      );
      const hasAny = children.some((c) => c.selection_state !== "none");
      const selection_state: TreeNode["selection_state"] =
        hasFull && !children.some((c) => c.selection_state === "none")
          ? "full"
          : hasAny
            ? "partial"
            : "none";
      return { ...n, children, selection_state };
    }
    if (selected.has(n.path)) {
      return { ...n, selection_state: "full" };
    }
    return { ...n };
  });
}

/** Totals as the backend reports them for a fully expanded tree. */
function countNodes(nodes: TreeNode[]): { files: number; dirs: number } {
  let files = 0;
  let dirs = 0;
  for (const n of nodes) {
    if (n.is_directory) {
      const below = countNodes(n.children);
      files += below.files;
      dirs += 1 + below.dirs;
    } else {
      files += 1;
    }
  }
  return { files, dirs };
}

function withFilter(q: string): AppState {
  const t = q.toLowerCase();
  const pred = (n: TreeNode) => n.name.toLowerCase().includes(t);
  const s = withRepo();
  s.search_query = q;

  // 1) Sichtbaren Baum rekursiv prunen
  const pruned = pruneTree(seedTree(), pred);

  // 2) Mindestens eine Datei als selektiert markieren (damit canGenerate true wird)
  const selectedPath = "/repo/src/index.ts";
  const selected = markSelected(pruned, new Set([selectedPath]));

  s.tree = selected;
  const counts = countNodes(selected);
  s.tree_files_count = counts.files;
  s.tree_dirs_count = counts.dirs;
  s.selected_files_count = 1;
  return s;
}

test("Happy path: Select → Filter → Preview → Generate → Save", async ({
  page,
}, testInfo) => {
  await page.goto("/?e2e=1");

  // Ensure the dev bridge is available deterministically
  await waitForBridge(page);

  // 1) Select directory
  await setAppState(page, withRepo());

  const pathLabel = page.locator("#current-path");
  await expect(pathLabel).toContainText("/repo");

  // index.ts is visible
  const indexRow = page.locator(".tree .file-item .file-name", {
    hasText: "index.ts",
  });
  await expect(indexRow).toBeVisible();

  // 2) Filter to "index" deterministically (no debounce races) + Auswahl setzen
  await setAppState(page, withFilter("index"));

  // Debug-Snapshot anhängen
  const dbgAfterFilter = await page.evaluate(() => {
    return (
      window as unknown as {
        __e2e: { debug: { dump: () => unknown } };
      }
    ).__e2e.debug.dump();
  });
  await testInfo.attach("after-filter.json", {
    body: JSON.stringify(dbgAfterFilter, null, 2),
    contentType: "application/json",
  });

  await expect(indexRow).toBeVisible();
  const utilRow = page.locator(".tree .file-item .file-name", {
    hasText: "util.ts",
  });
  await expect(utilRow).toHaveCount(0);

  // 3) Preview via frontend hook
  await page.evaluate(() => {
    (
      window as unknown as {
        showPreviewContent: (
          c: string,
          l: string,
          s: string | null,
          p: string,
        ) => void;
      }
    ).showPreviewContent(
      "export const x = 1;\n",
      "typescript",
      "",
      "/repo/src/index.ts",
    );
  });

  const previewTitle = page.locator("#preview-title");
  await expect(previewTitle).toContainText("src/");
  await expect(previewTitle).toContainText("index.ts");

  // 4) Generate deterministically (jetzt enabled, da selected_files_count=1)
  const generateBtn = page.locator("#generate-btn");
  await expect(generateBtn).toBeEnabled();
  await generateBtn.click();

  await page.evaluate(() => {
    (
      window as unknown as {
        showGeneratedContent: (c: string, t: number, id: number) => void;
      }
    ).showGeneratedContent("// generated output\n", 1234, 7);
  });
  await expect(previewTitle).toContainText(/Preview generated/i);
  await expect(page.locator("#preview-panel")).toContainText("1.2K tokens");

  // 5) Save: click + backend ack
  const saveBtn = page.locator("#save-btn");
  await expect(saveBtn).toBeEnabled();
  await saveBtn.click();

  await page.evaluate(() => {
    (
      window as unknown as {
        saveResult: (result: { status: "saved"; path: string }) => void;
      }
    ).saveResult({ status: "saved", path: "/tmp/output.txt" });
  });

  await expect(page.locator("#status-bar .status-text")).toHaveText(
    "Status: Saved to /tmp/output.txt",
  );
  await expect(page.locator(".toast-list .toast.success")).toHaveCount(1);
});

test.describe("Saving the generated output", () => {
  type Posted = { command: string; payload: unknown };
  type SaveResult =
    | { status: "saved"; path: string }
    | { status: "cancelled" }
    | { status: "failed"; error: string }
    | { status: "document_unavailable"; document_id: number };

  const GENERATED = "// generated output\nline two\n";

  test.beforeEach(async ({ page }) => {
    // Record what the UI posts to the host.
    await page.addInitScript(() => {
      const w = window as unknown as {
        __posted: string[];
        ipc: { postMessage: (message: string) => void };
      };
      w.__posted = [];
      w.ipc = { postMessage: (message) => w.__posted.push(message) };
    });
    await page.goto("/?e2e=1");
    await page.waitForFunction(() => {
      const w = window as unknown as {
        __e2e?: { store?: { setAppState?: (s: unknown) => void } };
      };
      return typeof w.__e2e?.store?.setAppState === "function";
    });
    await page.evaluate((content) => {
      (
        window as unknown as {
          showGeneratedContent: (c: string, t: number, id: number) => void;
        }
      ).showGeneratedContent(content, 12, 7);
    }, GENERATED);
    await expect(page.locator("#preview-title")).toContainText(
      /Preview generated/i,
    );
    await expect(page.locator("#save-btn")).toBeEnabled();
  });

  /** Click "Save" and return the one command it posts. */
  async function save(page: import("@playwright/test").Page): Promise<Posted> {
    await page.evaluate(() => {
      (window as unknown as { __posted: string[] }).__posted.length = 0;
    });
    await page.locator("#save-btn").click();
    const posted = await page.evaluate(() =>
      (window as unknown as { __posted: string[] }).__posted.map(
        // The command, without the number of its envelope.
        (message) => {
          const { command, payload } = JSON.parse(message) as Posted;
          return { command, payload };
        },
      ),
    );
    expect(posted).toHaveLength(1);
    return posted[0];
  }

  async function answer(
    page: import("@playwright/test").Page,
    result: SaveResult,
  ) {
    await page.evaluate((r) => {
      (
        window as unknown as { saveResult: (result: SaveResult) => void }
      ).saveResult(r);
    }, result);
  }

  /** Put the cursor at the start of the editor's text, as a click does. */
  async function focusEditor(page: import("@playwright/test").Page) {
    await page
      .locator("#editor-container .view-line")
      .first()
      .click({ position: { x: 1, y: 5 } });
  }

  test("an unedited output is named, not sent back", async ({ page }) => {
    expect(await save(page)).toEqual({
      command: "saveFile",
      payload: { kind: "generated", documentId: 7 },
    });
  });

  test("an edited output is sent, and named again once the edit is undone", async ({
    page,
  }) => {
    await focusEditor(page);
    await page.keyboard.type("my note");
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "my note",
    );

    expect(await save(page)).toEqual({
      command: "saveFile",
      payload: { kind: "edited", content: "my note" + GENERATED },
    });

    await focusEditor(page);
    // Monaco binds undo to the modifier of the platform the page reports.
    const undo = (await page.evaluate(() => /Mac/.test(navigator.userAgent)))
      ? "Meta+z"
      : "Control+z";
    // The note is more than one step of the undo history (a step per word).
    const lines = page.locator("#editor-container .view-lines");
    await page.keyboard.press(undo);
    await expect(lines).not.toContainText("note");
    await expect(lines).toContainText("my");
    expect((await save(page)).payload).toMatchObject({ kind: "edited" });

    await focusEditor(page);
    await page.keyboard.press(undo);
    await expect(lines).not.toContainText("my");

    expect(await save(page)).toEqual({
      command: "saveFile",
      payload: { kind: "generated", documentId: 7 },
    });
  });

  test("a newer output is the one a save names", async ({ page }) => {
    await focusEditor(page);
    await page.keyboard.type("edited");
    await page.evaluate(() => {
      (
        window as unknown as {
          showGeneratedContent: (c: string, t: number, id: number) => void;
        }
      ).showGeneratedContent("// second output\n", 3, 8);
    });
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "second output",
    );

    expect(await save(page)).toEqual({
      command: "saveFile",
      payload: { kind: "generated", documentId: 8 },
    });
  });

  test("the output can still be saved after another directory was opened", async ({
    page,
  }) => {
    const render = (next: AppState) => renderAsHost(page, next);

    // The host reports directory A, then directory B: the output stays on
    // screen, and a save still names it.
    await render(withRepo());
    await render({ ...withRepo(), current_path: "/other" });
    await expect(page.locator("#current-path")).toContainText("/other");
    await expect(page.locator("#preview-title")).toContainText(
      /Preview generated/i,
    );
    expect(await save(page)).toEqual({
      command: "saveFile",
      payload: { kind: "generated", documentId: 7 },
    });

    // No directory any more: the editor is cleared, and the host is told so
    // (that is where it gives its copy of the output up).
    await page.evaluate(() => {
      (window as unknown as { __posted: string[] }).__posted.length = 0;
    });
    await render({ ...withRepo(), current_path: "", tree: [] });
    await expect(page.locator("#save-btn")).toBeDisabled();
    const posted = await page.evaluate(() =>
      (window as unknown as { __posted: string[] }).__posted.map(
        (message) => (JSON.parse(message) as Posted).command,
      ),
    );
    expect(posted).toContain("clearPreviewState");
  });

  test("an unedited output is copied with the line ends of its files", async ({
    page,
  }) => {
    // What is written to the clipboard, recorded.
    await page.evaluate(() => {
      const w = window as unknown as { __copied: string[] };
      w.__copied = [];
      Object.defineProperty(navigator, "clipboard", {
        configurable: true,
        value: {
          writeText: (text: string) => {
            w.__copied.push(text);
            return Promise.resolve();
          },
        },
      });
    });
    const copy = async () => {
      await page.evaluate(() => {
        (window as unknown as { __copied: string[] }).__copied.length = 0;
      });
      await page.locator("#copy-btn").click();
      await expect
        .poll(() =>
          page.evaluate(
            () => (window as unknown as { __copied: string[] }).__copied.length,
          ),
        )
        .toBe(1);
      return page.evaluate(
        () => (window as unknown as { __copied: string[] }).__copied[0],
      );
    };

    // One file with CRLF line ends among files with LF.
    const mixed =
      "# CFC Output\n\nwin.txt\n===FILE-START===\none\r\ntwo\r\n---FILE-END-----\n\n" +
      "unix.txt\n===FILE-START===\none\ntwo\nthree\nfour\n---FILE-END-----\n\n";
    await page.evaluate((content) => {
      (
        window as unknown as {
          showGeneratedContent: (c: string, t: number, id: number) => void;
        }
      ).showGeneratedContent(content, 30, 9);
    }, mixed);
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "win.txt",
    );

    // Unedited: the copy is the text of the host, and the save names it.
    expect(await copy()).toBe(mixed);
    expect((await save(page)).payload).toEqual({
      kind: "generated",
      documentId: 9,
    });

    // Edited: the editor's text is the one text there is, for both.
    await focusEditor(page);
    await page.keyboard.type("x");
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "x# CFC Output",
    );
    const copied = await copy();
    expect(copied.startsWith("x# CFC Output")).toBe(true);
    expect((await save(page)).payload).toEqual({
      kind: "edited",
      content: copied,
    });
  });

  /** What the UI has posted since the last `save()`, without the numbers. */
  async function postedSince(
    page: import("@playwright/test").Page,
  ): Promise<Posted[]> {
    return page.evaluate(() =>
      (window as unknown as { __posted: string[] }).__posted.map((message) => {
        const { command, payload } = JSON.parse(message) as Posted;
        return { command, payload };
      }),
    );
  }

  test("a save that names a document the host has just replaced saves what was shown", async ({
    page,
  }) => {
    // Save is pressed on output 7 ...
    expect((await save(page)).payload).toEqual({
      kind: "generated",
      documentId: 7,
    });
    // ... while the host had finished the next generation already: its output
    // arrives first, then the answer to the save.
    await page.evaluate(() => {
      (
        window as unknown as {
          showGeneratedContent: (c: string, t: number, id: number) => void;
        }
      ).showGeneratedContent("// second output\n", 3, 8);
    });
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "second output",
    );
    await answer(page, { status: "document_unavailable", document_id: 7 });

    // The text the user pressed Save on is sent, once; nothing is reported.
    expect(await postedSince(page)).toEqual([
      { command: "saveFile", payload: { kind: "generated", documentId: 7 } },
      { command: "saveFile", payload: { kind: "edited", content: GENERATED } },
    ]);
    await expect(page.locator(".toast-list .toast")).toHaveCount(0);

    await answer(page, { status: "saved", path: "/tmp/out.txt" });
    await expect(page.locator(".toast-list .toast").last()).toContainText(
      "File saved",
    );
  });

  test("a save that names a document a file preview has just replaced saves what was shown", async ({
    page,
  }) => {
    await save(page);
    await page.evaluate(() => {
      (
        window as unknown as {
          showPreviewContent: (
            c: string,
            l: string,
            s: string | null,
            p: string,
          ) => void;
        }
      ).showPreviewContent("file content\n", "plaintext", null, "/repo/a.txt");
    });
    await expect(page.locator("#editor-container .view-lines")).toContainText(
      "file content",
    );
    await answer(page, { status: "document_unavailable", document_id: 7 });

    expect((await postedSince(page)).at(-1)).toEqual({
      command: "saveFile",
      payload: { kind: "edited", content: GENERATED },
    });
    await expect(page.locator(".toast-list .toast")).toHaveCount(0);
  });

  test("a document nobody has any more is a failed save, said in the UI's language", async ({
    page,
  }) => {
    // Not the answer to a save of this UI: there is nothing to repeat.
    await answer(page, { status: "document_unavailable", document_id: 99 });

    const toast = page.locator(".toast-list .toast").last();
    await expect(toast).toHaveClass(/error/);
    await expect(toast).toContainText("no longer available");
    expect(
      (await postedSince(page)).filter((p) => p.command === "saveFile"),
    ).toEqual([]);
  });

  test("each result of a save has its own toast", async ({ page }) => {
    const toasts = page.locator(".toast-list .toast");

    await answer(page, { status: "cancelled" });
    await expect(toasts.last()).toHaveClass(/info/);
    await expect(toasts.last()).toContainText("Save cancelled");
    await expect(page.locator("#status-bar .status-text")).toHaveText(
      "Status: Save cancelled.",
    );

    await answer(page, {
      status: "failed",
      error: "Permission denied (os error 13)",
    });
    await expect(toasts.last()).toHaveClass(/error/);
    await expect(toasts.last()).toContainText("Failed to save file");
    await expect(toasts.last()).toContainText(
      "Permission denied (os error 13)",
    );
    await expect(page.locator("#status-bar .status-text")).toHaveText(
      "Error: Failed to save file.",
    );

    // A path is a path, whatever it is called.
    await answer(page, { status: "saved", path: "/tmp/cancelled" });
    await expect(toasts.last()).toHaveClass(/success/);
    await expect(toasts.last()).toContainText("File saved");
    await expect(page.locator("#status-bar .status-text")).toHaveText(
      "Status: Saved to /tmp/cancelled",
    );
  });

  test("the size of the output is announced or refused with a toast", async ({
    page,
  }) => {
    type Notice = {
      status: "large" | "too_large";
      estimated_bytes: number;
      limit_bytes: number;
    };
    const notify = (notice: Notice) =>
      page.evaluate((n) => {
        (
          window as unknown as { outputSizeNotice: (notice: Notice) => void }
        ).outputSizeNotice(n);
      }, notice);
    const toasts = page.locator(".toast-list .toast");

    await notify({
      status: "large",
      estimated_bytes: 52428800,
      limit_bytes: 20971520,
    });
    await expect(toasts.last()).toHaveClass(/warning/);
    await expect(toasts.last()).toContainText("Large output");
    await expect(toasts.last()).toContainText("50 MB");

    await notify({
      status: "too_large",
      estimated_bytes: 314572800,
      limit_bytes: 209715200,
    });
    await expect(toasts.last()).toHaveClass(/error/);
    await expect(toasts.last()).toContainText("Output too large");
    await expect(toasts.last()).toContainText("300 MB");
    await expect(toasts.last()).toContainText("200 MB");
    // The output that is shown stays, and can still be saved.
    await expect(page.locator("#save-btn")).toBeEnabled();
    expect((await save(page)).payload).toEqual({
      kind: "generated",
      documentId: 7,
    });
  });
});

test("A11y sanity on main screen (no critical issues)", async ({ page }) => {
  await page.goto("/?e2e=1");

  await waitForBridge(page);
  await setAppState(page, withRepo());

  const results = await new AxeBuilder({ page }).analyze();
  const critical = results.violations.filter(
    (v) => v.impact === "critical" || v.impact === "serious",
  );
  expect(critical.length, "Expected no CRITICAL a11y violations").toBe(0);

  // With a toast on screen (its list must only own list items).
  await page.evaluate(() => {
    (window as unknown as { showError: (m: string) => void }).showError(
      "Something failed",
    );
  });
  await expect(page.locator(".toast-list .toast")).toHaveCount(1);
  const withToast = await new AxeBuilder({ page }).analyze();
  expect(
    withToast.violations
      .filter((v) => v.impact === "critical" || v.impact === "serious")
      .map((v) => v.id),
    "Expected no CRITICAL a11y violations with a toast shown",
  ).toEqual([]);
});

test.describe("Keyboard on the controls of a tree row", () => {
  type Posted = { command: string; payload: unknown };

  test.beforeEach(async ({ page }) => {
    // Record what the UI posts to the host.
    await page.addInitScript(() => {
      const w = window as unknown as {
        __posted: string[];
        ipc: { postMessage: (message: string) => void };
      };
      w.__posted = [];
      w.ipc = { postMessage: (message) => w.__posted.push(message) };
    });
    await page.goto("/?e2e=1");
    await waitForBridge(page);
    await setAppState(page, withRepo());
    await expect(page.locator(".tree .tree-item-container")).toHaveCount(5);
  });

  /** Focus a control of a row, press a key on it, return what was posted. */
  async function press(
    page: import("@playwright/test").Page,
    selector: string,
    key: string,
  ): Promise<Posted[]> {
    const control = page.locator(selector);
    await control.focus();
    await expect(control).toBeFocused();
    await page.evaluate(() => {
      (window as unknown as { __posted: string[] }).__posted.length = 0;
    });
    await page.keyboard.press(key);
    return page.evaluate(() =>
      (window as unknown as { __posted: string[] }).__posted.map(
        // The command, without the number of its envelope.
        (message) => {
          const { command, payload } = JSON.parse(message) as Posted;
          return { command, payload };
        },
      ),
    );
  }
  const inRow = (path: string, control: string) =>
    `.tree .tree-item-container[data-path="${path}"] ${control}`;

  for (const key of ["Enter", "Space"]) {
    test(`${key} on the ignore button of a directory ignores it`, async ({
      page,
    }) => {
      const posted = await press(page, inRow("/repo/src", ".ignore-btn"), key);
      expect(posted).toEqual([
        { command: "addIgnorePath", payload: "/repo/src" },
      ]);
    });

    test(`${key} on the ignore button of a file ignores it`, async ({
      page,
    }) => {
      const posted = await press(
        page,
        inRow("/repo/README.md", ".ignore-btn"),
        key,
      );
      expect(posted).toEqual([
        { command: "addIgnorePath", payload: "/repo/README.md" },
      ]);
    });
  }

  test("Space on a checkbox toggles the selection once", async ({ page }) => {
    expect(
      await press(page, inRow("/repo/src", 'input[type="checkbox"]'), "Space"),
    ).toEqual([{ command: "toggleDirectorySelection", payload: "/repo/src" }]);
    expect(
      await press(
        page,
        inRow("/repo/README.md", 'input[type="checkbox"]'),
        "Space",
      ),
    ).toEqual([{ command: "toggleSelection", payload: "/repo/README.md" }]);
  });

  test("Delete on a row ignores it, as its ignore button does", async ({
    page,
  }) => {
    // Delete acts only on a row in sight and not while a focus lookup is
    // open; before the tree has settled (slow WebKit on Linux) a press posts
    // nothing. The press is repeated until the tree is ready.
    for (const path of ["/repo/README.md", "/repo/src"]) {
      await expect(async () => {
        expect(await press(page, inRow(path, ""), "Delete")).toEqual([
          { command: "addIgnorePath", payload: path },
        ]);
      }).toPass({ timeout: 5_000 });
    }
  });

  test("Delete with a modifier, and Backspace, ignore nothing", async ({
    page,
  }) => {
    for (const key of ["Shift+Delete", "Control+Delete", "Backspace"]) {
      expect(await press(page, inRow("/repo/README.md", ""), key)).toEqual([]);
    }
  });

  test("a row is named by its name, its ignore button by the key", async ({
    page,
  }) => {
    await expect(
      page.getByRole("treeitem", { name: "README.md", exact: true }),
    ).toHaveAttribute("data-path", "/repo/README.md");
    await expect(
      page.getByRole("treeitem", { name: "src", exact: true }),
    ).toHaveAttribute("data-path", "/repo/src");

    // The platform as the browser under test reports it (an emulated
    // device may say another one than the machine the test runs on).
    const mac = await page.evaluate(() => {
      const nav = navigator as Navigator & {
        userAgentData?: { platform?: string };
      };
      return /^mac/i.test(nav.userAgentData?.platform || nav.platform);
    });
    await expect(
      page.locator(inRow("/repo/README.md", ".ignore-btn")),
    ).toHaveAttribute(
      "title",
      `Add this file to ignore patterns (${mac ? "fn+⌫" : "Delete"})`,
    );
  });

  test("the tree is one stop for Tab: the focused row", async ({ page }) => {
    const stops = () =>
      page.evaluate(() => {
        const tree = document.querySelector<HTMLElement>(".tree")!;
        return [tree, ...tree.querySelectorAll<HTMLElement>("*")]
          .filter((el) => el.tabIndex >= 0)
          .map((el) => el.getAttribute("data-path") ?? el.className);
      });
    expect(await stops()).toEqual(["/repo"]);

    await page.locator(inRow("/repo", "")).focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator(inRow("/repo/src", ""))).toBeFocused();
    expect(await stops()).toEqual(["/repo/src"]);
  });

  test("Enter and Space on a row still act on the row", async ({ page }) => {
    expect(await press(page, inRow("/repo/README.md", ""), "Enter")).toEqual([
      { command: "loadFilePreview", payload: "/repo/README.md" },
    ]);
    expect(await press(page, inRow("/repo/README.md", ""), "Space")).toEqual([
      { command: "toggleSelection", payload: "/repo/README.md" },
    ]);
  });
});

test.describe("Text being typed and the answers of the host", () => {
  type Message = {
    session: number;
    seq: number;
    command: string;
    payload: unknown;
  };

  test.beforeEach(async ({ page }) => {
    // Record what the UI posts to the host; the stand-in host of the bridge
    // listens as well and counts the commands as processed.
    await page.addInitScript(() => {
      const w = window as unknown as {
        __posted: string[];
        ipc: { postMessage: (message: string) => void };
      };
      w.__posted = [];
      w.ipc = { postMessage: (message) => w.__posted.push(message) };
    });
    await page.goto("/?e2e=1");
    await waitForBridge(page);
    await renderAsHost(page, withRepo());
    await expect(page.locator("#current-path")).toContainText("/repo");
  });

  const messages = (page: import("@playwright/test").Page) =>
    page.evaluate(() =>
      (window as unknown as { __posted: string[] }).__posted.map(
        (message) => JSON.parse(message) as Message,
      ),
    );

  test("every command is posted with the next number", async ({ page }) => {
    await page.locator("#search-query").fill("index");
    await expect
      .poll(async () => (await messages(page)).at(-1)?.command)
      .toBe("updateFilters");

    const posted = await messages(page);
    const { session } = posted[0];
    expect(Number.isSafeInteger(session) && session > 0).toBe(true);
    expect(posted[0]).toEqual({
      session,
      seq: 1,
      command: "initialize",
      payload: null,
    });
    expect(posted.map((message) => message.seq)).toEqual(
      posted.map((_, index) => index + 1),
    );
    // One session for as long as the page is loaded ...
    expect(posted.every((message) => message.session === session)).toBe(true);
    // ... and another one after a reload, counting from 1 again.
    await page.reload();
    await waitForBridge(page);
    const [first] = await messages(page);
    expect(first.seq).toBe(1);
    expect(first.session).not.toBe(session);
    for (const message of posted) {
      expect(Object.keys(message).sort()).toEqual([
        "command",
        "payload",
        "seq",
        "session",
      ]);
    }
  });

  test("a filter text stays until the host has processed its command", async ({
    page,
  }) => {
    const search = page.locator("#search-query");
    await search.fill("index");
    await expect
      .poll(async () => (await messages(page)).at(-1)?.command)
      .toBe("updateFilters");
    const { seq } = (await messages(page)).at(-1)!;

    // A snapshot that was under way when the filter was sent: it accounts
    // for the commands before it and still shows no filter.
    await renderAsHost(page, {
      ...withRepo(),
      current_config_filename: "from-before.json",
      last_processed_seq: seq - 1,
    });
    await expect(page.getByText("from-before.json").first()).toBeVisible();
    await expect(search).toHaveValue("index");

    // A snapshot the host made for an earlier load of the UI (before a
    // reload): its number is high, but it is not about this UI's commands.
    await renderAsHost(page, {
      ...withRepo(),
      current_config_filename: "for-the-ui-before.json",
      last_processed_session: 1,
      last_processed_seq: seq + 1000,
    });
    await expect(
      page.getByText("for-the-ui-before.json").first(),
    ).toBeVisible();
    await expect(search).toHaveValue("index");

    // The answer. The host is the source of truth: what it reports is shown,
    // here a text it has changed.
    await renderAsHost(page, {
      ...withRepo(),
      search_query: "index.ts",
      last_processed_seq: seq,
    });
    await expect(search).toHaveValue("index.ts");

    // And from then on every snapshot is followed again.
    await renderAsHost(page, { ...withRepo(), search_query: "" });
    await expect(search).toHaveValue("");
  });

  test("text typed after a send is not replaced by the answer to it", async ({
    page,
  }) => {
    const search = page.locator("#search-query");
    await search.fill("ind");
    await expect
      .poll(async () => (await messages(page)).at(-1)?.command)
      .toBe("updateFilters");
    const first = (await messages(page)).at(-1)!;

    // Typing on; the answer to the first text arrives before the debounce of
    // the second one has run out, or after: either way the text stays.
    await search.pressSequentially("ex");
    await renderAsHost(page, {
      ...withRepo(),
      search_query: "ind",
      last_processed_seq: first.seq,
    });
    await expect(search).toHaveValue("index");

    await expect
      .poll(async () => (await messages(page)).at(-1)?.payload)
      .toMatchObject({ searchQuery: "index" });
    const second = (await messages(page)).at(-1)!;
    expect(second.seq).toBeGreaterThan(first.seq);
    await renderAsHost(page, {
      ...withRepo(),
      search_query: "index",
      last_processed_seq: second.seq,
    });
    await expect(search).toHaveValue("index");
  });
});
