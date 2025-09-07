import { writable } from "svelte/store";

export type PreviewMode = "idle" | "file" | "generated";

/** Current preview mode (idle = nothing shown, file = read-only preview, generated = editable output) */
export const previewMode = writable<PreviewMode>("idle");

/** For generated previews, holds the token count for stats display (nullable when unknown) */
export const generatedTokenCount = writable<number | null>(null);
