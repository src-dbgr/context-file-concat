/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { get } from "svelte/store";

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
import { makeValidUiState } from "../contracts/factories";

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
      {} as unknown as string,
    );
    expect(warn).toHaveBeenCalled();
  });

  it("showGeneratedContent: invalid args are ignored with a warning", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    window.showGeneratedContent(
      {} as unknown as string,
      "x" as unknown as number,
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

describe("ipc/handlers – render() validates every row, in every build", () => {
  /** A valid snapshot except for one row inside the window. */
  function stateWithInvalidRow(currentPath: string): AppState {
    const state = makeValidUiState();
    return {
      ...state,
      current_path: currentPath,
      rows: [state.rows[0], { path: 42 }, state.rows[2]],
    } as unknown as AppState;
  }

  function validState(currentPath: string): AppState {
    return {
      ...makeValidUiState(),
      current_path: currentPath,
    } as unknown as AppState;
  }

  beforeEach(() => {
    vi.restoreAllMocks();
    window.ipc = { postMessage: (_: string) => void 0 };
    installWindowIPCHandlers();
  });

  afterEach(() => {
    vi.unstubAllEnvs();
  });

  for (const dev of [true, false]) {
    describe(dev ? "DEV" : "production", () => {
      beforeEach(() => {
        vi.stubEnv("DEV", dev);
      });

      it("accepts a valid payload", () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

        window.render(validState("/valid"));

        expect(warn).not.toHaveBeenCalled();
        const state = get(appState);
        expect(state.current_path).toBe("/valid");
        expect(state.rows).toHaveLength(makeValidUiState().rows.length);
        expect(state.rows_total).toBe(makeValidUiState().rows_total);
      });

      it("rejects a snapshot with an invalid row", () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

        window.render(stateWithInvalidRow("/bad-row"));

        expect(warn).toHaveBeenCalled();
        expect(get(appState).current_path).not.toBe("/bad-row");
      });

      it("rejects a window that exceeds the list", () => {
        const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

        window.render({
          ...validState("/bad-window"),
          rows_total: 2,
        });

        expect(warn).toHaveBeenCalled();
        expect(get(appState).current_path).not.toBe("/bad-window");
      });
    });
  }

  describe("production", () => {
    beforeEach(() => {
      vi.stubEnv("DEV", false);
    });

    it("rejects an envelope with a missing config", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      const { config: _config, ...withoutConfig } =
        validState("/prod-noconfig");

      window.render(withoutConfig as unknown as AppState);

      expect(warn).toHaveBeenCalled();
      expect(get(appState).current_path).not.toBe("/prod-noconfig");
    });

    it("rejects an envelope with wrongly typed fields", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      window.render({
        ...validState("/prod-badtype"),
        is_scanning: "no",
      } as unknown as AppState);
      window.render({
        ...validState("/prod-badconfig"),
        config: { ...makeValidUiState().config, output_filename: 7 },
      } as unknown as AppState);

      expect(warn).toHaveBeenCalledTimes(2);
      expect(get(appState).current_path).not.toMatch(/^\/prod-bad/);
    });

    it("rejects rows that are not an array of objects", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      window.render({
        ...validState("/prod-badtree"),
        rows: "nope",
      } as unknown as AppState);
      window.render({
        ...validState("/prod-badtree"),
        rows: [1, null],
        rows_total: 2,
      } as unknown as AppState);

      expect(warn).toHaveBeenCalledTimes(2);
      expect(get(appState).current_path).not.toBe("/prod-badtree");
    });
  });
});
