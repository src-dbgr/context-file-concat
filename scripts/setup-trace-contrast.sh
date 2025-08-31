#!/usr/bin/env bash
set -euo pipefail

# Run from repo root (context-file-concat/)
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

UI_DIR="$ROOT_DIR/src/ui"
E2E_DIR="$UI_DIR/e2e"
CONFIG_FILE="$UI_DIR/playwright.config.ts"
SPEC_FILE="$E2E_DIR/a11y-contrast.spec.ts"
GITIGNORE_FILE="$UI_DIR/.gitignore"

mkdir -p "$E2E_DIR"

# --- Write/Update playwright.config.ts (backup once) ---
if [[ -f "$CONFIG_FILE" && ! -f "$CONFIG_FILE.bak" ]]; then
  cp "$CONFIG_FILE" "$CONFIG_FILE.bak"
fi

cat > "$CONFIG_FILE" <<'TS'
// src/ui/playwright.config.ts
import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,

  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "playwright-report" }],
    ["blob", { outputDir: "blob-report" }]
  ],

  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    video: "retain-on-failure",
    screenshot: "only-on-failure"
  },

  webServer: {
    command: "npm run preview:e2e",
    port: 4173,
    reuseExistingServer: true,
    timeout: 120_000
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] }
    }
  ]
});
TS

# --- Write new a11y contrast-only spec ---
cat > "$SPEC_FILE" <<'TS'
// src/ui/e2e/a11y-contrast.spec.ts
import { test, expect } from "@playwright/test";
import axe from "axe-core";

/**
 * Runs a focused Axe check for "color-contrast".
 * Policy: allow "serious" (for incremental remediation), but fail on "critical".
 * No timeouts or sleeps; render + run in-place.
 */
test("Color contrast report (no *critical* issues)", async ({ page }) => {
  // Use e2e mode (harmless for this test; ensures stable app shell)
  await page.goto("/?e2e=1");

  // Inject axe-core into the page
  await page.addScriptTag({ content: axe.source });

  type AxeImpact = "minor" | "moderate" | "serious" | "critical" | null;
  type AxeViolation = { id: string; impact: AxeImpact; nodes: unknown[] };
  type AxeResults = { violations: AxeViolation[] };

  const results = (await page.evaluate(async () => {
    // Axe is exposed on window after addScriptTag
    // Keep types minimal to avoid 'any'
    const w = window as unknown as {
      axe: { run: (root: Document, options: unknown) => Promise<unknown> };
    };
    const r = await w.axe.run(document, {
      runOnly: { type: "rule", values: ["color-contrast"] },
      reporter: "v2"
    });
    return r;
  })) as AxeResults;

  // Compact console summary for devs (id/impact/count)
  // eslint-disable-next-line no-console
  console.log(
    JSON.stringify(
      results.violations.map((v) => ({
        id: v.id,
        impact: v.impact,
        nodes: v.nodes.length
      })),
      null,
      2
    )
  );

  const critical = results.violations.filter((v) => v.impact === "critical");
  expect(
    critical.length,
    "No critical color-contrast violations expected"
  ).toBe(0);
});
TS


echo "✅ Playwright traces/videos (on failure) enabled, contrast test added."
echo "   - Config:    $CONFIG_FILE"
echo "   - New test:  $SPEC_FILE"
echo
echo "Run from src/ui/:"
echo "  npm run check:clean"
