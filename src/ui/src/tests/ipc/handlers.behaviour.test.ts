/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { get } from "svelte/store";
import { appState } from "$lib/stores/app";
import { scanProgress } from "$lib/stores/scanProgress";

/**
 * Hoisted mocks (evaluated before Imports)
 */

// toast: we check calls for success/info/error
const { toast } = vi.hoisted(() => ({
  toast: {
    success: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  },
}));
vi.mock("$lib/stores/toast", () => ({ toast }));

// i18n: t(key) => key, with the parameters appended where there are any
vi.mock("$lib/i18n", () => {
  type Params = Record<string, string | number>;
  const t = {
    subscribe(run: (val: (key: string, params?: Params) => string) => void) {
      run((k: string, params?: Params) =>
        params ? `${k} ${JSON.stringify(params)}` : k,
      );
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
    SaveResultSchema: { safeParse: passthrough },
    OutputSizeNoticeSchema: { safeParse: passthrough },
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
  opts?: { id?: string; className?: string },
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
      "/path/file.ts",
    );
  });

  it("showGeneratedContent validates and forwards to editor", () => {
    window.showGeneratedContent("output", 42, 7);
    expect(showGeneratedContentImpl).toHaveBeenCalledWith("output", 42, 7);
  });

  it("showError updates status and emits toast.error", () => {
    window.showError("Boom");
    const state = get(appState);
    expect(state.status_message).toMatch(/^Error: /);
    // The toast carries the text of the host, under a localized title.
    expect(toast.error).toHaveBeenCalledWith("Boom", {
      title: "toast.errorTitle",
      duration: 10_000,
    });
  });

  it("showStatus updates status", () => {
    window.showStatus("Working");
    const state = get(appState);
    expect(state.status_message).toBe("Status: Working");
  });

  it("saveResult: a saved file is reported with its path", () => {
    window.saveResult({ status: "saved", path: "/tmp/out.txt" });
    expect(get(appState).status_message).toBe("Status: Saved to /tmp/out.txt");
    expect(toast.success).toHaveBeenCalledWith("toast.fileSaved");
    expect(toast.info).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("saveResult: a cancelled save is neither a success nor an error", () => {
    window.saveResult({ status: "cancelled" });
    expect(get(appState).status_message).toBe("Status: Save cancelled.");
    expect(toast.info).toHaveBeenCalledWith("toast.saveCancelled");
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("saveResult: a failed save shows the reason of the host", () => {
    window.saveResult({
      status: "failed",
      error: "Permission denied (os error 13)",
    });
    expect(get(appState).status_message).toBe("Error: Failed to save file.");
    // The reason is the text, under a localized title; it is never a path.
    expect(toast.error).toHaveBeenCalledWith(
      "Permission denied (os error 13)",
      { title: "toast.saveFailed", duration: 10_000 },
    );
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("saveResult: a file that is named 'cancelled' is a saved file", () => {
    window.saveResult({ status: "saved", path: "cancelled" });
    expect(get(appState).status_message).toBe("Status: Saved to cancelled");
    expect(toast.success).toHaveBeenCalledWith("toast.fileSaved");
    expect(toast.info).not.toHaveBeenCalled();
  });

  it("saveResult: a failure without a reason still says what failed", () => {
    window.saveResult({ status: "failed", error: "  " });
    expect(toast.error).toHaveBeenCalledWith("toast.saveFailed");
  });

  it("saveResult: a long reason is cut", () => {
    window.saveResult({ status: "failed", error: "x".repeat(1000) });
    const [text] = toast.error.mock.calls[0] as [string];
    expect(text.length).toBeLessThanOrEqual(300);
  });

  it("outputSizeNotice: a large output is announced with a warning", () => {
    window.outputSizeNotice({
      status: "large",
      estimated_bytes: 31 * 1024 * 1024,
      limit_bytes: 20 * 1024 * 1024,
    });
    expect(toast.warning).toHaveBeenCalledWith(
      'toast.outputLarge {"size":"31 MB","limit":"20 MB"}',
      { title: "toast.outputLargeTitle" },
    );
    expect(toast.error).not.toHaveBeenCalled();
  });

  it("outputSizeNotice: a refused output is an error that stays readable", () => {
    window.outputSizeNotice({
      status: "too_large",
      estimated_bytes: 314572800,
      limit_bytes: 209715200,
    });
    expect(toast.error).toHaveBeenCalledWith(
      'toast.outputTooLarge {"size":"300 MB","limit":"200 MB"}',
      { title: "toast.outputTooLargeTitle", duration: 10_000 },
    );
    expect(toast.warning).not.toHaveBeenCalled();
  });

  it("setDragState toggles css class on the file tree container", () => {
    const container = el("div", { id: "file-tree-container" });
    expect(container.classList.contains("drag-over")).toBe(false);

    window.setDragState(true);
    expect(container.classList.contains("drag-over")).toBe(true);

    window.setDragState(false);
    expect(container.classList.contains("drag-over")).toBe(false);
  });

  it("updateScanProgress reports to the scanProgress store when scanning", () => {
    appState.update((s) => {
      s.is_scanning = true;
      return s;
    });

    window.updateScanProgress({
      files_scanned: 10,
      current_scanning_path: "/some/path",
      large_files_skipped: 1,
    });

    expect(get(scanProgress)).toEqual({
      files_scanned: 10,
      current_scanning_path: "/some/path",
      large_files_skipped: 1,
    });
  });

  it("render takes the backend's expansion state as is", () => {
    const base = get(appState);
    const dir = (path: string, is_expanded: boolean, children: unknown[]) => ({
      path,
      name: path,
      is_directory: true,
      is_expanded,
      is_binary: false,
      is_match: false,
      is_previewed: false,
      selection_state: "none",
      size: 0,
      children,
      has_children: true,
      children_loaded: true,
    });
    const snapshot = (tree: unknown[]) =>
      ({
        ...base,
        current_path: "/a",
        tree,
        status_message: "Ready.",
      }) as Parameters<typeof window.render>[0];

    // Expanded by the backend: the children arrive with the snapshot.
    const expanded = [dir("/a/src", true, [dir("/a/src/core", false, [])])];
    window.render(snapshot(expanded));
    expect(get(appState).tree).toBe(expanded);
    expect(get(appState).tree[0].is_expanded).toBe(true);

    // Collapsed by a later snapshot: nothing in the UI keeps it open.
    window.render(snapshot([dir("/a/src", false, [])]));
    const [src] = get(appState).tree;
    expect(src.is_expanded).toBe(false);
    expect(src.children).toEqual([]);

    // ...and nothing keeps it closed either (e.g. search auto-expansion).
    window.render(snapshot(expanded));
    expect(get(appState).tree[0].is_expanded).toBe(true);
    expect(get(appState).tree[0].children).toHaveLength(1);
  });

  it("render clears preview when the directory is closed", () => {
    appState.update((s) => {
      s.current_path = "/was/set";
      return s;
    });

    const base = get(appState);
    const p = {
      ...base,
      current_path: "",
      tree: Array.isArray(base.tree) ? base.tree : [],
      status_message: "Ready.",
    } as Parameters<typeof window.render>[0];

    window.render(p);
    expect(clearPreviewImpl).toHaveBeenCalled();
  });
});
