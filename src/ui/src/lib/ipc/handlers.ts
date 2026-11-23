/**
 * Window IPC handlers extracted from main.ts
 * - Zod-validated
 * - TS strict, no any
 * - Idempotent install
 */

import { appState, getState } from "$lib/stores/app";
import { scanProgress } from "$lib/stores/scanProgress";
import { toast } from "$lib/stores/toast";
import { truncate } from "$lib/utils";
import { t as tStore } from "$lib/i18n";
import { get } from "svelte/store";

import {
  UiStateSchema,
  UiStateEnvelopeSchema,
  ScanProgressSchema,
  ShowPreviewArgsSchema,
  ShowGeneratedArgsSchema,
  StatusMessageSchema,
  FileSaveStatusArgsSchema,
  DragStateSchema,
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
    showGeneratedContent: (content: string, tokenCount: number) => void;
    showError: (msg: string) => void;
    showStatus: (msg: string) => void;
    fileSaveStatus: (success: boolean, path: string) => void;
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
    // Dev/tests: full deep validation. Prod: envelope only, so the recursive
    // tree walk stays off the hot path (same DEV switch as backend.post()).
    const schema = import.meta.env.DEV ? UiStateSchema : UiStateEnvelopeSchema;
    const parsed = schema.safeParse(incoming);
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
      // is authoritative: it decides which children are part of the tree.
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

  window.showGeneratedContent = (content: string, tokenCount: number) => {
    const parsed = ShowGeneratedArgsSchema.safeParse([content, tokenCount]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid showGeneratedContent() payload:",
        parsed.error.flatten(),
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

  window.fileSaveStatus = (success: boolean, path: string) => {
    const parsed = FileSaveStatusArgsSchema.safeParse([success, path]);
    if (!parsed.success) {
      console.warn(
        "[IPC] Ignored invalid fileSaveStatus() payload:",
        parsed.error.flatten(),
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
        parsed.error.flatten(),
      );
      return;
    }
    const container = document.getElementById("file-tree-container");
    if (parsed.data) container?.classList.add("drag-over");
    else container?.classList.remove("drag-over");
  };
}
