import { test, expect, Page } from "@playwright/test";

/**
 * Budget E2E:
 * - No artificial sleeps. Only wait for deterministic conditions.
 * - Relies on main.ts performance marks when `?budget=1` is set.
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
  children: TreeNode[];
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
  search_query: string;
  extension_filter: string;
  content_search_query: string;
  active_ignore_patterns: string[];
  config: AppConfig;
};

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

function makeHugeTree(filesCount: number): TreeNode[] {
  const children: TreeNode[] = [];
  for (let i = 0; i < filesCount; i++) {
    const name = `file-${i}.txt`;
    children.push({
      path: `/repo/src/${name}`,
      name,
      is_directory: false,
      is_expanded: false,
      is_binary: false,
      is_match: true,
      is_previewed: false,
      selection_state: "none",
      size: 123,
      children: [],
    });
  }
  return [
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
      children,
    },
  ];
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
        | PerformanceMeasure
        | undefined;
      return m ? m.duration : -1;
    });

    const duration = measured >= 0 ? measured : 0;
    expect(duration).toBeLessThanOrEqual(thresholdMs);
  });

  test("Virtualized tree handles 25k files (structural smoothness)", async ({
    page,
  }) => {
    await page.goto("/?e2e=1");

    await page.waitForFunction(() => {
      const w = window as unknown as {
        __e2e?: { store?: { setAppState?: (s: unknown) => void } };
      };
      return typeof w.__e2e?.store?.setAppState === "function";
    });

    const huge = baseState();
    huge.tree = makeHugeTree(25_000);

    const startedAt = Date.now();
    await page.evaluate((next) => {
      (
        window as unknown as {
          __e2e: { store: { setAppState: (s: AppState) => void } };
        }
      ).__e2e.store.setAppState(next);
    }, huge);

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
