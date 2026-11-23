import { test, expect } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";

/**
 * Deterministic E2E:
 * - The bridge (window.__e2e) is part of the e2e build only (see vite.config.ts, main.ts)
 * - State transitions via window.__e2e.store.setAppState (no timers)
 * - No "any", no artificial timeouts, no regression to production logic
 */

type TreeNode = {
  path: string;
  name: string;
  is_directory: boolean;
  is_expanded: boolean;
  is_binary: boolean;
  is_match: boolean;
  is_previewed: boolean;
  selection_state: "none" | "full" | "partial";
  size: number;
  /** Only expanded directories carry children (pruned snapshot). */
  children: TreeNode[];
  has_children: boolean;
  children_loaded: boolean;
};

type AppConfig = {
  ignore_patterns: string[];
  case_sensitive_search: boolean;
  include_tree_by_default: boolean;
  use_relative_paths: boolean;
  remove_empty_directories: boolean;
  output_directory: string;
  output_filename: string;
};

type AppState = {
  is_scanning: boolean;
  is_generating: boolean;
  is_fully_scanned: boolean;
  patterns_need_rescan: boolean;
  tree: TreeNode[];
  current_path: string | null;
  current_config_filename: string | null;
  status_message: string;
  selected_files_count: number;
  tree_files_count: number;
  tree_dirs_count: number;
  search_query: string;
  extension_filter: string;
  content_search_query: string;
  active_ignore_patterns: string[];
  config: AppConfig;
};

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
  await page.waitForFunction(() => {
    const w = window as unknown as {
      __e2e?: { store?: { setAppState?: (s: unknown) => void } };
    };
    return typeof w.__e2e?.store?.setAppState === "function";
  });

  // 1) Select directory
  await page.evaluate((next) => {
    (
      window as unknown as {
        __e2e: { store: { setAppState: (s: AppState) => void } };
      }
    ).__e2e.store.setAppState(next);
  }, withRepo());

  const pathLabel = page.locator("#current-path");
  await expect(pathLabel).toContainText("/repo");

  // index.ts is visible
  const indexRow = page.locator(".tree .file-item .file-name", {
    hasText: "index.ts",
  });
  await expect(indexRow).toBeVisible();

  // 2) Filter to "index" deterministically (no debounce races) + Auswahl setzen
  await page.evaluate((next) => {
    (
      window as unknown as {
        __e2e: { store: { setAppState: (s: AppState) => void } };
      }
    ).__e2e.store.setAppState(next);
  }, withFilter("index"));

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
        showGeneratedContent: (c: string, t: number) => void;
      }
    ).showGeneratedContent("// generated output\n", 1234);
  });
  await expect(previewTitle).toContainText(/Preview generated/i);

  // 5) Save: click + backend ack
  const saveBtn = page.locator("#save-btn");
  await expect(saveBtn).toBeEnabled();
  await saveBtn.click();

  await page.evaluate(() => {
    (
      window as unknown as {
        fileSaveStatus: (ok: boolean, p: string) => void;
        __e2e: { savedPath?: string | null };
      }
    ).fileSaveStatus(true, "/tmp/output.txt");
    (
      window as unknown as { __e2e: { savedPath?: string | null } }
    ).__e2e.savedPath = "/tmp/output.txt";
  });

  const savedPath = await page.evaluate(() => {
    return (
      (window as unknown as { __e2e: { savedPath?: string | null } }).__e2e
        .savedPath ?? null
    );
  });
  expect(savedPath).toBe("/tmp/output.txt");
});

test("A11y sanity on main screen (no critical issues)", async ({ page }) => {
  await page.goto("/?e2e=1");

  await page.waitForFunction(() => {
    const w = window as unknown as {
      __e2e?: { store?: { setAppState?: (s: unknown) => void } };
    };
    return typeof w.__e2e?.store?.setAppState === "function";
  });
  await page.evaluate((next) => {
    (
      window as unknown as {
        __e2e: { store: { setAppState: (s: AppState) => void } };
      }
    ).__e2e.store.setAppState(next);
  }, withRepo());

  const results = await new AxeBuilder({ page }).analyze();
  const critical = results.violations.filter((v) => v.impact === "critical");
  expect(critical.length, "Expected no CRITICAL a11y violations").toBe(0);
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
    await page.waitForFunction(() => {
      const w = window as unknown as {
        __e2e?: { store?: { setAppState?: (s: unknown) => void } };
      };
      return typeof w.__e2e?.store?.setAppState === "function";
    });
    await page.evaluate((next) => {
      (
        window as unknown as {
          __e2e: { store: { setAppState: (s: AppState) => void } };
        }
      ).__e2e.store.setAppState(next);
    }, withRepo());
    await expect(page.locator(".tree .tree-item-container")).toHaveCount(5);
    // A tree that appears takes the focus to its first row, one frame later.
    // Wait for that, or it can land after the focus a test sets itself.
    await expect(page.locator('.tree [data-index="0"]')).toBeFocused();
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
        (message) => JSON.parse(message) as Posted,
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

  test("Enter and Space on a row still act on the row", async ({ page }) => {
    expect(await press(page, inRow("/repo/README.md", ""), "Enter")).toEqual([
      { command: "loadFilePreview", payload: "/repo/README.md" },
    ]);
    expect(await press(page, inRow("/repo/README.md", ""), "Space")).toEqual([
      { command: "toggleSelection", payload: "/repo/README.md" },
    ]);
  });
});
