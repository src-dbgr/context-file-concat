import { describe, it, expect } from "vitest";
import {
  accessibleTheme,
  contrastRatio,
  readable,
  TEXT_CONTRAST,
  type BaseTheme,
} from "$lib/modules/editorTheme";

// Monaco's real themes. By file path: the package name is mapped to a stub
// in the unit tests (vitest.config.ts).
const themesFile =
  "../../../node_modules/monaco-editor/esm/vs/editor/standalone/common/themes.js";
const { vs, vs_dark } = (await import(/* @vite-ignore */ themesFile)) as {
  vs: BaseTheme;
  vs_dark: BaseTheme;
};

describe("contrastRatio", () => {
  it("is the ratio WCAG defines", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("ffffff", "ffffff")).toBe(1);
    // A pair with a known ratio: #767676 is the lightest gray with 4.5:1.
    expect(contrastRatio("#767676", "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio("#777777", "#ffffff")).toBeLessThan(4.5);
  });
});

describe("readable", () => {
  it("keeps a color that has enough contrast", () => {
    expect(readable("569cd6", "1e1e1e")).toBe("569cd6");
    expect(readable("#0000ff", "#fffffe")).toBe("#0000ff");
  });

  it("lightens a color on a dark background until it has", () => {
    const comment = readable("608b4e", "1e1e1e");
    expect(comment).not.toBe("608b4e");
    expect(contrastRatio(comment, "1e1e1e")).toBeGreaterThanOrEqual(4.5);
    // No further than needed: one step back is below the limit.
    expect(contrastRatio(comment, "1e1e1e")).toBeLessThan(5);
    expect(contrastRatio(comment, "ffffff")).toBeGreaterThan(1);
  });

  it("darkens a color on a light background, in the notation it came in", () => {
    const red = readable("#ff0000", "#fffffe");
    expect(red).toMatch(/^#[0-9a-f]{6}$/);
    expect(contrastRatio(red, "#fffffe")).toBeGreaterThanOrEqual(4.5);
    expect(contrastRatio(red, "#fffffe")).toBeLessThan(5);
  });
});

describe.each([
  ["vs", vs, "fffffe"],
  ["vs-dark", vs_dark, "1e1e1e"],
] as const)("accessibleTheme(%s)", (name, base, background) => {
  const theme = accessibleTheme(base);

  it("inherits from the theme it is made of", () => {
    expect(theme.base).toBe(name);
    expect(theme.inherit).toBe(true);
  });

  it("leaves no text color below 4.5:1 on the editor background", () => {
    const changed = new Map(theme.rules.map((rule) => [rule.token, rule]));
    // Monaco's theme has colors below the limit, or this theme is not needed.
    expect(changed.size).toBeGreaterThan(0);
    for (const rule of base.rules) {
      if (!rule.foreground) continue;
      const foreground = changed.get(rule.token)?.foreground ?? rule.foreground;
      expect(
        contrastRatio(foreground, background),
        `token "${rule.token}"`,
      ).toBeGreaterThanOrEqual(TEXT_CONTRAST);
    }
    for (const color of Object.values(theme.colors)) {
      expect(contrastRatio(color, background)).toBeGreaterThanOrEqual(
        TEXT_CONTRAST,
      );
    }
  });

  it("changes only the rules that need it, and only their color", () => {
    for (const rule of theme.rules) {
      const original = base.rules.find((r) => r.token === rule.token)!;
      expect(contrastRatio(original.foreground!, background)).toBeLessThan(
        TEXT_CONTRAST,
      );
      expect({ ...rule, foreground: original.foreground }).toEqual(original);
    }
  });
});
