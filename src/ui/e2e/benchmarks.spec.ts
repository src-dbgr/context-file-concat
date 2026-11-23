import { test, expect, Page } from "@playwright/test";
import {
  makeHugeTree,
  setTree,
  waitForBridge,
  type AppState,
} from "./support/tree";

/**
 * Benchmarks (dev-only):
 * - Keine künstlichen Waits. Es wird nur auf deterministische Bedingungen gewartet.
 * - Nutzt window.__e2e.host.setTree (siehe e2eBridge; nur im e2e-Build vorhanden):
 *   die Bridge hält die Zeilen und liefert wie der Host nur das Fenster, das
 *   die UI anfragt (setViewport).
 * - Misst mit Performance Marks/Measures im Browser.
 *
 * Thresholds via ENV:
 *   BMARK_FLATTEN_MS  (default 1800)
 *   BMARK_FILTER_MS   (default 600)
 *   BMARK_SCROLL_MS   (default 300)
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

async function waitForVirtualListReady(page: Page) {
  // „bereit“ = erste Reihe existiert und Virtual-Count ist plausibel begrenzt
  const rows = page.locator(".virtual-scroll-item");
  await expect(rows.first()).toBeAttached();
  // Rendered DOM-Zeilen << Gesamtelemente ⇒ Virtualisierung aktiv
  const count = await rows.count();
  expect(count).toBeGreaterThan(0);
  expect(count).toBeLessThanOrEqual(600);
}

test.describe("[@bench] 25k Benchmarks", () => {
  test.beforeEach(async ({ page }) => {
    await shouldInstallBridge(page);
  });

  test("Flatten/Apply 25k Nodes ≤ threshold", async ({ page }) => {
    const THRESH = Number(process.env.BMARK_FLATTEN_MS ?? 1800);

    await page.goto("/?e2e=1");

    // Bridge verfügbar?
    await waitForBridge(page);

    const huge = baseState();
    huge.tree = makeHugeTree(25_000);
    huge.tree_files_count = 25_000;
    huge.tree_dirs_count = 1;

    // Start messen + apply
    await page.evaluate(() => performance.mark("bm_flatten_start"));
    await setTree(page, huge);

    await waitForVirtualListReady(page);

    const duration = await page.evaluate(() => {
      performance.mark("bm_flatten_end");
      performance.measure("bm_flatten", "bm_flatten_start", "bm_flatten_end");
      const m = performance.getEntriesByName("bm_flatten")[0] as
        PerformanceMeasure | undefined;
      return m ? m.duration : -1;
    });

    expect(duration).toBeGreaterThanOrEqual(0);
    expect(duration).toBeLessThanOrEqual(THRESH);
  });

  test("Filter-Anwendung (Teilmenge ~1k Items) ≤ threshold", async ({
    page,
  }) => {
    const THRESH = Number(process.env.BMARK_FILTER_MS ?? 600);

    await page.goto("/?e2e=1");
    await waitForBridge(page);

    // Zuerst 25k setzen
    const huge = baseState();
    huge.tree = makeHugeTree(25_000);
    huge.tree_files_count = 25_000;
    huge.tree_dirs_count = 1;
    await setTree(page, huge);
    await waitForVirtualListReady(page);

    // Jetzt eine gefilterte Teilmenge (~1000 Dateien: "file-24xxx")
    const filtered = baseState();
    const all = makeHugeTree(25_000)[0]; // nur das "src"-Verzeichnis
    filtered.search_query = "file-24";
    filtered.tree = [
      {
        ...all,
        children: all.children.filter((c) => /file-24\d{2}\.txt$/.test(c.name)),
      },
    ];

    filtered.tree_files_count = filtered.tree[0].children.length;
    filtered.tree_dirs_count = 1;

    await page.evaluate(() => performance.mark("bm_filter_start"));
    await setTree(page, filtered);

    // „bereit“, plus Sicherheits-Check: sichtbare Namen enthalten den Filter
    await waitForVirtualListReady(page);
    const allVisibleMatch = await page.evaluate(() => {
      const nodes = Array.from(
        document.querySelectorAll(".tree .file-item .file-name"),
      );
      return nodes.every((n) => (n.textContent || "").includes("file-24"));
    });
    expect(allVisibleMatch).toBe(true);

    const duration = await page.evaluate(() => {
      performance.mark("bm_filter_end");
      performance.measure("bm_filter", "bm_filter_start", "bm_filter_end");
      const m = performance.getEntriesByName("bm_filter")[0] as
        PerformanceMeasure | undefined;
      return m ? m.duration : -1;
    });

    expect(duration).toBeGreaterThanOrEqual(0);
    expect(duration).toBeLessThanOrEqual(THRESH);
  });

  test("Scroll-Jump ans Ende (25k) ≤ threshold", async ({ page }) => {
    const THRESH = Number(process.env.BMARK_SCROLL_MS ?? 300);

    await page.goto("/?e2e=1");
    await waitForBridge(page);

    const huge = baseState();
    huge.tree = makeHugeTree(25_000);
    huge.tree_files_count = 25_000;
    huge.tree_dirs_count = 1;
    await setTree(page, huge);
    await waitForVirtualListReady(page);

    // Scroll-Jump messen: bis die letzte Datei sichtbar ist
    const duration = await page.evaluate(async () => {
      const container = document.querySelector('[role="tree"]') as HTMLElement;
      if (!container) return -1;

      performance.mark("bm_scroll_start");
      container.scrollTop = container.scrollHeight;

      const target = "file-24999.txt";
      // Auf Darstellung des letzten Items warten (deterministisch via rAF + query)
      const ok = await new Promise<boolean>((resolve) => {
        let tries = 0;
        const tick = () => {
          tries++;
          const match = Array.from(
            document.querySelectorAll(".tree .file-item .file-name"),
          ).some((n) => (n.textContent || "").includes(target));
          if (match) return resolve(true);
          if (tries > 60) return resolve(false); // ~1 Sekunde Worst-Case
          requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
      });

      performance.mark("bm_scroll_end");
      performance.measure("bm_scroll", "bm_scroll_start", "bm_scroll_end");
      const m = performance.getEntriesByName("bm_scroll")[0] as
        PerformanceMeasure | undefined;
      return ok && m ? m.duration : -1;
    });

    expect(duration).toBeGreaterThanOrEqual(0);
    expect(duration).toBeLessThanOrEqual(THRESH);

    // Nach dem Jump: Virtualisierung weiter im Rahmen?
    await waitForVirtualListReady(page);
  });
});
