# Changelog

## [0.3.1] - 2025-09-28

### Changed

- Replaced Icons to match squircle shape

## [0.3.0] — 2025-09-07

### Highlights

- **GUI migration to Svelte 5 (Runes) + Vite**: declarative components, strict TypeScript, and a single source of truth in Svelte stores.
- **25k node scalability**: virtualized file tree and deterministic E2E benchmarks gate performance (flatten/apply, filtered view, jump-to-end).
- **Typed IPC contracts**: runtime-validated boundaries with precise request/response schemas; safer error mapping from Rust.
- **Security**: production-only Content Security Policy injected at build time.
- **Developer docs**: Architecture, IPC, Virtualization Patterns, and Svelte Runes guidelines.

### Added

- Svelte 5 component suite (Header, Sidebar, FileTree, PreviewPanel, StatusBar, Footer, Toasts) with a11y roles and full keyboard navigation.
- Virtualization layer for large trees (windowed rendering, overscan, stable keys).
- **Benchmarks (Playwright)** up to ~25k nodes with env thresholds:
  - `BMARK_FLATTEN_MS` (default 1500)
  - `BMARK_FILTER_MS` (default 500)
  - `BMARK_SCROLL_MS` (default 200)
- **Bundle budgets** (Brotli) with category breakdown and CI failure on breach (`src/ui/scripts/check-budgets.mjs`).
- **E2E bridge** for deterministic UI tests (`$lib/dev/e2eBridge.ts`, `$lib/dev/e2eShim.ts`) and performance hooks (`$lib/dev/budget.ts`).
- **Idle helper** (`$lib/dev/idle.ts`) for low-priority chores via `requestIdleCallback` (no speculative prewarm).
- Developer documentation:
  - `docs/architecture.md`
  - `docs/ipc.md`
  - `docs/virtualization.md`
  - `docs/svelte-runes-guidelines.md`
  - `docs/README.md` (entry point)

### Changed

- Build outputs: stable chunk names (`entryFileNames`, `chunkFileNames`, `assetFileNames`) and explicit `manualChunks` for `monaco`.
- Web worker format set to `iife` for improved WebKit/WKWebView stability.
- UI state refactored into strict, typed stores; components consume state declaratively (Runes), removing imperative DOM updates.
- IPC error propagation normalized (`src/core/error.rs` → typed errors in UI).

### Performance

- Virtualized list renders **O(visible)** rows with bounded overscan.
- Benchmarks pass on CI and local runs (see `src/ui/e2e/benchmarks.spec.ts`); no artificial sleeps, only deterministic barriers.
- No “idle prewarm”: avoided speculative work; only backgrounding non-critical tasks.

### Security

- **CSP (production only)** injected by a Vite HTML transformer plugin:
  - `default-src 'none'; script-src 'self'; worker-src 'self' blob:; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'none'; upgrade-insecure-requests`
  - Not applied in `vite dev` to keep HMR working.

### Tests

- Unit tests for utilities, tree expansion memory, and components (Vitest).
- E2E flows (Playwright): directory selection → filter → preview → generate → save.
- Accessibility sanity checks with Axe; configurable strictness via `E2E_A11Y_STRICT`.

### Docs

- Architecture diagram (Mermaid) and detailed patterns for IPC, virtualization, and Svelte Runes usage.
- CI budgets doc (`docs/ci-budgets.md`) references the budget checker and thresholds.

### Build / CI

- `npm ci` + reproducible installs; strict ESLint 9 flat config and `svelte-check --fail-on-warnings` wired into checks.
- Budget checker prints app/workers/entry Brotli sizes and fails on violations.
- Release pipeline unchanged; Vite injects CSP only on `build`.

### Migration Notes

- **No breaking changes** expected for end users; behavior and shortcuts retained.
- If you develop locally: run `npm run dev` inside `src/ui` for HMR; CSP is not enforced in dev.
- If you embed UI into the Rust binary: `npm run build` first, then `cargo build --release`.

## [0.2.0] - 2025-08-04

### Added

- **Architecture Improvements** for large directories:
  - **Proactive Scanning**: Implemented a two-phase scan (shallow scan for instant UI, deep scan in the background) for a highly responsive experience.
  - **Lazy Loading**: Directory contents are now loaded on-demand when a folder is expanded, drastically reducing initial load times.
  - **UI Virtualization**: The file tree now uses virtual scrolling, allowing it to render tens of thousands of files smoothly without lagging.
- **Undo/Redo Support**: Added undo (`Ctrl/Cmd+Z`) and redo (`Ctrl/Cmd+Shift+Z`) functionality for all text input fields.
- **Background Indexing Indicator**: A subtle status indicator is now shown in the status bar while the deep scan is running in the background.

### Changed

- **Core Scanning Engine**: Replaced `walkdir` and `globset` with the highly optimized `ignore` crate. This provides native `.gitignore` support out-of-the-box and significantly improves scanning speed.
- **Keyboard Handling**: Refactored all frontend keyboard shortcuts into a robust command pattern, improving reliability and maintainability.

### Fixed

- **UI Performance**: Resolved major UI freezes and slowdowns that occurred when loading large or complex directories.
- **Application Stability**: Prevented potential application crashes caused by unhandled global keyboard shortcuts (e.g., `Escape`).
- **CI Build**: Corrected a Clippy warning (`too_many_arguments`) and added missing system dependencies to the GitHub Actions workflow to ensure successful builds.

### Removed

- Removed `walkdir` and `globset` dependencies from `Cargo.toml`.

## [0.1.1] - 2025-07-30

### Fixed

- Major performance optimization to prevent UI freezes on large directories.
- Instant UI feedback for newly added ignore patterns.
- Consistent file and folder counts in the UI status panel.

### Technical

- Improved code quality by resolving all clippy lints for a cleaner CI build.

## [0.1.0] - 2025-07-28

### Added

- Initial release of Context File Concatenator
- Directory selection with native file dialog and drag & drop
- Interactive file tree with selection capabilities
- File filtering by name, extension, and content search
- Gitignore-style pattern matching for file exclusion
- Syntax highlighting with Monaco editor integration
- Configuration import/export functionality
- ASCII directory tree generation in output
- Support for relative and absolute file paths
- Cross-platform desktop application (Windows, macOS, Linux)
- Professional release pipeline with automated builds
- Platform-specific icons (ICO, ICNS, PNG)

[Unreleased]: https://github.com/src-dbgr/context-file-concat/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/src-dbgr/context-file-concat/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/src-dbgr/context-file-concat/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/src-dbgr/context-file-concat/releases/tag/v0.1.0
