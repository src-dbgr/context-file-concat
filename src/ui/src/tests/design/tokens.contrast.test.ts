import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { contrastRatio } from "$lib/modules/editorTheme";

/**
 * The contrast the design tokens promise (see the head of tokens.css), read
 * from the palettes themselves: WCAG 2.1 AA, 4.5:1 for text and 3:1 for what
 * shows a control or its state. The screens are checked by e2e/a11y.spec.ts;
 * this pins the tokens, so a color that is changed fails here first.
 */

const css = readFileSync(
  join(process.cwd(), "src/lib/design/tokens.css"),
  "utf8",
);

type Theme = "light" | "dark";

function palette(theme: Theme): Record<string, string> {
  const colors: Record<string, string> = {};
  const pattern = new RegExp(
    `--palette-${theme}-([\\w-]+):\\s*(#[0-9a-fA-F]{6})\\b`,
    "g",
  );
  for (const [, name, value] of css.matchAll(pattern)) colors[name] = value;
  return colors;
}

/** `amount` of `color` over `base`, as `color-mix(in srgb, …)` paints it. */
function tint(color: string, amount: number, base: string): string {
  const channel = (hex: string, i: number) =>
    parseInt(hex.slice(1 + 2 * i, 3 + 2 * i), 16);
  return (
    "#" +
    [0, 1, 2]
      .map((i) =>
        Math.round(channel(color, i) * amount + channel(base, i) * (1 - amount))
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}

describe.each<Theme>(["light", "dark"])(
  "contrast of the %s palette",
  (theme) => {
    const p = palette(theme);
    const surfaces = ["bg", "surface-1", "surface-2"];

    it("defines every color the mapping of the theme uses", () => {
      for (const name of [
        ...surfaces,
        "border",
        "text",
        "muted",
        "accent",
        "success",
        "warning",
        "info",
        "error",
        "on-accent",
        "accent-text",
        "success-text",
        "warning-text",
        "error-text",
        "success-solid",
        "error-solid",
        "on-solid",
        "on-info",
        "control-border",
        "focus",
      ]) {
        expect(p[name], `--palette-${theme}-${name}`).toMatch(
          /^#[0-9a-f]{6}$/i,
        );
      }
    });

    it.each([
      "text",
      "muted",
      "accent-text",
      "success-text",
      "warning-text",
      "error-text",
    ])("%s is readable on every surface (4.5:1)", (name) => {
      for (const surface of surfaces) {
        expect(
          contrastRatio(p[name], p[surface]),
          `${name} on ${surface}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    });

    it("text is readable on the tinted surfaces (4.5:1)", () => {
      // The previewed row of the tree: 20% accent over the background.
      const previewed = tint(p.accent, 0.2, p.bg);
      for (const name of ["text", "muted", "warning-text", "accent-text"]) {
        expect(
          contrastRatio(p[name], previewed),
          `${name} on the previewed row`,
        ).toBeGreaterThanOrEqual(4.5);
      }
      // The chip of an active ignore pattern: 15% success over surface-2.
      expect(
        contrastRatio(p["success-text"], tint(p.success, 0.15, p["surface-2"])),
        "success-text on an active chip",
      ).toBeGreaterThanOrEqual(4.5);
    });

    it("text on a filled button is readable (4.5:1)", () => {
      for (const solid of ["success-solid", "error-solid"]) {
        expect(
          contrastRatio(p["on-solid"], p[solid]),
          `on-solid on ${solid}`,
        ).toBeGreaterThanOrEqual(4.5);
      }
      expect(contrastRatio(p["on-info"], p.info)).toBeGreaterThanOrEqual(4.5);
    });

    it("what shows a control, its state or the focus stands out (3:1)", () => {
      // The edge of a form control and the focus ring: on every surface
      // (the text fields of the sidebar stand on surface-2).
      for (const name of ["control-border", "focus"]) {
        for (const surface of surfaces) {
          expect(
            contrastRatio(p[name], p[surface]),
            `${name} on ${surface}`,
          ).toBeGreaterThanOrEqual(3);
        }
      }
      for (const surface of ["bg", "surface-1"]) {
        expect(
          contrastRatio(p.accent, p[surface]),
          `accent on ${surface}`,
        ).toBeGreaterThanOrEqual(3);
      }
    });
  },
);
