import { describe, it, expect } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { availableLocales, translate } from "$lib/i18n";

/**
 * `translate()` returns the key itself when a locale has no message for it,
 * so a missing key shows up in the UI as e.g. "filetree.stats.visible".
 * This guards every literal key used in the sources, for every locale.
 */

const SRC_DIR = join(process.cwd(), "src");
// t("key") / $t("key") / tr("key") / translate("key")
const KEY_CALL = /(?<![\w.])(?:\$?t|tr|translate)\(\s*(["'])([\w.-]+)\1/g;

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      return entry.name === "tests" ? [] : sourceFiles(full);
    }
    return /\.(ts|svelte)$/.test(entry.name) ? [full] : [];
  });
}

function usedKeys(): Map<string, string> {
  const keys = new Map<string, string>();
  for (const file of sourceFiles(SRC_DIR)) {
    const text = readFileSync(file, "utf8");
    for (const match of text.matchAll(KEY_CALL)) {
      if (!keys.has(match[2])) keys.set(match[2], relative(SRC_DIR, file));
    }
  }
  return keys;
}

describe("i18n key coverage", () => {
  const keys = usedKeys();

  it("finds the translation keys used in the sources", () => {
    expect(keys.size).toBeGreaterThan(20);
    expect(keys.has("filetree.stats.visible")).toBe(true);
  });

  it.each(["en", "de"])("locale %s defines every used key", (locale) => {
    expect(availableLocales()).toContain(locale);
    const missing = [...keys]
      .filter(([key]) => translate(key, undefined, locale) === key)
      .map(([key, file]) => `${key} (${file})`);
    expect(missing).toEqual([]);
  });
});
