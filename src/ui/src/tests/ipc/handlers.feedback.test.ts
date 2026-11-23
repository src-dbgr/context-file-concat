/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { get } from "svelte/store";

// The editor module pulls Monaco; it plays no part here.
vi.mock("$lib/modules/editor", () => ({
  showPreviewContent: vi.fn(),
  showGeneratedContent: vi.fn(),
  clearPreview: vi.fn(),
}));

import { installWindowIPCHandlers } from "$lib/ipc/handlers";
import { appState } from "$lib/stores/app";
import { scanProgress } from "$lib/stores/scanProgress";
import { toasts, toast } from "$lib/stores/toast";
import { setLocale } from "$lib/i18n";
import type { AppState } from "$lib/types";

/**
 * What the user gets to see of host feedback, with the real stores:
 * - an error of the host is shown as it is, in a toast that stays a while;
 * - scan progress lands in its store and belongs to one scan.
 */

declare global {
  interface Window {
    ipc: { postMessage(message: string): void };
  }
}

function snapshot(overrides: Partial<AppState> = {}): AppState {
  return structuredClone({ ...get(appState), ...overrides });
}

describe("ipc/handlers – feedback the user sees", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    window.ipc = { postMessage: () => void 0 };
    installWindowIPCHandlers();
    toast.clear();
    scanProgress.reset();
    setLocale("en");
    window.render(snapshot({ is_scanning: false, status_message: "Ready." }));
  });
  afterEach(() => {
    toast.clear();
    setLocale("en");
    vi.useRealTimers();
  });

  describe("showError", () => {
    it("shows the message of the host in an error toast", () => {
      window.showError("Could not read /repo/secret.txt: permission denied");

      const shown = get(toasts);
      expect(shown).toHaveLength(1);
      expect(shown[0].variant).toBe("error");
      expect(shown[0].title).toBe("Error");
      expect(shown[0].message).toBe(
        "Could not read /repo/secret.txt: permission denied",
      );
    });

    it("localizes the title, not the text of the host", () => {
      setLocale("de");
      window.showError("Disk full");

      const [shown] = get(toasts);
      expect(shown.title).toBe("Fehler");
      expect(shown.message).toBe("Disk full");
    });

    it("stays readable when the next snapshot replaces the status line", () => {
      window.showError("Disk full");
      window.render(snapshot({ status_message: "Scan complete." }));

      expect(get(appState).status_message).toBe("Status: Scan complete.");
      expect(get(toasts).map((t) => t.message)).toEqual(["Disk full"]);
    });

    it("stays for ten seconds, then goes away by itself", () => {
      window.showError("Disk full");

      vi.advanceTimersByTime(9_900);
      expect(get(toasts)).toHaveLength(1);

      vi.advanceTimersByTime(200);
      expect(get(toasts)).toHaveLength(0);
    });

    it("caps a very long message", () => {
      window.showError(`Failed: ${"x".repeat(5_000)}`);

      const [shown] = get(toasts);
      expect(shown.message.length).toBe(300);
      expect(shown.message.startsWith("Failed: xxx")).toBe(true);
      expect(shown.message.endsWith("…")).toBe(true);
    });

    it("falls back to a general text for an empty message", () => {
      window.showError("   ");

      expect(get(toasts)[0].message).toBe("An error occurred");
    });

    it("also puts the message on the status line", () => {
      window.showError("Disk full");

      expect(get(appState).status_message).toBe("Error: Disk full");
    });
  });

  describe("updateScanProgress", () => {
    const progress = {
      files_scanned: 120,
      large_files_skipped: 2,
      current_scanning_path: "/repo/src/lib.rs",
    };

    it("reports the progress of a running scan to the store", () => {
      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress(progress);

      expect(get(scanProgress)).toEqual(progress);
    });

    it("keeps the progress across snapshots of the same scan", () => {
      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress(progress);
      window.render(snapshot({ is_scanning: true }));

      expect(get(scanProgress)).toEqual(progress);
    });

    it("ignores progress when no scan is running", () => {
      window.updateScanProgress(progress);

      expect(get(scanProgress).files_scanned).toBe(0);
    });

    it("ignores an invalid report", () => {
      const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress({
        ...progress,
        files_scanned: -1,
      });

      expect(get(scanProgress).files_scanned).toBe(0);
      expect(warn).toHaveBeenCalled();
      warn.mockRestore();
    });

    it("forgets the progress when the scan ends and when a new one starts", () => {
      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress(progress);

      window.render(snapshot({ is_scanning: false }));
      expect(get(scanProgress).files_scanned).toBe(0);

      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress(progress);
      // A snapshot that is the first of a new scan (the end of the previous
      // one was not seen as a snapshot of its own).
      appState.update((s) => ({ ...s, is_scanning: false }));
      window.render(snapshot({ is_scanning: true }));
      expect(get(scanProgress)).toEqual({
        files_scanned: 0,
        large_files_skipped: 0,
        current_scanning_path: "",
      });
    });

    it("writes nothing into the DOM itself", () => {
      const count = document.createElement("span");
      count.id = "scan-files-count";
      document.body.appendChild(count);

      window.render(snapshot({ is_scanning: true }));
      window.updateScanProgress(progress);

      expect(count.textContent).toBe("");
      count.remove();
    });
  });
});
