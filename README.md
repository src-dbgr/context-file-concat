# CFC - Context File Concatenator

CFC (Context File Concatenator) is a desktop application designed to intelligently select, filter, and combine project files into a single text file. This output is optimized for use with Large Language Models (LLMs), providing them with the necessary context to understand and analyze a codebase.

![CFC Screenshot](placeholder.png)

## Key Features

- **Directory Selection**: Easily select a project directory using a native file dialog or by dragging and dropping it into the application.
- **Interactive File Tree**: View your project structure in a familiar tree view. Select or deselect individual files or entire directories.
- **Flexible Filtering**:
  - Filter files by name (case-sensitive or insensitive).
  - Filter by file extension (e.g., show only `.rs` or `.py` files).
  - Search for text content within files.
- **Powerful Ignore System**:
  - Uses `.gitignore`-style patterns to exclude unwanted files and directories (like `node_modules/`, `target/`, or `*.log`).
  - Add and remove patterns dynamically.
  - Right-click any file or folder in the tree to ignore it instantly.
- **Syntax Highlighting**: Preview individual files or the final concatenated output with syntax highlighting in a built-in Monaco editor.
- **Configuration Management**:
  - Import and export your settings (including ignore patterns) as a JSON file to share configurations across projects or teams.
  - Settings are automatically saved between sessions.
- **Customizable Output**:
  - Choose to include an ASCII directory tree at the start of the output file.
  - Use relative or absolute file paths in the output headers.

## Architecture Overview

CFC uses a modern hybrid architecture that combines a powerful Rust backend with a web-based frontend for a flexible and performant user experience.

- **Backend (Rust)**: The core logic is written in Rust, leveraging its performance, safety, and concurrency features. It handles all file system operations, scanning, filtering, and content processing. The backend is structured into three main layers:

  - `core`: Contains the pure, reusable business logic (scanning, filtering, file handling) with no dependencies on the application or UI layers. It uses a robust, typed error handling system with `thiserror`.
  - `app`: Acts as the orchestrator, managing the application state, handling events from the UI, and executing commands.
  - `main.rs`: The entry point that sets up the window, WebView, and event loop.

- **Frontend (HTML/CSS/JS)**: The user interface is built with standard web technologies and rendered in a WebView.
  - **Wry & Tao**: We use the `wry` crate for the WebView and `tao` for windowing and the event loop, providing a lightweight cross-platform solution.
  - **Monaco Editor**: The same editor that powers VS Code is integrated for a high-quality code preview experience.
  - **IPC**: The frontend communicates with the Rust backend via an Inter-Process Communication (IPC) channel, sending JSON messages to trigger commands and receiving events to update the UI.

## Building from Source

To build and run the application locally, you need [Node.js/npm](https://nodejs.org/) and the [Rust toolchain](https://www.rust-lang.org/tools/install).

1.  **Prepare the Frontend Dependencies:**
    Navigate to the `src/ui` directory and install the necessary JavaScript packages.

    ```bash
    cd src/ui
    npm install
    ```

2.  **Build the Frontend Bundle:**
    This command uses `esbuild` to bundle all JavaScript modules into a single file that will be injected into the HTML.

    ```bash
    npm run build
    ```

    For development, you can use `npm run watch` to have `esbuild` automatically rebuild the bundle whenever you change a frontend file.

3.  **Run the Rust Application:**
    Navigate back to the project root directory and run the application using Cargo. This will build and launch the desktop application.
    ```bash
    cd ../..
    cargo run
    ```
