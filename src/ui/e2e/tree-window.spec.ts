import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  fileNode,
  setTree,
  waitForBridge,
  type AppState,
  type TreeNode,
} from "./support/tree";

/**
 * The file tree with a list of rows that is far longer than one window.
 *
 * The UI holds a window of the rows and asks for another one when the viewport
 * moves (`setViewport`), or for the position of a row it does not hold
 * (`locateRow`). The stand-in host of the E2E bridge answers like the real
 * one, so these tests cover the real scrolling, focus and layout of a browser:
 * mouse wheel, scroll jumps, keyboard navigation across the edge of the
 * window, type-ahead, and the accessibility of the result.
 */

const FILES = 5_000;
const ROW_HEIGHT = 28;

function bigTree(): TreeNode[] {
  const files: TreeNode[] = [];
  for (let i = 0; i < FILES; i++) {
    files.push(fileNode(`/repo/src/file-${String(i).padStart(4, "0")}.txt`));
  }
  return [
    {
      ...fileNode("/repo/src", 0),
      is_directory: true,
      is_expanded: true,
      has_children: true,
      children: files,
    },
    fileNode("/repo/zebra.md"),
  ];
}

function state(): AppState {
  return {
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    tree: bigTree(),
    current_path: "/repo",
    current_config_filename: null,
    status_message: "Status: Ready.",
    selected_files_count: 0,
    tree_files_count: FILES + 1,
    tree_dirs_count: 1,
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

type Posted = { command: string; payload: unknown };

/** Rows of the whole list: `src`, its files, `zebra.md`. */
const TOTAL_ROWS = FILES + 2;
const LAST_ROW = TOTAL_ROWS - 1;

const rowAt = (page: Page, index: number) =>
  page.locator(`.tree .tree-item-container[data-index="${index}"]`);

async function posted(page: Page, command: string): Promise<Posted[]> {
  return page.evaluate(
    (name) =>
      (window as unknown as { __posted: string[] }).__posted
        .map((message) => JSON.parse(message) as Posted)
        .filter((message) => message.command === name),
    command,
  );
}

test.beforeEach(async ({ page }) => {
  // Record what the UI posts; the stand-in host listens as well.
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
  await setTree(page, state());
  await expect(rowAt(page, 0)).toBeVisible();
});

test("renders a screen of rows out of a long list", async ({ page }) => {
  const sizer = page.locator(".tree .virtual-scroll-sizer");
  await expect(sizer).toHaveCSS("height", `${TOTAL_ROWS * ROW_HEIGHT}px`);

  const rendered = await page.locator(".tree .virtual-scroll-item").count();
  expect(rendered).toBeGreaterThan(5);
  expect(rendered).toBeLessThan(200);
  await expect(rowAt(page, 0)).toHaveAttribute("data-path", "/repo/src");
  await expect(rowAt(page, 1)).toHaveAttribute("aria-level", "2");
});

test("scrolling brings in the rows of the new position", async ({ page }) => {
  const tree = page.locator(".tree");

  // A jump far away from the window the UI holds.
  await tree.evaluate((el, top) => (el.scrollTop = top), 3_000 * ROW_HEIGHT);
  await expect(rowAt(page, 3_005)).toBeVisible();
  await expect(rowAt(page, 3_005)).toHaveAttribute(
    "data-path",
    "/repo/src/file-3004.txt",
  );
  // Each row sits where its position in the list puts it.
  const top = await rowAt(page, 3_005).evaluate(
    (el) => (el.parentElement as HTMLElement).style.top,
  );
  expect(top).toBe(`${3_005 * ROW_HEIGHT}px`);

  // To the very end, and with the wheel back up a little.
  await tree.evaluate((el) => (el.scrollTop = el.scrollHeight));
  await expect(rowAt(page, LAST_ROW)).toBeVisible();
  await expect(rowAt(page, LAST_ROW)).toHaveAttribute(
    "data-path",
    "/repo/zebra.md",
  );
  await tree.hover();
  await page.mouse.wheel(0, -40 * ROW_HEIGHT);
  await expect(rowAt(page, LAST_ROW - 45)).toBeVisible();

  // The DOM stays a screen of rows, and the UI asked the host for windows
  // instead of holding the list.
  expect(await page.locator(".tree .virtual-scroll-item").count()).toBeLessThan(
    200,
  );
  const requests = await posted(page, "setViewport");
  expect(requests.length).toBeGreaterThan(1);
  for (const request of requests) {
    const { count } = request.payload as { start: number; count: number };
    expect(count).toBeLessThanOrEqual(600);
  }
});

test("keyboard navigation crosses the edge of the window", async ({ page }) => {
  await rowAt(page, 0).focus();
  await expect(rowAt(page, 0)).toBeFocused();

  await page.keyboard.press("End");
  await expect(rowAt(page, LAST_ROW)).toBeFocused();
  await expect(rowAt(page, LAST_ROW)).toBeInViewport();

  await page.keyboard.press("ArrowUp");
  await expect(rowAt(page, LAST_ROW - 1)).toBeFocused();

  // The parent directory of this file is 5,000 rows above the window.
  await page.keyboard.press("ArrowLeft");
  await expect(rowAt(page, 0)).toBeFocused();
  await expect(rowAt(page, 0)).toBeInViewport();
  expect(await posted(page, "locateRow")).toEqual([
    {
      command: "locateRow",
      payload: {
        request: 1,
        target: { kind: "parent_of", path: "/repo/src/file-4999.txt" },
      },
    },
  ]);

  // Page by page down, further than the first window reaches.
  for (let i = 0; i < 12; i++) await page.keyboard.press("PageDown");
  const focused = page.locator(".tree .tree-item-container:focus");
  await expect(focused).toHaveCount(1);
  const index = Number(await focused.getAttribute("data-index"));
  expect(index).toBeGreaterThan(100);
  await expect(focused).toBeInViewport();

  await page.keyboard.press("Home");
  await expect(rowAt(page, 0)).toBeFocused();
});

test("typing finds a row the UI does not hold", async ({ page }) => {
  await rowAt(page, 0).focus();

  await page.keyboard.type("ze");

  await expect(rowAt(page, LAST_ROW)).toBeFocused();
  await expect(rowAt(page, LAST_ROW)).toHaveAttribute(
    "data-path",
    "/repo/zebra.md",
  );
  await expect(rowAt(page, LAST_ROW)).toBeInViewport();

  // Space acts on the row that was found.
  await page.keyboard.press("Space");
  expect(await posted(page, "toggleSelection")).toEqual([
    { command: "toggleSelection", payload: "/repo/zebra.md" },
  ]);
});

test("a click far down the list acts on the row under the pointer", async ({
  page,
}) => {
  const tree = page.locator(".tree");
  await tree.evaluate((el, top) => (el.scrollTop = top), 2_000 * ROW_HEIGHT);
  const row = rowAt(page, 2_003);
  await expect(row).toBeVisible();

  await row.locator('input[type="checkbox"]').click();
  await row.locator(".file-name").click();

  expect(await posted(page, "toggleSelection")).toEqual([
    { command: "toggleSelection", payload: "/repo/src/file-2002.txt" },
  ]);
  expect(await posted(page, "loadFilePreview")).toEqual([
    { command: "loadFilePreview", payload: "/repo/src/file-2002.txt" },
  ]);
});

test("the tree has no critical a11y issue in the middle of a long list", async ({
  page,
}) => {
  const tree = page.locator(".tree");
  await tree.evaluate((el, top) => (el.scrollTop = top), 2_500 * ROW_HEIGHT);
  await expect(rowAt(page, 2_505)).toBeVisible();

  const results = await new AxeBuilder({ page }).include(".tree").analyze();
  const critical = results.violations.filter((v) => v.impact === "critical");
  expect(critical.map((violation) => violation.id)).toEqual([]);
});
