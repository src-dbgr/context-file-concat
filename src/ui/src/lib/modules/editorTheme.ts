/**
 * The color themes of the editor: Monaco's own `vs` and `vs-dark`, with every
 * text color that has less contrast to the editor background than WCAG AA
 * asks for (4.5:1) moved towards black or white until it has. A color that
 * has enough contrast stays as it is, so the themes look like the ones they
 * are made of.
 *
 * Pure functions; `monacoBundle` hands in Monaco's themes and registers the
 * result. Colors are written the way Monaco's token rules write them: six hex
 * digits, with or without a leading `#`.
 */

/** WCAG 2.1 AA for text of normal size. */
export const TEXT_CONTRAST = 4.5;

type Rgb = [number, number, number];

/** What this module reads of a token rule of Monaco. */
export type TokenRule = {
  token: string;
  foreground?: string;
  background?: string;
  fontStyle?: string;
};

/** What this module reads of a theme of Monaco. */
export type BaseTheme = {
  base: "vs" | "vs-dark" | "hc-black" | "hc-light";
  rules: TokenRule[];
};

export type DerivedTheme = {
  base: BaseTheme["base"];
  inherit: true;
  rules: TokenRule[];
  colors: Record<string, string>;
};

/**
 * The colors of bracket pairs (`editorBracketHighlight.foreground1` to `3`)
 * as Monaco registers them; they are text, but no token rules.
 */
const BRACKET_COLORS: Record<"light" | "dark", string[]> = {
  light: ["#0431fa", "#319331", "#7b3814"],
  dark: ["#ffd700", "#da70d6", "#179fff"],
};

function parse(color: string): Rgb {
  const hex = color.replace(/^#/, "");
  return [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16)) as Rgb;
}

function format(rgb: Rgb): string {
  return rgb.map((v) => Math.round(v).toString(16).padStart(2, "0")).join("");
}

function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map((value) => {
    const v = value / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(a: Rgb, b: Rgb): number {
  const [lighter, darker] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (lighter + 0.05) / (darker + 0.05);
}

/** The contrast ratio of two colors as WCAG defines it (1 to 21). */
export function contrastRatio(a: string, b: string): number {
  return ratio(parse(a), parse(b));
}

/**
 * `foreground` if it has a contrast of at least `minimum` to `background`,
 * else the nearest color towards white (on a dark background) or black (on a
 * light one) that has. Keeps the notation of `foreground` (`#` or none).
 */
export function readable(
  foreground: string,
  background: string,
  minimum = TEXT_CONTRAST,
): string {
  const from = parse(foreground);
  const bg = parse(background);
  if (ratio(from, bg) >= minimum) return foreground;

  const target: Rgb = luminance(bg) < 0.5 ? [255, 255, 255] : [0, 0, 0];
  let color = from;
  for (let step = 1; step <= 100 && ratio(color, bg) < minimum; step++) {
    color = from.map((v, i) =>
      Math.round(v + ((target[i] - v) * step) / 100),
    ) as Rgb;
  }
  return (foreground.startsWith("#") ? "#" : "") + format(color);
}

/**
 * A theme on top of `base` in which every text color is readable on the
 * background of the editor. Only the rules that had to change are listed;
 * the rest is inherited.
 */
export function accessibleTheme(base: BaseTheme): DerivedTheme {
  const background =
    base.rules.find((rule) => rule.token === "")?.background ??
    (base.base === "vs" ? "ffffff" : "1e1e1e");

  const rules = base.rules.flatMap((rule) => {
    if (!rule.foreground) return [];
    const foreground = readable(rule.foreground, background);
    return foreground === rule.foreground ? [] : [{ ...rule, foreground }];
  });

  const colors: Record<string, string> = {};
  BRACKET_COLORS[base.base === "vs" ? "light" : "dark"].forEach((color, i) => {
    colors[`editorBracketHighlight.foreground${i + 1}`] = readable(
      color,
      `#${background}`,
    );
  });

  return { base: base.base, inherit: true, rules, colors };
}
