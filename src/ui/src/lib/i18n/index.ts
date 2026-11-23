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
  string | ((params: P) => string);

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
  locale?: string,
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
  ($loc) => (key: string, params?: MsgParams) => translate(key, params, $loc),
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
  "action.clear": "Clear",
  "action.generate": "Generate",
  "action.saveToFile": "Save to File",
  "action.noDirectorySelected": "No Directory selected",

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
  "preview.copy": "Copy",
  "preview.clear": "Clear",
  "preview.generated": "Preview generated",

  // ===== Status / Common =====
  "common.output": "Output",

  // ===== Footer / Output =====
  "action.browse": "Choose Dir",
  "action.cancel": "Cancel",
  "status.dir": "Directory",
  "status.filename": "Filename",
  "status.includeTree": "Include File Tree",
  "status.relativePaths": "Relative File Paths",

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
  "stats.readOnly": "read only",
  "stats.editable": "editable",
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
  "sidebar.commonPatterns": "Common Ignore Pattern:",
  "sidebar.ignore.add_placeholder": "Add pattern (*.log, build/)",
  "sidebar.ignore.delete_all": "Delete All",
  "sidebar.ignore.remove_empty_dirs": "Remove empty dirs",
  "sidebar.ignore.common_label": "Common Ignore Pattern:",
  "sidebar.ph.searchFileNames": "Search for File names",
  "sidebar.ph.extensionFilter": "Search for File Types",
  "sidebar.ph.contentSearch": "Search inside Files",
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
  "toast.pasted_editor": "Pasted into editor",
  "toast.paste_unsupported": "Paste not supported here",
  "clipboard.copiedInput": "✓ Copied text from input field.",
  "clipboard.copiedEditorAll": "✓ Copied entire editor content.",
  "clipboard.copiedEditorSelection": "✓ Copied selected text from editor.",
  "clipboard.copyFailed": "✗ Failed to copy to clipboard.",
  "clipboard.pasted": "✓ Content pasted.",
  "clipboard.pasteFailed": "✗ Paste failed or clipboard empty.",
  "clipboard.pasteCancelled": "Status: Paste cancelled.",
  "clipboard.pasteEmpty": "Status: Clipboard is empty.",
  "clipboard.pasteUnsupported": "✗ Paste not supported here.",
  "clipboard.cutOk": "✓ Text cut to clipboard.",
  "clipboard.cutFailed": "✗ Cut failed.",
  "clipboard.pastePrompt":
    "Could not access clipboard. Please paste your text here:",
  "status.ready": "Status: Ready.",
  "status.save_cancelled": "Status: Save cancelled.",
  "status.saved_to": "Status: Saved to {path}",
  "error.save_failed": "Error: Failed to save file.",
  "error.render_failed": "Error: Failed to render state.",

  // ===== Keys used by components / IPC handlers =====
  "action.rescan": "Re-Scan",
  "sidebar.scanning": "Scanning...",
  "filetree.cancel": "Cancel",
  "filetree.emptyDir": "No files found in this directory.",
  "filetree.noMatch": "No files found matching filters.",
  "filetree.stats.visible": "Visible",
  "aria.scanProgress": "Scan progress",
  "aria.generatingPreview": "Generating preview",
  "status.indexing": "Indexing...",
  "toast.fileSaved": "File saved",
  "toast.saveCancelled": "Save cancelled",
  "toast.saveFailed": "Failed to save file",
  "toast.outputUnavailable":
    "The generated output is no longer available. Generate it again.",
  "toast.editorLoadFailed":
    "The editor could not be loaded, so the content cannot be shown. Try again.",
  "scan.cancelTitle": "Cancel the scan",
  "toast.outputLargeTitle": "Large output",
  "toast.outputLarge":
    "About {size} are being generated (more than {limit}). Showing the output will take a moment.",
  "toast.outputTooLargeTitle": "Output too large",
  "toast.outputTooLarge":
    "The selection would produce about {size}; the limit is {limit}. Select fewer files.",
  "toast.ignoreRefusedTitle": "Not ignored",
  "toast.ignoreRefusedWhitespace":
    "No ignore pattern can name \u201c{name}\u201d: the name ends with a whitespace character that a pattern drops. Nothing was added. Rename the file, or ignore its folder.",
  "toast.ignoreRefusedNotUnicode":
    "No ignore pattern can name \u201c{name}\u201d: the name is not valid Unicode. Nothing was added. Rename the entry, or ignore its folder.",
  "toast.renderFailed": "An error occurred",
  "toast.errorTitle": "Error",
  "preview.copied": "Copied!",
  "preview.copyFailed": "Failed",
  "preview.searchMatch": "Search match",
  "scan.starting": "Starting scan...",
  "scan.filesScanned": "Files scanned: {count}",
  "scan.largeFilesSkipped": "Large files skipped: {count}",

  // ===== Names of controls and regions (screen readers, tooltips) =====
  "app.title": "CFC – Context File Concatenator",
  "aria.projectFiles": "Project files",
  "aria.expandDirectory": "Expand {name}",
  "aria.collapseDirectory": "Collapse {name}",
  "aria.toggleDirectorySelection": "Toggle directory selection",
  "aria.toggleFileSelection": "Toggle file selection",
  "aria.ignoreDirectory": "Add directory to ignore patterns",
  "aria.ignoreFile": "Add file to ignore patterns",
  "aria.removePattern": "Remove pattern {pattern}",
  "aria.notifications": "Notifications",
  "aria.dismissNotification": "Dismiss notification",
  "filetree.ignoreDirectoryTitle":
    "Add this directory to ignore patterns ({key})",
  "filetree.ignoreFileTitle": "Add this file to ignore patterns ({key})",
  // Forward delete. On a Mac the key labelled "delete" is Backspace.
  "key.delete": "Delete",
  "key.deleteMac": "fn+⌫",
  "sidebar.rescanRecommended": "Re-scan recommended after removing patterns",
  "sidebar.addCommonPattern": 'Click to add "{pattern}" to ignore patterns',
  "sidebar.patternActive":
    "This pattern was active and matched one or more files/directories.",
  "theme.switchToDark": "Switch to dark mode",
  "theme.switchToLight": "Switch to light mode",
  "locale.current": "Language: English",
});

register("de", {
  // ===== Common / Actions =====
  "action.selectDirectory": "Verzeichnis wählen",
  "action.importConfig": "Konfiguration import",
  "action.exportConfig": "Konfiguration export",
  "action.add": "Hinzufügen",
  "action.clear": "Bereinigen",
  "action.generate": "Erzeugen",
  "action.saveToFile": "In Datei speichern",
  "action.noDirectorySelected": "Kein Verzeichnis ausgewählt",

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
  "preview.copy": "Kopieren",
  "preview.clear": "Löschen",
  "preview.generated": "Vorschau erstellt",

  // ===== Status / Common =====
  "common.output": "Ausgabe",

  // ===== Footer / Output =====
  "action.browse": "Auswählen",
  "action.cancel": "Abbrechen",
  "status.dir": "Verzeichnis",
  "status.filename": "Dateiname",
  "status.includeTree": "File Tree addieren",
  "status.relativePaths": "Relative Dateipfade",

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
  "stats.readOnly": "read only",
  "stats.editable": "editierbar",
  "header.select_directory": "Verzeichnis wählen",
  "header.scanning": "Scanne...",
  "header.clear": "Leeren",
  "header.import_config": "Konfig. importieren",
  "header.export_config": "Konfig. exportieren",
  "sidebar.commonPatterns": "Häufige Ignore-Muster",
  "sidebar.search.title": "Suche & Filter",
  "sidebar.search.filenames": "Dateinamen durchsuchen...",
  "sidebar.search.extension": "Nach Endung filtern (z. B. rs, py)",
  "sidebar.search.content": "Text in Dateien suchen...",
  "sidebar.search.case_sensitive": "Groß-/Kleinschreibung beachten",
  "sidebar.ignore.add_placeholder": "Muster hinzufügen (*.log, build/)",
  "sidebar.ignore.delete_all": "Alle löschen",
  "sidebar.ignore.remove_empty_dirs": "Leere Ordner entfernen",
  "sidebar.ignore.common_label": "Gängiges Ignore-Muster:",
  "sidebar.ph.searchFileNames": "Suche nach Dateinamen",
  "sidebar.ph.extensionFilter": "Suche nach Dateitypen",
  "sidebar.ph.contentSearch": "Suche innerhalb Dateien",
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
  "toast.pasted_editor": "In den Editor eingefügt",
  "toast.paste_unsupported": "Einfügen ist hier nicht möglich",
  "clipboard.copiedInput": "✓ Text aus dem Eingabefeld kopiert.",
  "clipboard.copiedEditorAll": "✓ Gesamten Editorinhalt kopiert.",
  "clipboard.copiedEditorSelection":
    "✓ Markierten Text aus dem Editor kopiert.",
  "clipboard.copyFailed": "✗ Kopieren in die Zwischenablage fehlgeschlagen.",
  "clipboard.pasted": "✓ Inhalt eingefügt.",
  "clipboard.pasteFailed":
    "✗ Einfügen fehlgeschlagen oder Zwischenablage leer.",
  "clipboard.pasteCancelled": "Status: Einfügen abgebrochen.",
  "clipboard.pasteEmpty": "Status: Zwischenablage ist leer.",
  "clipboard.pasteUnsupported": "✗ Einfügen ist hier nicht möglich.",
  "clipboard.cutOk": "✓ Text in die Zwischenablage ausgeschnitten.",
  "clipboard.cutFailed": "✗ Ausschneiden fehlgeschlagen.",
  "clipboard.pastePrompt":
    "Kein Zugriff auf die Zwischenablage. Bitte den Text hier einfügen:",
  "status.ready": "Status: Bereit.",
  "status.save_cancelled": "Status: Speichern abgebrochen.",
  "status.saved_to": "Status: Gespeichert unter {path}",
  "error.save_failed": "Fehler: Datei konnte nicht gespeichert werden.",
  "error.render_failed": "Fehler: Rendering des Zustands fehlgeschlagen.",

  // ===== Keys used by components / IPC handlers =====
  "action.rescan": "Neu scannen",
  "sidebar.scanning": "Scanne...",
  "filetree.cancel": "Abbrechen",
  "filetree.emptyDir": "Keine Dateien in diesem Verzeichnis gefunden.",
  "filetree.noMatch": "Keine Dateien für die Filter gefunden.",
  "filetree.stats.visible": "Sichtbar",
  "aria.scanProgress": "Scan-Fortschritt",
  "aria.generatingPreview": "Vorschau wird erzeugt",
  "status.indexing": "Indexiere...",
  "toast.fileSaved": "Datei gespeichert",
  "toast.saveCancelled": "Speichern abgebrochen",
  "toast.saveFailed": "Datei konnte nicht gespeichert werden",
  "toast.outputUnavailable":
    "Die erzeugte Ausgabe ist nicht mehr verfügbar. Bitte erneut erzeugen.",
  "toast.editorLoadFailed":
    "Der Editor konnte nicht geladen werden, der Inhalt kann nicht angezeigt werden. Bitte erneut versuchen.",
  "scan.cancelTitle": "Scan abbrechen",
  "toast.outputLargeTitle": "Große Ausgabe",
  "toast.outputLarge":
    "Es werden etwa {size} erzeugt (mehr als {limit}). Die Anzeige der Ausgabe dauert einen Moment.",
  "toast.outputTooLargeTitle": "Ausgabe zu groß",
  "toast.outputTooLarge":
    "Die Auswahl ergäbe etwa {size}; die Grenze liegt bei {limit}. Wähle weniger Dateien aus.",
  "toast.ignoreRefusedTitle": "Nicht ignoriert",
  "toast.ignoreRefusedWhitespace":
    "Kein Ignore-Muster kann \u201e{name}\u201c benennen: Der Name endet mit einem Leerraumzeichen, das ein Muster verwirft. Es wurde nichts hinzugefügt. Benenne die Datei um oder ignoriere ihren Ordner.",
  "toast.ignoreRefusedNotUnicode":
    "Kein Ignore-Muster kann \u201e{name}\u201c benennen: Der Name ist kein gültiges Unicode. Es wurde nichts hinzugefügt. Benenne den Eintrag um oder ignoriere seinen Ordner.",
  "toast.renderFailed": "Ein Fehler ist aufgetreten",
  "toast.errorTitle": "Fehler",
  "preview.copied": "Kopiert!",
  "preview.copyFailed": "Fehlgeschlagen",
  "preview.searchMatch": "Suchtreffer",
  "scan.starting": "Scan wird gestartet...",
  "scan.filesScanned": "Gescannte Dateien: {count}",
  "scan.largeFilesSkipped": "Große Dateien übersprungen: {count}",

  // ===== Names of controls and regions (screen readers, tooltips) =====
  "app.title": "CFC – Context File Concatenator",
  "aria.projectFiles": "Projektdateien",
  "aria.expandDirectory": "{name} aufklappen",
  "aria.collapseDirectory": "{name} einklappen",
  "aria.toggleDirectorySelection": "Verzeichnisauswahl umschalten",
  "aria.toggleFileSelection": "Dateiauswahl umschalten",
  "aria.ignoreDirectory": "Verzeichnis zu den Ignore-Mustern hinzufügen",
  "aria.ignoreFile": "Datei zu den Ignore-Mustern hinzufügen",
  "aria.removePattern": "Muster {pattern} entfernen",
  "aria.notifications": "Benachrichtigungen",
  "aria.dismissNotification": "Benachrichtigung schließen",
  "filetree.ignoreDirectoryTitle":
    "Dieses Verzeichnis zu den Ignore-Mustern hinzufügen ({key})",
  "filetree.ignoreFileTitle":
    "Diese Datei zu den Ignore-Mustern hinzufügen ({key})",
  "key.delete": "Entf",
  "key.deleteMac": "fn+⌫",
  "sidebar.rescanRecommended":
    "Nach dem Entfernen von Mustern wird ein erneuter Scan empfohlen",
  "sidebar.addCommonPattern":
    'Klicken, um "{pattern}" zu den Ignore-Mustern hinzuzufügen',
  "sidebar.patternActive":
    "Dieses Muster war aktiv und traf auf mindestens eine Datei oder ein Verzeichnis zu.",
  "theme.switchToDark": "Zum dunklen Design wechseln",
  "theme.switchToLight": "Zum hellen Design wechseln",
  "locale.current": "Sprache: Deutsch",
});
