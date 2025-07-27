<table>
<tr>
<td><img src="assets/text_flash_logo.svg" alt="CFC Logo" width="100"/></td>
<td><h1>Context File Concatenator</h1></td>
</tr>
</table>

CFC (Context File Concatenator) is a desktop application designed to intelligently select, filter, and combine project files into a single text file. This output is optimized for use with Large Language Models (LLMs), providing them with the necessary context to understand and analyze a codebase.

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
  - Click the ignore button next to any file or folder in the tree to ignore it instantly.
- **Syntax Highlighting**: Preview individual files or the final concatenated output with syntax highlighting in a built-in Monaco editor.
- **Configuration Management**:
  - Import and export your settings (including ignore patterns) as a JSON file to share configurations across projects or teams.
  - Settings are automatically saved between sessions.
- **Customizable Output**:
  - Choose to include an ASCII directory tree at the start of the output file.
  - Use relative or absolute file paths in the output headers.

---

## Architecture & Design

CFC uses a modern hybrid architecture that combines a powerful Rust backend with a web-based frontend. The design emphasizes **modularity, testability, and robustness**.

- **Backend (Rust)**: The core logic is written in Rust, leveraging its performance, safety, and concurrency features. It's structured as a **library crate** with a thin binary wrapper, ensuring the core is reusable and testable in isolation. The backend is layered:

  - **`core`**: Contains the pure, reusable business logic (scanning, filtering, file handling). It is completely decoupled from the application and UI layers and uses a robust, typed error handling system.
  - **`app`**: Acts as the orchestrator, managing the application state, handling events from the UI, and executing commands. It uses an `EventProxy` trait to decouple itself from the concrete windowing system, enabling robust integration testing.
  - **`main.rs` / `lib.rs`**: The project is organized as a library (`lib.rs`) that exposes all core functionality, and a thin binary (`main.rs`) that acts as the application entry point.

- **Frontend (HTML/CSS/JS)**: The user interface is built with standard web technologies and rendered in a WebView.

  - **Wry & Tao**: We use the `wry` crate for the WebView and `tao` for windowing and the event loop, providing a lightweight cross-platform solution.
  - **Monaco Editor**: The same editor that powers VS Code is integrated for a high-quality code preview experience.
  - **IPC**: The frontend communicates with the Rust backend via an Inter-Process Communication (IPC) channel, sending JSON messages to trigger commands.

---

## Quality Assurance & Testing

- **Unit Tests**: The `core` logic is thoroughly tested with unit tests to ensure the correctness of individual components like the ignore-pattern matching and search engine.
- **Integration Tests**: Key application workflows are covered by integration tests that validate the interaction between the `app` and `core` layers without requiring a live UI.
- **Continuous Integration (CI)**: A GitHub Actions workflow automatically runs `cargo fmt` (formatter), `cargo clippy` (linter), and `cargo test` on every push and pull request to maintain code quality and prevent regressions.

---

## Building from Source

To build and run the application locally, you need [Node.js/npm](https://nodejs.org/) and the [Rust toolchain](https://www.rust-lang.org/tools/install).

1.  **Prepare the Frontend Dependencies:**
    Navigate to the `src/ui` directory and install the necessary JavaScript packages.

    ```bash
    cd src/ui
    npm install
    ```

2.  **Build the Frontend Bundle:**
    This command uses `esbuild` to bundle all JavaScript modules into a single file.

    ```bash
    npm run build
    ```

    For development, `npm run watch` will automatically rebuild on file changes.

3.  **Run the Rust Application:**
    Navigate back to the project root and use Cargo to build and run the app.

    ```bash
    cd ../..
    cargo run
    ```

4.  **Run Tests:** To run the full suite of unit and integration tests, use:

    ```bash
    cargo test
    ```
