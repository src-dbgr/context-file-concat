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

describe("ipc/handlers – render() validation depth (DEV vs. production)", () => {
  /** Valid envelope whose nested tree node violates TreeNodeSchema. */
  function stateWithInvalidNestedNode(currentPath: string): AppState {
    const state = makeValidUiState();
    return {
      ...state,
      current_path: currentPath,
      tree: [
        {
          path: `${currentPath}/src`,
          name: "src",
          is_directory: true,
          is_expanded: true,
          is_binary: false,
          is_match: true,
          is_previewed: false,
          selection_state: "none",
          size: 0,
          children: [{ path: 42 }],
        },
      ],
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

  it("DEV: validates the tree deeply and rejects an invalid nested node", () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    expect(import.meta.env.DEV).toBe(true);

    window.render(stateWithInvalidNestedNode("/dev-deep"));

    expect(warn).toHaveBeenCalled();
    expect(get(appState).current_path).not.toBe("/dev-deep");
  });

  describe("production", () => {
    beforeEach(() => {
      vi.stubEnv("DEV", false);
    });

    it("accepts a valid payload", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      window.render(validState("/prod-valid"));

      expect(warn).not.toHaveBeenCalled();
      const state = get(appState);
      expect(state.current_path).toBe("/prod-valid");
      expect(state.tree).toHaveLength(makeValidUiState().tree.length);
    });

    it("does not recurse into the tree (envelope-only validation)", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      window.render(stateWithInvalidNestedNode("/prod-shallow"));

      expect(warn).not.toHaveBeenCalled();
      expect(get(appState).current_path).toBe("/prod-shallow");
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

    it("rejects a tree that is not an array of objects", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});

      window.render({
        ...validState("/prod-badtree"),
        tree: "nope",
      } as unknown as AppState);
      window.render({
        ...validState("/prod-badtree"),
        tree: [1, null],
      } as unknown as AppState);

      expect(warn).toHaveBeenCalledTimes(2);
      expect(get(appState).current_path).not.toBe("/prod-badtree");
    });
  });
});
