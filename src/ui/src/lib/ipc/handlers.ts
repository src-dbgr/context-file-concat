/**
 * Window IPC handlers extracted from main.ts
 * - Zod-validated
 * - TS strict, no any
 * - Idempotent install
 */

import { appState, getState } from "$lib/stores/app";
import { toast } from "$lib/stores/toast";
import { t as tStore } from "$lib/i18n";
import { get } from "svelte/store";

import {
  UiStateSchema,
  ScanProgressSchema,
  ShowPreviewArgsSchema,
  ShowGeneratedArgsSchema,
  StatusMessageSchema,
  FileSaveStatusArgsSchema,
  DragStateSchema,
} from "$lib/ipc/schema";

import {
  applyExpansionMemory,
  clearExpansionMemory,
} from "$lib/modules/treeExpansion";

import {
  showPreviewContent as showPreviewContentImpl,
  showGeneratedContent as showGeneratedContentImpl,
  clearPreview,
} from "$lib/modules/editor";

import type { AppState } from "$lib/types";

declare global {
  interface Window {
    render: (newState: AppState) => void;
    updateScanProgress: (progress: {
      files_scanned: number;
      current_scanning_path: string;
      large_files_skipped: number;
    }) => void;
    showPreviewContent: (
      content: string,
      language: string,
      searchTerm: string | null | undefined,
      path: string
    ) => void;
    showGeneratedContent: (content: string, tokenCount: number) => void;
    showError: (msg: string) => void;
    showStatus: (msg: string) => void;
    fileSaveStatus: (success: boolean, path: string) => void;
    setDragState: (isDragging: boolean) => void;
    __APP_READY?: boolean;
  }
}

/** Install window.* handlers exactly once (re-install is harmless but avoided). */
export function installWindowIPCHandlers(): void {
  // If already installed, return early.
  if (
    typeof window.render === "function" &&
    typeof window.showStatus === "function"
  ) {
    return;
  }

  let lastPath: string | null = null;

  window.render = (incoming: AppState) => {
    const parsed = UiStateSchema.safeParse(incoming);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid render() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const input = parsed.data as unknown as AppState;

    try {
      const prev = getState();
      const nextPath = input?.current_path ?? null;
      const safeTree = Array.isArray(input?.tree) ? input.tree : [];

      if (input?.status_message) {
        input.status_message = input.status_message.startsWith("Status:")
          ? input.status_message
          : `Status: ${input.status_message}`;
      } else {
        input.status_message = "Status: Ready.";
      }

      if (lastPath !== nextPath) clearExpansionMemory();

      const patched: AppState = {
        ...input,
        tree: applyExpansionMemory(safeTree),
      };
      appState.set(patched);

      if (prev.current_path && !patched.current_path) clearPreview();

      lastPath = nextPath;
    } catch (err) {
      console.error("render() failed:", err);
      appState.update((s) => {
        s.status_message = "Error: Failed to render state.";
        return s;
      });
      toast.error("Failed to render state");
    }
  };

  window.updateScanProgress = (progress: {
    files_scanned: number;
    current_scanning_path: string;
    large_files_skipped: number;
  }) => {
    if (!getState().is_scanning) return;

    const parsed = ScanProgressSchema.safeParse(progress);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid updateScanProgress() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const { files_scanned, current_scanning_path, large_files_skipped } =
      parsed.data;

    const tr = get(tStore);

    const scanTextEl = document.querySelector(".scan-text");
    if (scanTextEl)
      (scanTextEl as HTMLElement).textContent = tr("filetree.scanning");

    const filesCountEl = document.getElementById("scan-files-count");
    if (filesCountEl)
      (filesCountEl as HTMLElement).textContent =
        `${files_scanned} files processed`;

    const currentPathEl = document.getElementById("scan-current-path");
    if (currentPathEl)
      (currentPathEl as HTMLElement).textContent =
        current_scanning_path || "Processing...";

    const skippedEl = document.getElementById("scan-skipped-count");
    if (skippedEl) {
      (skippedEl as HTMLElement).textContent =
        large_files_skipped > 0
          ? `${large_files_skipped} large files skipped`
          : "";
      (skippedEl as HTMLElement).style.display =
        large_files_skipped > 0 ? "inline" : "none";
    }

    const fillEl = document.getElementById("scan-progress-fill");
    if (fillEl && files_scanned > 0) {
      (fillEl as HTMLElement).style.width =
        `${Math.min(90, (files_scanned / 1000) * 100)}%`;
    }
  };

  window.showPreviewContent = (
    content: string,
    language: string,
    searchTerm: string | null | undefined,
    path: string
  ) => {
    const parsed = ShowPreviewArgsSchema.safeParse([
      content,
      language,
      searchTerm,
      path,
    ]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showPreviewContent() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const [c, l, s, p] = parsed.data;
    showPreviewContentImpl(c, l, s ?? "", p);
  };

  window.showGeneratedContent = (content: string, tokenCount: number) => {
    const parsed = ShowGeneratedArgsSchema.safeParse([content, tokenCount]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showGeneratedContent() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const [c, t] = parsed.data;
    showGeneratedContentImpl(c, t);
  };

  window.showError = (msg: string) => {
    const parsed = StatusMessageSchema.safeParse(msg);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showError() payload:",
        parsed.error.flatten()
      );
      return;
    }
    appState.update((s: AppState) => {
      s.status_message = `Error: ${parsed.data}`;
      return s;
    });
    const tr = get(tStore);
    toast.error(tr("toast.renderFailed"));
  };

  window.showStatus = (msg: string) => {
    const parsed = StatusMessageSchema.safeParse(msg);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showStatus() payload:",
        parsed.error.flatten()
      );
      return;
    }
    appState.update((s: AppState) => {
      s.status_message = `Status: ${parsed.data}`;
      return s;
    });
  };

  window.fileSaveStatus = (success: boolean, path: string) => {
    const parsed = FileSaveStatusArgsSchema.safeParse([success, path]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid fileSaveStatus() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const [ok, p] = parsed.data;
    const tr = get(tStore);

    const msg =
      p === "cancelled"
        ? "Status: Save cancelled."
        : ok
          ? `Status: Saved to ${p}`
          : "Error: Failed to save file.";

    appState.update((s: AppState) => {
      s.status_message = msg;
      return s;
    });

    if (p === "cancelled") {
      toast.info(tr("toast.saveCancelled"));
    } else if (ok) {
      toast.success(tr("toast.fileSaved"));
    } else {
      toast.error(tr("toast.saveFailed"));
    }
  };

  window.setDragState = (isDragging: boolean) => {
    const parsed = DragStateSchema.safeParse(isDragging);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid setDragState() payload:",
        parsed.error.flatten()
      );
      return;
    }
    const container = document.getElementById("file-tree-container");
    if (parsed.data) container?.classList.add("drag-over");
    else container?.classList.remove("drag-over");
  };
}
