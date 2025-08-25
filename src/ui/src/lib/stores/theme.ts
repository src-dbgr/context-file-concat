import { writable } from "svelte/store";

export type Theme = "light" | "dark";

const STORAGE_KEY = "cfc-theme";

/** Safely read localStorage value (if available) */
function readStoredTheme(): Theme | null {
  try {
    const v = localStorage.getItem(STORAGE_KEY);
    return v === "light" || v === "dark" ? v : null;
  } catch {
    return null;
  }
}

/** Detect system preference (fallback if nothing stored) */
function detectSystemTheme(): Theme {
  if (typeof window !== "undefined" && "matchMedia" in window) {
    try {
      return window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light";
    } catch {
      /* no-op */
    }
  }
  return "dark";
}

/** Apply theme on the document element */
function applyThemeAttr(t: Theme) {
  if (typeof document === "undefined") return;
  const root = document.documentElement;
  // Keep attribute explicit for both themes; avoids default ambiguity.
  root.setAttribute("data-theme", t);
}

/** Persist chosen theme */
function persistTheme(t: Theme) {
  try {
    localStorage.setItem(STORAGE_KEY, t);
  } catch {
    /* ignore storage errors in sandboxed/locked environments */
  }
}

/** Initialize current theme value */
const initialTheme: Theme = readStoredTheme() ?? detectSystemTheme();

/**
 * Central theme store. Subscribing applies DOM attribute and persists.
 * Note: this module has no Svelte component dependencies and is safe to import anywhere.
 */
export const theme = writable<Theme>(initialTheme);
theme.subscribe((t) => {
  applyThemeAttr(t);
  persistTheme(t);
});

/** Public helpers */
export function setTheme(t: Theme) {
  theme.set(t);
}
export function toggleTheme() {
  theme.update((t) => (t === "light" ? "dark" : "light"));
}

/**
 * Optional explicit init hook (idempotent).
 * Kept for clarity if one wants to call from main.ts before mounting.
 */
export function initTheme() {
  applyThemeAttr(initialTheme);
}
