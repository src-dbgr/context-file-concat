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
    ["blob", { outputDir: "blob-report" }],
  ],

  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    video: "retain-on-failure",
    screenshot: "only-on-failure",
  },

  webServer: {
    command: "npm run preview:e2e",
    port: 4173,
    reuseExistingServer: true,
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
