import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  fileNode,
  setAppState,
  setTree,
  waitForBridge,
  type AppState,
  type TreeNode,
} from "./support/tree";

/**
 * Accessibility of every screen of the app, in both themes and (through the
 * Playwright projects) in Chromium and WebKit.
 *
 * axe-core checks each screen; a violation of impact "serious" or "critical"
 * fails the test. That covers the contrast of every text (WCAG AA), the names
 * of the controls and the structure of the page. A keyboard-only run through
 * the page checks what axe cannot: that Tab reaches every control and that
 * the focused control shows it.
 */

const THEMES = ["light", "dark"] as const;
type Theme = (typeof THEMES)[number];

const FILES = 5_000;
const ROW_HEIGHT = 28;

function directory(path: string, children: TreeNode[]): TreeNode {
  return {
    ...fileNode(path, 0),
    is_directory: true,
    is_expanded: true,
    has_children: true,
    children,
  };
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

/**
 * A directory with every kind of row: selected, partly selected, previewed,
 * binary, a collapsed directory; with filters, ignore patterns and a loaded
 * config, so that every text of the main screen is on it.
 */
function withRepo(): AppState {
  const state = baseState();
  state.current_path = "/repo";
  state.current_config_filename = "team.cfc.json";
  state.tree = [
    {
      ...directory("/repo/src", [
        { ...fileNode("/repo/src/index.ts", 512), selection_state: "full" },
        { ...fileNode("/repo/src/util.ts", 420), is_previewed: true },
        { ...fileNode("/repo/src/logo.png", 20_480), is_binary: true },
        { ...fileNode("/repo/src/notes.md", 64), is_match: false },
      ]),
      selection_state: "partial",
    },
    { ...directory("/repo/docs", []), is_expanded: false },
    fileNode("/repo/README.md", 256),
  ];
  state.tree_files_count = 5;
  state.tree_dirs_count = 2;
  state.selected_files_count = 1;
  state.search_query = "s";
  state.extension_filter = "ts";
  state.content_search_query = "export";
  state.patterns_need_rescan = true;
  state.active_ignore_patterns = ["node_modules/"];
  state.config.ignore_patterns = ["node_modules/", "*.log", "target/"];
  state.config.output_directory = "/tmp/out";
  state.status_message = "Status: Directory selected.";
  return state;
}

function withLongTree(): AppState {
  const files: TreeNode[] = [];
  for (let i = 0; i < FILES; i++) {
    files.push(fileNode(`/repo/src/file-${String(i).padStart(4, "0")}.txt`));
  }
  const state = baseState();
  state.current_path = "/repo";
  state.tree = [directory("/repo/src", files), fileNode("/repo/zebra.md")];
  state.tree_files_count = FILES + 1;
  state.tree_dirs_count = 1;
  return state;
}

async function open(page: Page, theme: Theme): Promise<void> {
  await page.addInitScript((value) => {
    localStorage.setItem("cfc-theme", value);
  }, theme);
  await page.goto("/?e2e=1");
  await waitForBridge(page);
  await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
}

async function showPreview(page: Page): Promise<void> {
  await page.evaluate(() => {
    (
      window as unknown as {
        showPreviewContent: (
          content: string,
          language: string,
          search: string | null,
          path: string,
        ) => void;
      }
    ).showPreviewContent(
      "// a comment\nexport const answer: number = 42;\nexport function greet(name: string) {\n  return `hello ${name}`;\n}\n",
      "typescript",
      "export",
      "/repo/src/util.ts",
    );
  });
  await expect(page.locator("#preview-title")).toContainText("util.ts");
  await expect(
    page.locator("#editor-container .view-line").first(),
  ).toBeVisible();
}

async function showGenerated(page: Page): Promise<void> {
  await page.evaluate(() => {
    (
      window as unknown as {
        showGeneratedContent: (c: string, tokens: number, id: number) => void;
      }
    ).showGeneratedContent(
      "# CFC Output\n\n// generated output\nexport const answer = 42;\n",
      1234,
      7,
    );
  });
  await expect(page.locator("#save-btn")).toBeEnabled();
  await expect(
    page.locator("#editor-container .view-line").first(),
  ).toBeVisible();
}

/** One toast of every kind. */
async function showToasts(page: Page): Promise<void> {
  await page.evaluate(() => {
    const w = window as unknown as {
      showError: (message: string) => void;
      saveResult: (result: unknown) => void;
      outputSizeNotice: (notice: unknown) => void;
    };
    w.showError("Something failed");
    w.saveResult({ status: "saved", path: "/tmp/output.txt" });
    w.saveResult({ status: "cancelled" });
    w.outputSizeNotice({
      status: "large",
      estimated_bytes: 52428800,
      limit_bytes: 20971520,
    });
  });
  for (const kind of ["error", "success", "info", "warning"]) {
    await expect(page.locator(`.toast-list .toast.${kind}`)).toHaveCount(1);
  }
  await settle(page);
}

/** Waits for running transitions: axe measures the colors as painted. */
async function settle(page: Page): Promise<void> {
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter(
          (animation) =>
            animation.effect?.getComputedTiming().iterations !== Infinity,
        )
        .map((animation) => animation.finished.catch(() => undefined)),
    ),
  );
}

/**
 * Rules of impact "moderate" that fail a screen as well: the page has one
 * heading of level 1, no heading level is skipped, and everything lies in a
 * landmark.
 */
const STRUCTURE_RULES = ["page-has-heading-one", "heading-order", "region"];

/**
 * Fails on every violation of impact "serious" or "critical", and on the
 * rules of the structure of the page.
 */
async function expectAccessible(page: Page): Promise<void> {
  await settle(page);
  const results = await new AxeBuilder({ page }).analyze();
  const found = results.violations
    .filter(
      (v) =>
        v.impact === "serious" ||
        v.impact === "critical" ||
        STRUCTURE_RULES.includes(v.id),
    )
    .flatMap((v) =>
      v.nodes.map(
        (node) =>
          `${v.impact} ${v.id}: ${node.target.join(" ")}\n    ${(node.failureSummary ?? "").replace(/\s+/g, " ").slice(0, 320)}`,
      ),
    );
  expect(found, found.join("\n")).toEqual([]);
}

type KeyboardWindow = {
  __tabStops: Element[];
  __focusLook: (el: Element) => string;
};

/**
 * Walks the page with Tab alone, once around. Every control that can be
 * operated must be reached, and each must look different with the focus
 * than without it.
 */
async function expectKeyboardReach(
  page: Page,
  browserName: string,
): Promise<void> {
  // A state is compared, not a transition into it.
  await page.addStyleTag({
    content: "*, *::before, *::after { transition: none !important; }",
  });
  // Safari and WKWebView reach controls other than text fields with
  // Option+Tab, unless "Keyboard navigation" is on in the system settings.
  const tab = browserName === "webkit" ? "Alt+Tab" : "Tab";

  await page.evaluate(() => {
    const w = window as unknown as KeyboardWindow;
    w.__tabStops = [];
    // What of the look of an element can show that it has the focus.
    w.__focusLook = (el) => {
      const cs = getComputedStyle(el);
      return [
        cs.outlineStyle === "none" ? "none" : cs.outlineWidth + cs.outlineColor,
        cs.boxShadow,
        cs.borderTopColor,
        cs.backgroundColor,
      ].join("|");
    };
    (document.activeElement as HTMLElement | null)?.blur();
  });

  const stops: Array<{ key: string; focused: string }> = [];
  for (let i = 0; i < 120; i++) {
    await page.keyboard.press(tab);
    const stop = await page.evaluate(() => {
      const w = window as unknown as KeyboardWindow;
      const el = document.activeElement;
      if (!el || el === document.body) return "outside";
      if (w.__tabStops.includes(el)) return "around";
      w.__tabStops.push(el);
      const where =
        el.closest("[data-path]")?.getAttribute("data-path") ??
        (el.textContent ?? "").trim().slice(0, 24);
      return {
        key: el.id
          ? `#${el.id}`
          : `${el.tagName.toLowerCase()}.${el.getAttribute("class") ?? ""} [${where}]`,
        focused: w.__focusLook(el),
      };
    });
    if (stop === "outside") continue; // the browser's own UI, between rounds
    if (stop === "around") break;
    stops.push(stop);
  }

  // Every control of the page was a stop.
  const missed = await page.evaluate(() => {
    const w = window as unknown as KeyboardWindow;
    const controls = document.querySelectorAll<HTMLElement>(
      'button, input, select, textarea, summary, a[href], [role="button"], [role="switch"]',
    );
    return [...controls]
      .filter((el) => {
        if (el.matches(":disabled") || el.tabIndex < 0) return false;
        if (el.closest(".monaco-editor")) return false; // the editor is one stop
        return el.checkVisibility({ visibilityProperty: true });
      })
      .filter((el) => !w.__tabStops.includes(el))
      .map((el) => el.id || el.outerHTML.slice(0, 120));
  });
  expect(missed, "controls Tab does not reach").toEqual([]);
  expect(
    await page.evaluate(() =>
      (window as unknown as KeyboardWindow).__tabStops.some((el) =>
        el.closest(".monaco-editor"),
      ),
    ),
    "the editor is a stop, and Tab leaves it again",
  ).toBe(true);
  const keys = stops.map((stop) => stop.key);
  for (const id of ["#search-query", "#copy-btn", "#generate-btn"]) {
    expect(keys).toContain(id);
  }

  // Each stop looks different without the focus.
  const unfocused = await page.evaluate(() => {
    const w = window as unknown as KeyboardWindow;
    (document.activeElement as HTMLElement | null)?.blur();
    return w.__tabStops.map((el) => w.__focusLook(el));
  });
  const invisible = stops
    .filter((stop, i) => stop.focused === unfocused[i])
    .map((stop) => stop.key);
  expect(invisible, "stops that do not show the focus").toEqual([]);
}

for (const theme of THEMES) {
  test.describe(`a11y, ${theme} theme`, () => {
    test.beforeEach(async ({ page }) => {
      await open(page, theme);
    });

    test("main screen without a directory", async ({ page }) => {
      await expectAccessible(page);
    });

    test("main screen with a directory", async ({ page }) => {
      await setAppState(page, withRepo());
      await expect(page.locator(".tree .tree-item-container")).toHaveCount(7);
      await expectAccessible(page);
    });

    test("main screen with toasts", async ({ page }) => {
      await setAppState(page, withRepo());
      await showToasts(page);
      await expectAccessible(page);
    });

    test("tree in the middle of a long list", async ({ page }) => {
      await setTree(page, withLongTree());
      const tree = page.locator(".tree");
      await expect(
        tree.locator('.tree-item-container[data-index="0"]'),
      ).toBeVisible();
      await tree.evaluate(
        (el, top) => (el.scrollTop = top),
        2_500 * ROW_HEIGHT,
      );
      await expect(
        tree.locator('.tree-item-container[data-index="2505"]'),
      ).toBeVisible();
      await expectAccessible(page);
    });

    test("preview of a file", async ({ page }) => {
      await setAppState(page, withRepo());
      await showPreview(page);
      await expectAccessible(page);
    });

    test("generated output", async ({ page }) => {
      await setAppState(page, withRepo());
      await showGenerated(page);
      await expectAccessible(page);
    });

    test("while a directory is scanned", async ({ page }) => {
      const state = baseState();
      state.current_path = "/repo";
      state.is_scanning = true;
      state.is_fully_scanned = false;
      await setAppState(page, state);
      await page.evaluate(() => {
        (
          window as unknown as { updateScanProgress: (p: unknown) => void }
        ).updateScanProgress({
          files_scanned: 1234,
          large_files_skipped: 2,
          current_scanning_path: "/repo/src/lib/components",
        });
      });
      await expect(page.locator(".cancel-scan-btn")).toBeVisible();
      await expectAccessible(page);
    });

    test("while the output is generated", async ({ page }) => {
      const state = withRepo();
      state.is_generating = true;
      await setAppState(page, state);
      await expect(page.locator("#generate-btn")).toHaveClass(/is-generating/);
      await expectAccessible(page);
      // Under the pointer the button offers to cancel.
      await page.locator("#generate-btn").hover();
      await expectAccessible(page);
    });

    test("output settings open", async ({ page }) => {
      await setAppState(page, withRepo());
      await page.locator(".status-output-settings summary").click();
      await expect(page.locator(".status-output-settings")).toHaveAttribute(
        "open",
        "",
      );
      await expectAccessible(page);
    });

    test("keyboard only: Tab reaches every control and shows the focus", async ({
      page,
      browserName,
    }) => {
      await setAppState(page, withRepo());
      await showPreview(page);
      await expectKeyboardReach(page, browserName);
    });
  });
}
