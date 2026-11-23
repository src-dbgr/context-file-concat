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
    it("is true only for a collapsed, unloaded directory", () => {
      expect(needsLazyLoad(dir({ children_loaded: false }))).toBe(true);
      expect(needsLazyLoad(dir({ children_loaded: true }))).toBe(false);
      expect(
        needsLazyLoad(dir({ children_loaded: false, is_expanded: true })),
      ).toBe(false);
    });

    it("does not depend on whether a scan is running", () => {
      const unloaded = dir({ children_loaded: false });
      expect(needsLazyLoad(unloaded)).toBe(true);
      setScanning(true);
      expect(needsLazyLoad(unloaded)).toBe(true);
    });
  });

  describe("toggleDirectory", () => {
    it("asks for the opposite of the rendered state", () => {
      toggleDirectory(dir({ is_expanded: false }));
      toggleDirectory(dir({ is_expanded: true }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("is idempotent while the snapshot is pending (double click)", () => {
      // Both events see the same, not yet re-rendered node.
      const rendered = dir({ is_expanded: false });
      toggleDirectory(rendered);
      toggleDirectory(rendered);

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["setExpansion", { path: "/repo/src", expanded: true }],
      ]);
    });

    it("loads an unloaded directory instead (the backend expands it)", () => {
      toggleDirectory(dir({ children_loaded: false }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["loadDirectoryLevel", "/repo/src"],
      ]);
    });

    it("collapses an expanded directory whose children are not loaded", () => {
      // Expanded during a scan that was cancelled: it must not be loaded
      // (and thereby kept open) by the gesture that is meant to close it.
      toggleDirectory(dir({ children_loaded: false, is_expanded: true }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("loads an unloaded directory while a scan is running too (the tree can be explored during the scan)", () => {
      setScanning(true);
      toggleDirectory(dir({ children_loaded: false }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["loadDirectoryLevel", "/repo/src"],
      ]);
    });

    it("does not load a directory the scan has completed meanwhile", () => {
      setScanning(true);
      toggleDirectory(dir({ children_loaded: true }));

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
      ]);
    });
  });

  describe("expandDirectory / collapseDirectory", () => {
    it("expands a collapsed directory", () => {
      expandDirectory(dir({ is_expanded: false }));
      expect(post).toHaveBeenCalledExactlyOnceWith("setExpansion", {
        path: "/repo/src",
        expanded: true,
      });
    });

    it("never collapses on a repeated expand (ArrowRight key repeat)", () => {
      // The repeated key event still sees the collapsed node.
      const rendered = dir({ is_expanded: false });
      expandDirectory(rendered);
      expandDirectory(rendered);

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: true }],
        ["setExpansion", { path: "/repo/src", expanded: true }],
      ]);
    });

    it("never expands on a repeated collapse (ArrowLeft key repeat)", () => {
      const rendered = dir({ is_expanded: true });
      collapseDirectory(rendered);
      collapseDirectory(rendered);

      expect(vi.mocked(post).mock.calls).toEqual([
        ["setExpansion", { path: "/repo/src", expanded: false }],
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("does not collapse a directory that is already expanded", () => {
      expandDirectory(dir({ is_expanded: true }));
      expect(post).not.toHaveBeenCalled();
    });

    it("does not load a directory that is expanded but not loaded", () => {
      expandDirectory(dir({ is_expanded: true, children_loaded: false }));
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
        ["setExpansion", { path: "/repo/src", expanded: false }],
        ["setExpansion", { path: "/repo/src", expanded: false }],
      ]);
    });

    it("does not expand a directory that is already collapsed", () => {
      collapseDirectory(dir({ is_expanded: false }));
      expect(post).not.toHaveBeenCalled();
    });
  });

  it("never sends the non-idempotent toggle command", () => {
    for (const is_expanded of [true, false]) {
      for (const children_loaded of [true, false]) {
        for (const scanning of [true, false]) {
          setScanning(scanning);
          const node = dir({ is_expanded, children_loaded });
          toggleDirectory(node);
          expandDirectory(node);
          collapseDirectory(node);
        }
      }
    }
    const sent = vi.mocked(post).mock.calls.map(([command]) => command);
    expect(new Set(sent)).toEqual(
      new Set(["setExpansion", "loadDirectoryLevel"]),
    );
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
