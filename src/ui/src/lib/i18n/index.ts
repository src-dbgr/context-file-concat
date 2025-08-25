// Lightweight i18n helper (Svelte-agnostic core + Svelte store adapters)
// - No `any` types; passes eslint rules
// - Flat key dictionary: "sidebar.title", "filetree.placeholder.chooseDir", ...
// - Message values can be strings or (params)=>string
// - Safe to import anywhere (browser/WebView context)

import { derived, writable, type Readable } from "svelte/store";

/** Parameter bag passed to message formatters. */
export type MsgParams = Record<string, unknown>;

/** A message can be a raw string or a formatter function. */
export type MessageValue<P extends MsgParams = MsgParams> =
  | string
  | ((params: P) => string);

/** Dictionary of translation messages for a single locale. */
export type Messages = Record<string, MessageValue>;

/** Locale code type – known locales plus open ended string. */
export type Locale = "en" | "de" | (string & {});

/** Registered locales → messages. */
const registry: Record<string, Messages> = Object.create(null);

/** Current locale store (defaults to "en"). */
const localeStore = writable<Locale>("en");

/** Get the current locale synchronously (for non-component modules). */
export function getLocale(): Locale {
  let current: Locale = "en";
  const unsub = localeStore.subscribe((v) => (current = v));
  unsub();
  return current;
}

/** Set current locale to an already registered locale. */
export function setLocale(locale: Locale): void {
  localeStore.set(locale);
}

/** Register (or replace) messages for a locale. */
export function register(locale: string, messages: Messages): void {
  registry[locale] = { ...(registry[locale] ?? {}), ...messages };
}

/** Return the list of registered locale codes. */
export function availableLocales(): string[] {
  return Object.keys(registry);
}

/** Simple `{name}` placeholder interpolation on strings. */
function interpolate(template: string, params?: MsgParams): string {
  if (!params) return template;
  return template.replace(/\{(\w+)\}/g, (_, key: string) => {
    const v = params[key];
    if (v === null || v === undefined) return "";
    if (typeof v === "string") return v;
    if (typeof v === "number" || typeof v === "boolean") return String(v);
    try {
      return JSON.stringify(v);
    } catch {
      return "";
    }
  });
}

/**
 * Translate a key for a given (or the current) locale.
 * Falls back to the key itself if nothing is found.
 */
export function translate(
  key: string,
  params?: MsgParams,
  locale?: string
): string {
  const loc = locale ?? getLocale();
  const dict = registry[loc] ?? registry["en"];
  if (!dict) return key;

  const value = dict[key];
  if (typeof value === "string") return interpolate(value, params);
  if (typeof value === "function") return value((params ?? {}) as MsgParams);
  return key;
}

/** Svelte store API: a readable of the current locale string. */
export const locale: Readable<Locale> = {
  subscribe: localeStore.subscribe,
};

/** Svelte-friendly translator store */
export const t: Readable<(key: string, params?: MsgParams) => string> = derived(
  localeStore,
  ($loc) => (key: string, params?: MsgParams) => translate(key, params, $loc)
);

/* ----------------------------------------------------------------------------
 * Built-in EN/DE messages. Includes both your new key scheme (screenshot)
 * and our earlier keys for backward compatibility.
 * -------------------------------------------------------------------------- */

register("en", {
  // ===== Common / Actions =====
  "action.selectDirectory": "Select Directory",
  "action.importConfig": "Import Config",
  "action.exportConfig": "Export Config",
  "action.add": "Add",
  "action.generate": "Generate",
  "action.saveToFile": "Save to File",

  // ===== Sidebar =====
  "sidebar.title": "Search & Filter",
  "sidebar.ph.selectDirFirst": "Select a directory first...",
  "sidebar.caseSensitive": "Case Sensitive",

  "sidebar.ignoreTitle": "Ignore Patterns",
  "sidebar.res": "Re-Scan",
  "sidebar.rescan": "Re-Scan",
  "sidebar.ph.addPattern": "Add pattern (*.log, build/)",
  "sidebar.removeAll": "Delete All",
  "sidebar.removeEmptyDirs": "Remove empty dirs",
  "sidebar.ph.filterAssigned": "Filter currently assigned ignore patterns...",

  // ===== Filetree placeholder =====
  "filetree.placeholder.chooseDir": "Choose Directory",
  "filetree.placeholder.dropHelp": "Drag & drop or click to browse",

  // ===== Preview =====
  "preview.defaultTitle": "Preview",
  "preview.selectAFile": "Select a file to preview",

  // ===== Status / Common =====
  "common.output": "Output",

  // ------- Legacy/earlier keys we still ship -------
  "preview.title": "Preview",
  "preview.status.idle": "Select a file to preview",
  "preview.status.generated": "Editable",
  "filetree.title": "Files",
  "filetree.none_in_dir": "No files found in this directory.",
  "filetree.none_for_filter": "No files found matching filters.",
  "filetree.choose_dir": "Choose Directory",
  "filetree.scanning": "Scanning directory...",
  "filetree.files_processed": "{count} files processed",
  "filetree.large_skipped": "{count} large files skipped",
  "filetree.selectAll": "Select all",
  "filetree.deselectAll": "Deselect all",
  "filetree.expandAll": "Expand all",
  "filetree.collapseAll": "Collapse all",
  "filetree.stats.files": "Files selected",
  "filetree.stats.selectedOf": "of total files",
  "filetree.stats.folders": "Folders",
  "header.select_directory": "Select Directory",
  "header.scanning": "Scanning...",
  "header.clear": "Clear",
  "header.import_config": "Import Config",
  "header.export_config": "Export Config",
  "sidebar.search.title": "Search & Filter",
  "sidebar.search.filenames": "Search filenames...",
  "sidebar.search.extension": "Filter by extension (e.g., rs, py)",
  "sidebar.search.content": "Search text inside files...",
  "sidebar.search.case_sensitive": "Case Sensitive",
  "sidebar.ignore.add_placeholder": "Add pattern (*.log, build/)",
  "sidebar.ignore.delete_all": "Delete All",
  "sidebar.ignore.remove_empty_dirs": "Remove empty dirs",
  "sidebar.ignore.common_label": "Common Ignore Pattern:",
  "footer.generate": "Generate",
  "footer.concat": "Concat{dots}",
  "footer.cancel": "Cancel",
  "footer.save": "Save to File",
  "toast.copied": "Copied to clipboard",
  "toast.copy_failed": "Failed to copy to clipboard",
  "toast.pasted": "Pasted content",
  "toast.paste_cancelled": "Paste cancelled",
  "toast.paste_empty": "Clipboard is empty",
  "toast.cut_ok": "Cut to clipboard",
  "toast.cut_failed": "Cut failed",
  "status.ready": "Status: Ready.",
  "status.save_cancelled": "Status: Save cancelled.",
  "status.saved_to": "Status: Saved to {path}",
  "error.save_failed": "Error: Failed to save file.",
  "error.render_failed": "Error: Failed to render state.",
});

register("de", {
  // ===== Common / Actions =====
  "action.selectDirectory": "Verzeichnis wählen",
  "action.importConfig": "Konfig. importieren",
  "action.exportConfig": "Konfig. exportieren",
  "action.add": "Hinzufügen",
  "action.generate": "Erzeugen",
  "action.saveToFile": "In Datei speichern",

  // ===== Sidebar =====
  "sidebar.title": "Suche & Filter",
  "sidebar.ph.selectDirFirst": "Zuerst ein Verzeichnis wählen...",
  "sidebar.caseSensitive": "Groß-/Kleinschreibung beachten",

  "sidebar.ignoreTitle": "Ignore-Muster",
  "sidebar.res": "Neu scannen",
  "sidebar.rescan": "Neu scannen",
  "sidebar.ph.addPattern": "Muster hinzufügen (*.log, build/)",
  "sidebar.removeAll": "Alle löschen",
  "sidebar.removeEmptyDirs": "Leere Ordner entfernen",
  "sidebar.ph.filterAssigned": "Zugewiesene Ignore-Muster filtern...",

  // ===== Filetree placeholder =====
  "filetree.placeholder.chooseDir": "Verzeichnis wählen",
  "filetree.placeholder.dropHelp":
    "Per Drag & Drop ablegen oder klicken zum Auswählen",

  // ===== Preview =====
  "preview.defaultTitle": "Vorschau",
  "preview.selectAFile": "Wähle eine Datei zur Vorschau",

  // ===== Status / Common =====
  "common.output": "Ausgabe",

  // ------- Legacy/earlier keys we still ship -------
  "preview.title": "Vorschau",
  "preview.status.idle": "Wähle eine Datei zur Vorschau",
  "preview.status.generated": "Editierbar",
  "filetree.title": "Dateien",
  "filetree.none_in_dir": "Keine Dateien in diesem Verzeichnis gefunden.",
  "filetree.none_for_filter": "Keine Dateien für die Filter gefunden.",
  "filetree.choose_dir": "Verzeichnis wählen",
  "filetree.scanning": "Verzeichnis wird gescannt...",
  "filetree.files_processed": "{count} Dateien verarbeitet",
  "filetree.large_skipped": "{count} große Dateien übersprungen",
  "filetree.selectAll": "Alles auswählen",
  "filetree.deselectAll": "Alles abwählen",
  "filetree.expandAll": "Aufklappen",
  "filetree.collapseAll": "Einklappen",
  "filetree.stats.files": "Dateien selektiert",
  "filetree.stats.selectedOf": "von gesamt Dateien",
  "filetree.stats.folders": "Verzeichnisse",
  "header.select_directory": "Verzeichnis wählen",
  "header.scanning": "Scanne...",
  "header.clear": "Leeren",
  "header.import_config": "Konfig. importieren",
  "header.export_config": "Konfig. exportieren",
  "sidebar.search.title": "Suche & Filter",
  "sidebar.search.filenames": "Dateinamen durchsuchen...",
  "sidebar.search.extension": "Nach Endung filtern (z. B. rs, py)",
  "sidebar.search.content": "Text in Dateien suchen...",
  "sidebar.search.case_sensitive": "Groß-/Kleinschreibung beachten",
  "sidebar.ignore.add_placeholder": "Muster hinzufügen (*.log, build/)",
  "sidebar.ignore.delete_all": "Alle löschen",
  "sidebar.ignore.remove_empty_dirs": "Leere Ordner entfernen",
  "sidebar.ignore.common_label": "Gängiges Ignore-Muster:",
  "footer.generate": "Erzeugen",
  "footer.concat": "Concat{dots}",
  "footer.cancel": "Abbrechen",
  "footer.save": "In Datei speichern",
  "toast.copied": "In Zwischenablage kopiert",
  "toast.copy_failed": "Kopieren fehlgeschlagen",
  "toast.pasted": "Inhalt eingefügt",
  "toast.paste_cancelled": "Einfügen abgebrochen",
  "toast.paste_empty": "Zwischenablage ist leer",
  "toast.cut_ok": "Inhalt ausgeschnitten",
  "toast.cut_failed": "Ausschneiden fehlgeschlagen",
  "status.ready": "Status: Bereit.",
  "status.save_cancelled": "Status: Speichern abgebrochen.",
  "status.saved_to": "Status: Gespeichert unter {path}",
  "error.save_failed": "Fehler: Datei konnte nicht gespeichert werden.",
  "error.render_failed": "Fehler: Rendering des Zustands fehlgeschlagen.",
});
