# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

-

### Changed

-

### Deprecated

-

### Removed

-

### Fixed

-

### Security

-

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
