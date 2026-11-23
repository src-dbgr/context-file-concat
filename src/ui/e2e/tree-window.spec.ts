import { test, expect, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import {
  fileNode,
  holdHost,
  releaseHost,
  setHostLatency,
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

test.describe("with a host that takes its time to answer", () => {
  const LATENCY_MS = 120;

  test.beforeEach(async ({ page }) => {
    await setHostLatency(page, LATENCY_MS);
    await rowAt(page, 0).focus();
    await expect(rowAt(page, 0)).toBeFocused();
  });

  /** `true` if the focus is on the tree or inside it (not lost to the body). */
  const focusIsInTree = (page: Page) =>
    page.evaluate(() => {
      const tree = document.querySelector(".tree");
      return !!tree && tree.contains(document.activeElement);
    });

  test("40 times PageDown lands on the right row", async ({ page }) => {
    const rowsPerPage = await page
      .locator(".tree")
      .evaluate(
        (el, height) => Math.floor(el.clientHeight / height),
        ROW_HEIGHT,
      );
    expect(rowsPerPage).toBeGreaterThan(3);

    // Far faster than the host answers: most rows the focus passes never
    // arrive, and the focused row is not in the DOM for most of the keys.
    for (let i = 0; i < 40; i++) {
      await page.keyboard.press("PageDown");
      expect(await focusIsInTree(page), `after ${i + 1} keys`).toBe(true);
    }

    const target = 40 * rowsPerPage;
    await expect(rowAt(page, target)).toBeFocused();
    await expect(rowAt(page, target)).toBeInViewport();
    await expect(rowAt(page, target)).toHaveAttribute(
      "data-path",
      `/repo/src/file-${String(target - 1).padStart(4, "0")}.txt`,
    );
  });

  test("End, then ArrowUp: the key after the jump is not lost", async ({
    page,
  }) => {
    await page.keyboard.press("End");
    await page.keyboard.press("ArrowUp");
    expect(await focusIsInTree(page)).toBe(true);

    await expect(rowAt(page, LAST_ROW - 1)).toBeFocused();
  });

  test("End, then Space: the key acts on the row once it is there", async ({
    page,
  }) => {
    await page.keyboard.press("End");
    // The rows at the end are still placeholders.
    await expect(page.locator(".tree")).toHaveAttribute("aria-busy", "true");
    await expect(
      page.locator(".tree .virtual-scroll-item.pending").first(),
    ).toBeVisible();
    await page.keyboard.press("Space");
    expect(await posted(page, "toggleSelection")).toEqual([]);

    await expect(rowAt(page, LAST_ROW)).toBeFocused();
    await expect(page.locator(".tree")).toHaveAttribute("aria-busy", "false");
    await expect(
      page.locator(".tree .virtual-scroll-item.pending"),
    ).toHaveCount(0);
    expect(await posted(page, "toggleSelection")).toEqual([
      { command: "toggleSelection", payload: "/repo/zebra.md" },
    ]);
  });

  test("a key kept for a row is never carried out late", async ({ page }) => {
    const tree = page.locator(".tree");

    // End, Space, Home: the Space is kept for the last row. Before the host
    // answers, the viewport moves on and reports another position, so the
    // rows of the end never arrive (the answer is overtaken). The host is
    // held meanwhile: no answer comes in between the keys.
    await holdHost(page);
    await page.keyboard.press("End");
    await page.keyboard.press("Space");
    await page.keyboard.press("Home");
    await tree.evaluate((el, top) => (el.scrollTop = top), 2_000 * ROW_HEIGHT);
    await expect(
      page.locator(".tree .virtual-scroll-item.pending").first(),
    ).toBeVisible();
    await releaseHost(page);
    await expect(rowAt(page, 2_005)).toBeVisible();
    await expect(tree).toHaveAttribute("aria-busy", "false");
    expect(await posted(page, "toggleSelection")).toEqual([]);

    // Seconds later the user scrolls to the end with the mouse.
    const since = await page.evaluate(() => performance.now());
    await page.waitForFunction((t) => performance.now() - t > 1200, since);
    await tree.evaluate((el) => (el.scrollTop = el.scrollHeight));
    await expect(rowAt(page, LAST_ROW)).toBeVisible();
    await expect(tree).toHaveAttribute("aria-busy", "false");

    // No selection is toggled without a key press.
    expect(await posted(page, "toggleSelection")).toEqual([]);
  });

  test("typing, then ArrowUp: the key acts from the row that is found", async ({
    page,
  }) => {
    await page.keyboard.type("ze");
    await page.keyboard.press("ArrowUp");

    await expect(rowAt(page, LAST_ROW - 1)).toBeFocused();
    await expect(rowAt(page, LAST_ROW - 1)).toBeInViewport();
  });

  test("placeholders take exactly the place of the rows", async ({ page }) => {
    const tree = page.locator(".tree");
    // The placeholders stay for as long as the host is held.
    await holdHost(page);
    await tree.evaluate((el, top) => (el.scrollTop = top), 3_000 * ROW_HEIGHT);

    const placeholder = page
      .locator(".tree .virtual-scroll-item.pending")
      .first();
    await expect(placeholder).toBeVisible();
    const before = await placeholder.evaluate((el) => ({
      top: (el as HTMLElement).style.top,
      height: el.getBoundingClientRect().height,
    }));
    expect(before.height).toBe(ROW_HEIGHT);
    const scrollTop = await tree.evaluate((el) => el.scrollTop);

    await releaseHost(page);
    await expect(rowAt(page, 3_005)).toBeVisible();
    // Nothing has moved: same scroll position, the row where its placeholder was.
    expect(await tree.evaluate((el) => el.scrollTop)).toBe(scrollTop);
    const index = parseInt(before.top, 10) / ROW_HEIGHT;
    const row = rowAt(page, index);
    await expect(row).toBeVisible();
    expect(
      await row.evaluate(
        (el) => el.parentElement!.getBoundingClientRect().height,
      ),
    ).toBe(ROW_HEIGHT);
  });
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
  const critical = results.violations.filter(
    (v) => v.impact === "critical" || v.impact === "serious",
  );
  expect(critical.map((violation) => violation.id)).toEqual([]);
});
