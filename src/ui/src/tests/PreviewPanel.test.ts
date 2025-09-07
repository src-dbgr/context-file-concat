/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import PreviewPanel from "$lib/components/PreviewPanel.svelte";
import { appState, editorInstance, previewedPath } from "$lib/stores/app";
import { previewMode, generatedTokenCount } from "$lib/stores/preview";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

// Mock clearPreview so clicking the button doesn't pull Monaco
vi.mock("$lib/modules/editor", () => ({
  clearPreview: vi.fn(),
}));
import { clearPreview } from "$lib/modules/editor";

// Minimal fake Monaco editor
function makeFakeEditor(
  content = "Hello world"
): monaco.editor.IStandaloneCodeEditor {
  const disposable: monaco.IDisposable = { dispose: () => {} };
  const model = {
    onDidChangeContent: (_cb: () => void) => disposable,
  } as unknown as monaco.editor.ITextModel;

  const fake = {
    getModel: () => model,
    getValue: () => content,
  } as unknown;

  return fake as monaco.editor.IStandaloneCodeEditor;
}

function seedApp(overrides: Partial<Parameters<typeof appState.set>[0]> = {}) {
  appState.set({
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
    ...overrides,
  });
}

describe("PreviewPanel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    editorInstance.set(null);
    previewedPath.set(null);
    previewMode.set("idle");
    generatedTokenCount.set(null);
    seedApp();
  });

  it("shows default title and hides action buttons when idle", () => {
    render(PreviewPanel);

    expect(
      screen.getByText(/^Preview$/i, { selector: ".preview-filename" })
    ).toBeInTheDocument();

    const copyBtn = document.getElementById("copy-btn") as HTMLButtonElement;
    const clearBtn = document.getElementById(
      "clear-preview-btn"
    ) as HTMLButtonElement;

    expect(copyBtn).toBeInTheDocument();
    expect(clearBtn).toBeInTheDocument();
    expect(copyBtn).not.toBeVisible();
    expect(clearBtn).not.toBeVisible();
    expect(copyBtn).toBeDisabled();
    expect(clearBtn).toBeDisabled();
  });

  it("renders Generated mode header and editable stats (incl. tokens)", () => {
    editorInstance.set(makeFakeEditor("a b c"));
    previewMode.set("generated");
    generatedTokenCount.set(1500);

    render(PreviewPanel);

    expect(screen.getByText(/Preview generated/i)).toBeInTheDocument();
    expect(screen.getByText(/1 lines/i)).toBeInTheDocument();
    expect(screen.getByText(/3 words/i)).toBeInTheDocument();
    expect(screen.getByText(/1\.5K tokens/i)).toBeInTheDocument();
    expect(screen.getByText(/editable/i)).toBeInTheDocument();

    // In Generated-Mode sind die Buttons sichtbar & aktiv
    const copyBtn = screen.getByRole("button", { name: /copy/i });
    const clearBtn = screen.getByRole("button", { name: /clear/i });

    expect(copyBtn).toBeVisible();
    expect(clearBtn).toBeVisible();
    expect(copyBtn).not.toBeDisabled();
    expect(clearBtn).not.toBeDisabled();
  });

  it("renders File mode path + filename from previewedPath", () => {
    editorInstance.set(makeFakeEditor("line1\nline2"));
    previewMode.set("file");
    previewedPath.set("/repo/src/main.rs");

    render(PreviewPanel);

    expect(screen.getByText("src/")).toBeInTheDocument();
    expect(screen.getByText("main.rs")).toBeInTheDocument();
    expect(screen.getByText(/read only/i)).toBeInTheDocument();
  });

  it("invokes clearPreview() when Clear button is clicked", async () => {
    editorInstance.set(makeFakeEditor("content"));
    previewMode.set("file");
    previewedPath.set("/repo/file.txt");

    render(PreviewPanel);

    const clearBtn = screen.getByRole("button", { name: /clear/i });
    await fireEvent.click(clearBtn);
    expect(clearPreview).toHaveBeenCalledTimes(1);
  });
});
