/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import PreviewPanel from "$lib/components/PreviewPanel.svelte";
import { appState, editorInstance, previewedPath } from "$lib/stores/app";
import {
  previewMode,
  generatedTokenCount,
  flashCopyFeedback,
} from "$lib/stores/preview";
import { tick } from "svelte";
import type * as monaco from "monaco-editor/esm/vs/editor/editor.api";

// Mock clearPreview so clicking the button doesn't pull Monaco
vi.mock("$lib/modules/editor", () => ({
  clearPreview: vi.fn(),
}));
import { clearPreview } from "$lib/modules/editor";

type ContentChange = { isFlush: boolean };

// Minimal fake Monaco editor
function makeFakeEditor(
  content = "Hello world",
): monaco.editor.IStandaloneCodeEditor {
  return makeEditableFakeEditor(content).editor;
}

/** A fake editor whose text can be changed the way Monaco reports it. */
function makeEditableFakeEditor(initial: string) {
  let content = initial;
  let listener: ((change: ContentChange) => void) | null = null;
  const getValue = vi.fn(() => content);
  const model = {
    onDidChangeContent: (cb: (change: ContentChange) => void) => {
      listener = cb;
      return { dispose: () => (listener = null) };
    },
  } as unknown as monaco.editor.ITextModel;
  const fake = { getModel: () => model, getValue } as unknown;

  return {
    editor: fake as monaco.editor.IStandaloneCodeEditor,
    getValue,
    /** An edit of the user (typing, paste, undo). */
    edit(next: string) {
      content = next;
      listener?.({ isFlush: false });
    },
    /** The whole text is replaced (`setValue`). */
    replace(next: string) {
      content = next;
      listener?.({ isFlush: true });
    },
  };
}

function seedApp(overrides: Partial<Parameters<typeof appState.set>[0]> = {}) {
  appState.set({
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    rows: [],
    rows_start: 0,
    rows_total: 0,
    rows_revision: 0,
    located: null,
    last_processed_seq: 0,
    current_path: "/repo",
    current_config_filename: null,
    status_message: "Status: Ready.",
    total_files_found: 0,
    visible_files_count: 0,
    scan_progress: {
      files_scanned: 0,
      large_files_skipped: 0,
      current_scanning_path: "",
    },
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
      screen.getByText(/^Preview$/i, { selector: ".preview-filename" }),
    ).toBeInTheDocument();

    const copyBtn = document.getElementById("copy-btn") as HTMLButtonElement;
    const clearBtn = document.getElementById(
      "clear-preview-btn",
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

  it("counts new content at once, and edits after a pause instead of per keystroke", async () => {
    vi.useFakeTimers();
    try {
      const fake = makeEditableFakeEditor("a b c");
      editorInstance.set(fake.editor);
      previewMode.set("generated");
      generatedTokenCount.set(3);
      render(PreviewPanel);
      const stats = () => document.querySelector(".preview-stats")?.textContent;
      expect(stats()).toContain("3 words");

      // Typing: the text is not read again for every key ...
      const readsBefore = fake.getValue.mock.calls.length;
      fake.edit("a b c d");
      fake.edit("a b c d e");
      await tick();
      expect(stats()).toContain("3 words");
      expect(fake.getValue.mock.calls.length).toBe(readsBefore);
      await vi.advanceTimersByTimeAsync(150);
      fake.edit("a b c d e f");
      await vi.advanceTimersByTimeAsync(150);
      expect(stats()).toContain("3 words");

      // ... but once, when the typing pauses.
      await vi.advanceTimersByTimeAsync(100);
      expect(stats()).toContain("6 words");

      // Content that replaces the text is counted right away, and an update
      // that was still pending for the old text does not follow it.
      fake.edit("a b c d e f g");
      fake.replace("one two");
      await tick();
      expect(stats()).toContain("2 words");
      const readsAfter = fake.getValue.mock.calls.length;
      await vi.advanceTimersByTimeAsync(1000);
      expect(fake.getValue.mock.calls.length).toBe(readsAfter);
    } finally {
      vi.useRealTimers();
    }
  });

  it("shows the outcome of a copy on the Copy button, then the normal label again", async () => {
    vi.useFakeTimers();
    try {
      editorInstance.set(makeFakeEditor("a b c"));
      previewMode.set("generated");
      render(PreviewPanel);
      const copyBtn = document.getElementById("copy-btn") as HTMLButtonElement;
      expect(copyBtn.textContent?.trim()).toBe("Copy");
      expect(copyBtn.querySelector("svg")).not.toBeNull();

      flashCopyFeedback("copied");
      await tick();
      expect(copyBtn.textContent?.trim()).toBe("Copied!");
      expect(copyBtn).toHaveClass("button-cta");
      expect(copyBtn).not.toHaveClass("button-cancel-action");

      flashCopyFeedback("failed");
      await tick();
      expect(copyBtn.textContent?.trim()).toBe("Failed");
      expect(copyBtn).toHaveClass("button-cancel-action");
      expect(copyBtn).not.toHaveClass("button-cta");

      vi.advanceTimersByTime(1100);
      await tick();
      expect(copyBtn.textContent?.trim()).toBe("Copy");
      expect(copyBtn.querySelector("svg")).not.toBeNull();
      expect(copyBtn).not.toHaveClass("button-cta");
      expect(copyBtn).not.toHaveClass("button-cancel-action");
    } finally {
      vi.useRealTimers();
    }
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
