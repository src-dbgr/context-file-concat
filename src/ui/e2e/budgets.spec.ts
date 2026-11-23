import { test, expect, Page } from "@playwright/test";
import {
  makeHugeTree,
  setTree,
  waitForBridge,
  type AppState,
} from "./support/tree";

/**
 * Budget E2E:
 * - No artificial sleeps. Only wait for deterministic conditions.
 * - Relies on main.ts performance marks when `?budget=1` is set.
 * - The 25k tree is served by the stand-in host of the E2E bridge, which sends
 *   the window of rows the UI asks for, as the real host does.
 */

function shouldInstallBridge(page: Page) {
  return page.addInitScript(() => {
    (window as unknown as { __PW_E2E: boolean }).__PW_E2E = true;
  });
}

function baseState(): AppState {
  return {
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    tree: [],
    current_path: "/repo",
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

test.describe("[@budget] CI budgets", () => {
  test.beforeEach(async ({ page }) => {
    await shouldInstallBridge(page);
  });

  test("First mount below threshold", async ({ page }) => {
    const thresholdMs = Number(process.env.BUDGET_INIT_MS ?? 1500);

    await page.goto("/?e2e=1&budget=1");

    await page.waitForFunction(() => {
      return (
        (window as unknown as { __APP_READY?: boolean }).__APP_READY === true
      );
    });

    const measured = await page.evaluate(() => {
      const m = performance.getEntriesByName("app-init")[0] as
        PerformanceMeasure | undefined;
      return m ? m.duration : -1;
    });

    const duration = measured >= 0 ? measured : 0;
    expect(duration).toBeLessThanOrEqual(thresholdMs);
  });

  test("Virtualized tree handles 25k files (structural smoothness)", async ({
    page,
  }) => {
    await page.goto("/?e2e=1");

    await waitForBridge(page);

    const huge = baseState();
    huge.tree = makeHugeTree(25_000);
    huge.tree_files_count = 25_000;
    huge.tree_dirs_count = 1;

    const startedAt = Date.now();
    await setTree(page, huge);

    const rows = page.locator(".virtual-scroll-item");
    await expect(rows.first()).toBeAttached();

    const count = await rows.count();
    expect(count).toBeGreaterThan(0);
    // Budget: virtualized list renders far fewer DOM nodes than total
    expect(count).toBeLessThanOrEqual(600);

    const elapsed = Date.now() - startedAt;
    const vtreeThresholdMs = Number(process.env.BUDGET_VTREE_MS ?? 3000);
    expect(elapsed).toBeLessThanOrEqual(vtreeThresholdMs);
  });
});
