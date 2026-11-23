/**
 * Window IPC handlers extracted from main.ts
 * - Zod-validated
 * - TS strict, no any
 * - Idempotent install
 */

import { appState, getState } from "$lib/stores/app";
import { scanProgress } from "$lib/stores/scanProgress";
import { toast } from "$lib/stores/toast";
import { formatFileSize, truncate } from "$lib/utils";
import { t as tStore } from "$lib/i18n";
import { get } from "svelte/store";

import {
  UiStateSchema,
  ScanProgressSchema,
  ShowPreviewArgsSchema,
  ShowGeneratedArgsSchema,
  StatusMessageSchema,
  SaveResultSchema,
  OutputSizeNoticeSchema,
  DragStateSchema,
  type SaveResult,
  type OutputSizeNotice,
} from "$lib/ipc/schema";

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
      path: string,
    ) => void;
    showGeneratedContent: (
      content: string,
      tokenCount: number,
      documentId: number,
    ) => void;
    outputSizeNotice: (notice: OutputSizeNotice) => void;
    showError: (msg: string) => void;
    showStatus: (msg: string) => void;
    saveResult: (result: SaveResult) => void;
    setDragState: (isDragging: boolean) => void;
    __APP_READY?: boolean;
  }
}

/** How long an error reported by the host stays on screen. */
const HOST_ERROR_TOAST_MS = 10_000;
/** Upper bound for the host text shown in a toast. */
const HOST_ERROR_MAX_CHARS = 300;

/** Install window.* handlers exactly once (re-install is harmless but avoided). */
export function installWindowIPCHandlers(): void {
  // If already installed, return early.
  if (
    typeof window.render === "function" &&
    typeof window.showStatus === "function"
  ) {
    return;
  }

  window.render = (incoming: AppState) => {
    // The whole snapshot is validated, rows included, in every build: it
    // holds a window of rows, so the cost does not grow with the project.
    const parsed = UiStateSchema.safeParse(incoming);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid render() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    const input = parsed.data as unknown as AppState;

    try {
      const prev = getState();

      if (input?.status_message) {
        input.status_message = input.status_message.startsWith("Status:")
          ? input.status_message
          : `Status: ${input.status_message}`;
      } else {
        input.status_message = "Status: Ready.";
      }

      // The snapshot is taken as is. In particular the backend's `is_expanded`
      // is authoritative: it decides which rows the tree has.
      appState.set(input);

      // The progress shown belongs to one scan: forget it when none is
      // running and when a new one starts.
      if (!input.is_scanning || !prev.is_scanning) scanProgress.reset();

      if (prev.current_path && !input.current_path) clearPreview();
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
        parsed.error.flatten(),
      );
      return;
    }
    scanProgress.report({
      files_scanned: parsed.data.files_scanned,
      large_files_skipped: parsed.data.large_files_skipped,
      current_scanning_path: parsed.data.current_scanning_path,
    });
  };

  window.showPreviewContent = (
    content: string,
    language: string,
    searchTerm: string | null | undefined,
    path: string,
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
        parsed.error.flatten(),
      );
      return;
    }
    const [c, l, s, p] = parsed.data;
    showPreviewContentImpl(c, l, s ?? "", p);
  };

  window.showGeneratedContent = (
    content: string,
    tokenCount: number,
    documentId: number,
  ) => {
    const parsed = ShowGeneratedArgsSchema.safeParse([
      content,
      tokenCount,
      documentId,
    ]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showGeneratedContent() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    const [c, t, id] = parsed.data;
    showGeneratedContentImpl(c, t, id);
  };

  window.outputSizeNotice = (notice: OutputSizeNotice) => {
    const parsed = OutputSizeNoticeSchema.safeParse(notice);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid outputSizeNotice() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    const tr = get(tStore);
    const sizes = {
      size: formatFileSize(parsed.data.estimated_bytes),
      limit: formatFileSize(parsed.data.limit_bytes),
    };
    if (parsed.data.status === "too_large") {
      // Nothing was generated; the reason has to stay readable for a while.
      toast.error(tr("toast.outputTooLarge", sizes), {
        title: tr("toast.outputTooLargeTitle"),
        duration: HOST_ERROR_TOAST_MS,
      });
    } else {
      toast.warning(tr("toast.outputLarge", sizes), {
        title: tr("toast.outputLargeTitle"),
      });
    }
  };

  window.showError = (msg: string) => {
    const parsed = StatusMessageSchema.safeParse(msg);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showError() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    appState.update((s: AppState) => {
      s.status_message = `Error: ${parsed.data}`;
      return s;
    });
    // The toast carries the message itself: the status line is overwritten
    // by the next state snapshot.
    const tr = get(tStore);
    const text = parsed.data.trim();
    toast.error(
      text ? truncate(text, HOST_ERROR_MAX_CHARS) : tr("toast.renderFailed"),
      { title: tr("toast.errorTitle"), duration: HOST_ERROR_TOAST_MS },
    );
  };

  window.showStatus = (msg: string) => {
    const parsed = StatusMessageSchema.safeParse(msg);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showStatus() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    appState.update((s: AppState) => {
      s.status_message = `Status: ${parsed.data}`;
      return s;
    });
  };

  window.saveResult = (result: SaveResult) => {
    const parsed = SaveResultSchema.safeParse(result);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid saveResult() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    const outcome = parsed.data;
    const tr = get(tStore);
    const setStatus = (message: string) =>
      appState.update((s: AppState) => {
        s.status_message = message;
        return s;
      });

    switch (outcome.status) {
      case "saved":
        setStatus(`Status: Saved to ${outcome.path}`);
        toast.success(tr("toast.fileSaved"));
        break;
      case "cancelled":
        setStatus("Status: Save cancelled.");
        toast.info(tr("toast.saveCancelled"));
        break;
      case "failed": {
        setStatus("Error: Failed to save file.");
        // The reason is the host's (the message of the operating system);
        // the toast says what failed and shows the reason below it.
        const reason = outcome.error.trim();
        if (reason) {
          toast.error(truncate(reason, HOST_ERROR_MAX_CHARS), {
            title: tr("toast.saveFailed"),
            duration: HOST_ERROR_TOAST_MS,
          });
        } else {
          toast.error(tr("toast.saveFailed"));
        }
        break;
      }
    }
  };

  window.setDragState = (isDragging: boolean) => {
    const parsed = DragStateSchema.safeParse(isDragging);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid setDragState() payload:",
        parsed.error.flatten(),
      );
      return;
    }
    const container = document.getElementById("file-tree-container");
    if (parsed.data) container?.classList.add("drag-over");
    else container?.classList.remove("drag-over");
  };
}
