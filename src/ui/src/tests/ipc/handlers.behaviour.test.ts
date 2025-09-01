/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { get } from "svelte/store";
import { appState } from "$lib/stores/app";

/**
 * Hoisted mocks (evaluated before Imports)
 */

// toast: we check calls for success/info/error
const { toast } = vi.hoisted(() => ({
  toast: { success: vi.fn(), info: vi.fn(), error: vi.fn() },
}));
vi.mock("$lib/stores/toast", () => ({ toast }));

// treeExpansion: applyExpansionMemory shall return identically; clearExpansionMemory only Spy
const { clearExpansionMemory, applyExpansionMemory } = vi.hoisted(() => ({
  clearExpansionMemory: vi.fn(),
  applyExpansionMemory: vi.fn((x: unknown[]) => x),
}));
vi.mock("$lib/modules/treeExpansion", () => ({
  clearExpansionMemory,
  applyExpansionMemory,
}));

// i18n: t(key) => key
vi.mock("$lib/i18n", () => {
  const t = {
    subscribe(run: (val: (key: string) => string) => void) {
      run((k: string) => k);
      return () => {};
    },
  };
  return { t };
});

// editor-API
vi.mock("$lib/modules/editor", () => ({
  layoutEditorSoon: vi.fn(),
  initEditor: vi.fn(),
  showPreviewContent: vi.fn(),
  showGeneratedContent: vi.fn(),
  clearPreview: vi.fn(),
}));

/**
 * Important: mock UiStateSchema etc. so that safeParse is always accepted.
 * Thus we are testing the behaviour of the handler and not the zod validation.
 */
vi.mock("$lib/ipc/schema", () => {
  const passthrough = <T>(data: T) => ({ success: true as const, data });
  return {
    UiStateSchema: { safeParse: passthrough },
    ScanProgressSchema: { safeParse: passthrough },
    ShowPreviewArgsSchema: { safeParse: passthrough },
    ShowGeneratedArgsSchema: { safeParse: passthrough },
    StatusMessageSchema: { safeParse: passthrough },
    FileSaveStatusArgsSchema: { safeParse: passthrough },
    DragStateSchema: { safeParse: passthrough },
  };
});

// Only now import the functions to be tested
import { installWindowIPCHandlers } from "$lib/ipc/handlers";
import {
  showPreviewContent as showPreviewContentImpl,
  showGeneratedContent as showGeneratedContentImpl,
  clearPreview as clearPreviewImpl,
} from "$lib/modules/editor";

declare global {
  interface Window {
    ipc: { postMessage(message: string): void };
  }
}

function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  opts?: { id?: string; className?: string }
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (opts?.id) node.id = opts.id;
  if (opts?.className) node.className = opts.className;
  document.body.appendChild(node);
  return node;
}

describe("ipc/handlers – behavior of installed window handlers", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    document.body.innerHTML = "";
    window.ipc = { postMessage: (_: string) => void 0 };
    installWindowIPCHandlers();
  });

  it("showPreviewContent validates and forwards to editor", () => {
    window.showPreviewContent("code", "javascript", "", "/path/file.ts");
    expect(showPreviewContentImpl).toHaveBeenCalledWith(
      "code",
      "javascript",
      "",
      "/path/file.ts"
    );
  });

  it("showGeneratedContent validates and forwards to editor", () => {
    window.showGeneratedContent("output", 42);
    expect(showGeneratedContentImpl).toHaveBeenCalledWith("output", 42);
  });

  it("showError updates status and emits toast.error", () => {
    window.showError("Boom");
    const state = get(appState);
    expect(state.status_message).toMatch(/^Error: /);
    expect(toast.error).toHaveBeenCalledWith("toast.renderFailed");
  });

  it("showStatus updates status", () => {
    window.showStatus("Working");
    const state = get(appState);
    expect(state.status_message).toBe("Status: Working");
  });

  it("fileSaveStatus handles cancelled, success, and failure", () => {
    window.fileSaveStatus(true, "cancelled");
    expect(get(appState).status_message).toBe("Status: Save cancelled.");
    expect(toast.info).toHaveBeenCalledWith("toast.saveCancelled");

    window.fileSaveStatus(true, "/tmp/out.txt");
    expect(get(appState).status_message).toBe("Status: Saved to /tmp/out.txt");
    expect(toast.success).toHaveBeenCalledWith("toast.fileSaved");

    window.fileSaveStatus(false, "/tmp/out.txt");
    expect(get(appState).status_message).toBe("Error: Failed to save file.");
    expect(toast.error).toHaveBeenCalledWith("toast.saveFailed");
  });

  it("setDragState toggles css class on the file tree container", () => {
    const container = el("div", { id: "file-tree-container" });
    expect(container.classList.contains("drag-over")).toBe(false);

    window.setDragState(true);
    expect(container.classList.contains("drag-over")).toBe(true);

    window.setDragState(false);
    expect(container.classList.contains("drag-over")).toBe(false);
  });

  it("updateScanProgress writes progress text and width when scanning", () => {
    el("div", { className: "scan-text" });
    el("div", { id: "scan-files-count" });
    el("div", { id: "scan-current-path" });
    el("div", { id: "scan-skipped-count" });
    el("div", { id: "scan-progress-fill" });

    appState.update((s) => {
      s.is_scanning = true;
      return s;
    });

    window.updateScanProgress({
      files_scanned: 10,
      current_scanning_path: "/some/path",
      large_files_skipped: 1,
    });

    const scanText = document.querySelector(".scan-text") as HTMLElement;
    const files = document.getElementById("scan-files-count") as HTMLElement;
    const current = document.getElementById("scan-current-path") as HTMLElement;
    const skipped = document.getElementById(
      "scan-skipped-count"
    ) as HTMLElement;
    const fill = document.getElementById("scan-progress-fill") as HTMLElement;

    expect(scanText.textContent).toBe("filetree.scanning");
    expect(files.textContent).toContain("files processed");
    expect(current.textContent).toBe("/some/path");
    expect(skipped.textContent).toContain("large files skipped");
    expect(fill.style.width).toMatch(/%$/);
  });

  it("render applies expansion memory and clears it on path change", () => {
    const base = get(appState);

    const p1 = {
      ...base,
      current_path: "/a",
      tree: Array.isArray(base.tree) ? base.tree : [],
      status_message: "Ready.",
    } as Parameters<typeof window.render>[0];

    window.render(p1);
    expect(applyExpansionMemory).toHaveBeenCalled();

    const p2 = {
      ...base,
      current_path: "/b",
      tree: Array.isArray(base.tree) ? base.tree : [],
      status_message: "Ready.",
    } as Parameters<typeof window.render>[0];

    window.render(p2);
    expect(clearExpansionMemory).toHaveBeenCalled();
  });

  it("render clears preview when current_path becomes null", () => {
    appState.update((s) => {
      s.current_path = "/was/set";
      return s;
    });

    const base = get(appState);
    const p = {
      ...base,
      current_path: null,
      tree: Array.isArray(base.tree) ? base.tree : [],
      status_message: "Ready.",
    } as Parameters<typeof window.render>[0];

    window.render(p);
    expect(clearPreviewImpl).toHaveBeenCalled();
  });
});
