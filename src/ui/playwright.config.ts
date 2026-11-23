// src/ui/playwright.config.ts
import { defineConfig, devices } from "@playwright/test";

// One preview server per run. `E2E_PORT` lets several checkouts (or agents)
// run the suite at the same time without sharing a server; `--strictPort`
// makes a taken port fail loudly instead of silently moving elsewhere.
const port = Number(process.env.E2E_PORT ?? 4173);

export default defineConfig({
  testDir: "./e2e",
  timeout: 30_000,
  expect: { timeout: 5_000 },
  fullyParallel: true,

  reporter: [
    ["list"],
    ["html", { open: "never", outputFolder: "playwright-report" }],
    ["blob", { outputDir: "blob-report" }],
  ],

  use: {
    baseURL: `http://localhost:${port}`,
    trace: "retain-on-failure",
    video: "retain-on-failure",
    screenshot: "only-on-failure",
  },

  webServer: {
    command: `npm run preview:e2e -- --port ${port}`,
    port,
    // Locally a running server may be reused (fast reruns); in CI never, so a
    // stale server of another job can't be tested by accident.
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },

  // The app runs in the system webview: WebKit on macOS (WKWebView) and Linux
  // (WebKitGTK), Chromium on Windows (WebView2). Timing budgets and benchmarks
  // (@budget, @bench) are calibrated for Chromium only; see package.json.
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
    {
      name: "webkit",
      use: { ...devices["Desktop Safari"] },
    },
  ],
});
