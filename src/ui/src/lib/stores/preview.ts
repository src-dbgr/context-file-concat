import { writable } from "svelte/store";

export type PreviewMode = "idle" | "file" | "generated";

/** Current preview mode (idle = nothing shown, file = read-only preview, generated = editable output) */
export const previewMode = writable<PreviewMode>("idle");

/** For generated previews, holds the token count for stats display (nullable when unknown) */
export const generatedTokenCount = writable<number | null>(null);

/** Outcome of the last copy of the editor content, shown briefly on the Copy button. */
export type CopyFeedback = "idle" | "copied" | "failed";

const COPY_FEEDBACK_MS = 1000;
const copyFeedbackStore = writable<CopyFeedback>("idle");
let copyFeedbackTimer: ReturnType<typeof setTimeout> | null = null;

export const copyFeedback = { subscribe: copyFeedbackStore.subscribe };

/** Show the outcome on the Copy button, then return to the normal label. */
export function flashCopyFeedback(
  outcome: Exclude<CopyFeedback, "idle">,
): void {
  if (copyFeedbackTimer) clearTimeout(copyFeedbackTimer);
  copyFeedbackStore.set(outcome);
  copyFeedbackTimer = setTimeout(() => {
    copyFeedbackTimer = null;
    copyFeedbackStore.set("idle");
  }, COPY_FEEDBACK_MS);
}
