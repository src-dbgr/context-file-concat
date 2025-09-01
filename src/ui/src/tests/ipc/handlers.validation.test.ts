/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";

// Mock the editor module BEFORE importing handlers to avoid monaco worker imports.
vi.mock("$lib/modules/editor", () => ({
  layoutEditorSoon: vi.fn(),
  initEditor: vi.fn(),
  showPreviewContent: vi.fn(),
  showGeneratedContent: vi.fn(),
  clearPreview: vi.fn(),
}));

import { installWindowIPCHandlers } from "$lib/ipc/handlers";
import { appState } from "$lib/stores/app";
import type { AppState } from "$lib/types";

declare global {
  interface Window {
    ipc: { postMessage(message: string): void };
  }
}

describe("ipc/handlers – validation paths with real Zod schemas", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    document.body.innerHTML = "";
    window.ipc = { postMessage: (_: string) => void 0 };
    installWindowIPCHandlers();
  });

  it("render: rejects invalid payloads and logs a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    // Invalid shape; cast via unknown to satisfy the signature
    window.render({} as unknown as AppState);
    expect(warn).toHaveBeenCalled();
  });

  it("showPreviewContent: invalid tuple is ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.showPreviewContent(
      123 as unknown as string,
      null as unknown as string,
      undefined,
      {} as unknown as string
    );
    expect(warn).toHaveBeenCalled();
  });

  it("showGeneratedContent: invalid args are ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.showGeneratedContent(
      {} as unknown as string,
      "x" as unknown as number
    );
    expect(warn).toHaveBeenCalled();
  });

  it("showError/showStatus: non-string are ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.showError({} as unknown as string);
    window.showStatus([] as unknown as string);
    expect(warn).toHaveBeenCalled();
  });

  it("fileSaveStatus: invalid tuple is ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.fileSaveStatus("yes" as unknown as boolean, 42 as unknown as string);
    expect(warn).toHaveBeenCalled();
  });

  it("setDragState: invalid boolean is ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.setDragState("true" as unknown as boolean);
    expect(warn).toHaveBeenCalled();
  });

  it("updateScanProgress: returns early when not scanning (no warning)", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.updateScanProgress({
      files_scanned: 1,
      current_scanning_path: "/x",
      large_files_skipped: 0,
    });
    expect(warn).not.toHaveBeenCalled();
  });

  it("updateScanProgress: invalid payload while scanning logs a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    appState.update((s) => {
      s.is_scanning = true;
      return s;
    });

    window.updateScanProgress({
      files_scanned: "NaN" as unknown as number,
      current_scanning_path: {} as unknown as string,
      large_files_skipped: "0" as unknown as number,
    });
    expect(warn).toHaveBeenCalled();
  });
});
