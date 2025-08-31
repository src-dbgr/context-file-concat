# CI Budgets (Step 4.5)

This project enforces **bundle** and **runtime** budgets in CI.

## Bundle budgets

We split built assets into:

- **ENTRY** – `index.js` (main entry).
- **APP** – all non-worker JS/CSS (application code).
- **WORKERS** – `*.worker-*.js` (Monaco workers: ts/css/html/json/editor).

**Defaults (Brotli):**

- ENTRY ≤ **0.8 MB**
- APP total ≤ **1.0 MB**
- WORKERS total ≤ **1.5 MB**

Configure via environment variables (bytes):

````bash
ENTRY_BROTLI=800000
APP_TOTAL_BROTLI=1000000
WORKER_TOTAL_BROTLI=1500000
# Optional legacy grand total cap:
# BUDGET_TOTAL_BROTLI=2000000

## Runtime budgets (E2E)

E2E tests under `src/ui/e2e/budgets.spec.ts` verify:

1. **First mount time** – measured by `performance.measure('app-init')` in `main.ts` when `?budget=1` is present.
2. **Virtualization smoothness** – with a 25k item file tree, we assert the virtualized DOM rows are ≤ 600 (structural proxy for smoothness).

Env overrides:

- `BUDGET_INIT_MS` (default **1500** ms)
- `BUDGET_VTREE_MS` (default **3000** ms)

> Local runs on Apple Silicon can use stricter values, e.g.
> `BUDGET_INIT_MS=200 npm run test:e2e:budgets`

## How to run locally

```bash
cd src/ui
npm ci
npm run build
npm run budget:bundle

# E2E budget tests (use the same Playwright setup)
npx playwright install --with-deps chromium
npm run test:e2e:budgets
````

## Boot-Sequenz (Mermaid)

```mermaid
sequenceDiagram
  autonumber
  actor Tester as Playwright/E2E
  participant Browser
  participant HTML as index.html
  participant Main as src/ui/src/main.ts
  participant Budget as $lib/dev/budget.ts
  participant Shim as $lib/dev/e2eShim.ts
  participant Bridge as $lib/dev/e2eBridge (opt.)
  participant IPC as Window IPC handlers
  participant Svelte as Svelte mounts
  participant Editor as $lib/modules/editor
  participant Keys as $lib/modules/keyboard
  participant Backend as post("initialize")

  Browser->>HTML: Lade HTML + early theme bootstrap
  HTML->>Main: Lade ES Module (main.ts)

  rect rgb(245,245,245)
    Main->>Budget: isBudgetMode()?
    alt budget=1
      Main->>Budget: markScriptStart()
      Main->>Budget: scheduleEarlyReadyFallback()<br/>(queueMicrotask → __APP_READY=true, marks)
    else
      note right of Budget: Budget-Pfad inaktiv
    end
  end

  Main->>Shim: ensureE2EShim(appState.set, getState)
  alt E2E erlaubt (dev | ?e2e=1 | __PW_E2E)
    Main->>Shim: installE2EBridgeIfAllowed()
    Shim-->>Bridge: dynamic import + install()
  else
    note over Shim,Bridge: Kein Bridge-Import in normaler Prod-Nutzung
  end

  Main->>Svelte: mount(App, Header, Sidebar, FileTree, PreviewPanel, Footer)
  Main->>IPC: attachWindowIPCHandlers(window)

  par DOM ready
    HTML-->>Main: DOMContentLoaded (oder readyState != "loading")
    Main->>Main: initialize()
  and E2E
    Tester-->>Shim: __e2e.store.setAppState(...)
  end

  rect rgb(245,245,245)
    alt budget=1
      Main->>Budget: markInitStart()
    end
    Main->>Editor: initEditor(cb)
    Editor-->>Main: Monaco ready
    Main->>Keys: setupGlobalKeyboardListeners()
    Main->>Backend: post("initialize")
    Main->>Main: beforeunload cleanup
    alt budget=1
      Main->>Budget: markReadyAndMeasureOnce()
    end
  end

  note over Tester,Main: E2E wartet deterministisch auf __APP_READY bzw. __e2e.store

  rect rgb(235,245,255)
    participant Rust as Rust/WebView Backend
    Rust-->>IPC: window.render(...) / updateScanProgress(...) / showPreviewContent(...)
    IPC->>Svelte: appState.set()/update → reaktives UI
  end
```
