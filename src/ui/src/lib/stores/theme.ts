// Theme store (Svelte 5+, TypeScript)
// Persists choice, respects prefers-color-scheme, and applies [data-theme] on <html>.

import { writable } from "svelte/store";

export type Theme = "light" | "dark";

const STORAGE_KEY = "cfc:theme";

function detectSystemTheme(): Theme {
  if (typeof window === "undefined") return "dark";
  return window.matchMedia &&
    window.matchMedia("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

function readInitial(): Theme {
  if (typeof window === "undefined") return "dark";
  const fromStorage = window.localStorage.getItem(STORAGE_KEY) as Theme | null;
  return fromStorage ?? detectSystemTheme();
}

export const theme = writable<Theme>(readInitial());

/** Apply theme attribute on :root (html element). */
function applyTheme(t: Theme) {
  if (typeof document === "undefined") return;
  document.documentElement.setAttribute("data-theme", t);
}

/** Initialize side effects once on app bootstrap. */
export function initTheme() {
  applyTheme(readInitial());
  const mq = window.matchMedia("(prefers-color-scheme: light)");
  const handler = () => {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (!stored) {
      // Only auto-follow system if user has no explicit preference.
      const sys = detectSystemTheme();
      theme.set(sys);
      applyTheme(sys);
    }
  };
  mq.addEventListener?.("change", handler);
  // No cleanup here; call from your bootstrap if needed.
}

/** Explicitly set a theme and persist. */
export function setTheme(next: Theme) {
  theme.set(next);
  if (typeof window !== "undefined") {
    window.localStorage.setItem(STORAGE_KEY, next);
  }
  applyTheme(next);
}

/** Toggle between light/dark. */
export function toggleTheme() {
  let current: Theme = "dark";
  const unsub = theme.subscribe((t) => (current = t));
  unsub();
  const next: Theme = current === "dark" ? "light" : "dark";
  setTheme(next);
}
