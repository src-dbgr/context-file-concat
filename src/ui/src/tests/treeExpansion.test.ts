/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, vi } from "vitest";

vi.mock("$lib/services/backend", () => ({
  post: vi.fn(),
}));
import { post } from "$lib/services/backend";

import * as treeExpansion from "$lib/modules/treeExpansion";
import {
  collapseDirectory,
  expandDirectory,
  needsLazyLoad,
  setAllExpanded,
  toggleDirectory,
  type ExpandableNode,
} from "$lib/modules/treeExpansion";
import { appState } from "$lib/stores/app";

function dir(overrides: Partial<ExpandableNode> = {}): ExpandableNode {
  return {
    path: "/repo/src",
    is_expanded: false,
    children_loaded: true,
    ...overrides,
  };
}

function setScanning(is_scanning: boolean) {
  appState.update((s) => ({ ...s, is_scanning }));
}

describe("treeExpansion – expansion intents go to the backend", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    setScanning(false);
  });

  it("keeps no expansion state of its own", () => {
    // The backend's is_expanded is the single source of truth; there must be
    // nothing here that could override a snapshot.
    expect(Object.keys(treeExpansion).sort()).toEqual([
      "collapseDirectory",
      "expandDirectory",
      "needsLazyLoad",
      "setAllExpanded",
      "toggleDirectory",
    ]);
  });

  describe("needsLazyLoad", () => {
    it("is true only for an unloaded directory while no scan is running", () => {
      expect(needsLazyLoad(dir({ children_loaded: false }), false)).toBe(true);
      expect(needsLazyLoad(dir({ children_loaded: false }), true)).toBe(false);
      expect(needsLazyLoad(dir({ children_loaded: true }), false)).toBe(false);
    });

    it("treats a node without the flag as loaded", () => {
      expect(needsLazyLoad({ path: "/x", is_expanded: false }, false)).toBe(
        false,
      );
    });

    it("reads the scan state from the store by default", () => {
      const unloaded = dir({ children_loaded: false });
      expect(needsLazyLoad(unloaded)).toBe(true);
      setScanning(true);
      expect(needsLazyLoad(unloaded)).toBe(false);
    });
  });

  describe("toggleDirectory", () => {
    it("toggles a loaded directory, whatever its current state", () => {
      toggleDirectory(dir({ is_expanded: false }));
      toggleDirectory(dir({ is_expanded: true }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["toggleExpansion", "/repo/src"],
        ["toggleExpansion", "/repo/src"],
      ]);
    });

    it("loads an unloaded directory instead (the backend expands it)", () => {
      toggleDirectory(dir({ children_loaded: false }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["loadDirectoryLevel", "/repo/src"],
      ]);
    });

    it("loads an unloaded directory even if it is already expanded", () => {
      // Expanded during a scan that was cancelled: it shows nothing yet.
      toggleDirectory(dir({ children_loaded: false, is_expanded: true }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["loadDirectoryLevel", "/repo/src"],
      ]);
    });

    it("only toggles while a scan is running (the scan delivers the children)", () => {
      setScanning(true);
      toggleDirectory(dir({ children_loaded: false }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["toggleExpansion", "/repo/src"],
      ]);
    });
  });

  describe("expandDirectory / collapseDirectory", () => {
    it("expands a collapsed directory", () => {
      expandDirectory(dir({ is_expanded: false }));
      expect(post).toHaveBeenCalledExactlyOnceWith(
        "toggleExpansion",
        "/repo/src",
      );
    });

    it("does not collapse a directory that is already expanded", () => {
      expandDirectory(dir({ is_expanded: true }));
      expect(post).not.toHaveBeenCalled();
    });

    it("loads an unloaded directory when expanding it", () => {
      expandDirectory(dir({ children_loaded: false }));
      expect(post).toHaveBeenCalledExactlyOnceWith(
        "loadDirectoryLevel",
        "/repo/src",
      );
    });

    it("collapses an expanded directory, loaded or not", () => {
      collapseDirectory(dir({ is_expanded: true }));
      collapseDirectory(dir({ is_expanded: true, children_loaded: false }));
      expect(vi.mocked(post).mock.calls).toEqual([
        ["toggleExpansion", "/repo/src"],
        ["toggleExpansion", "/repo/src"],
      ]);
    });

    it("does not expand a directory that is already collapsed", () => {
      collapseDirectory(dir({ is_expanded: false }));
      expect(post).not.toHaveBeenCalled();
    });
  });

  it("setAllExpanded sends one bulk command", () => {
    setAllExpanded(true);
    setAllExpanded(false);
    expect(vi.mocked(post).mock.calls).toEqual([
      ["expandCollapseAll", true],
      ["expandCollapseAll", false],
    ]);
  });
});
