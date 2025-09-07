import { test, expect, Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import type { Result, NodeResult } from "axe-core";

const STRICT = process.env.E2E_A11Y_STRICT === "1";

type ContrastIssue = {
  id: string;
  impact: Result["impact"];
  help: string;
  selector: string;
};

async function analyzeContrast(page: Page) {
  const cssVars = await page.evaluate(() => {
    const root = document.documentElement;
    const cs = getComputedStyle(root);
    return {
      theme: root.getAttribute("data-theme") ?? "unset",
      colorMuted: cs.getPropertyValue("--color-muted").trim(),
      paletteDarkMuted: cs.getPropertyValue("--palette-dark-muted").trim(),
      surface2: cs.getPropertyValue("--surface-2").trim(),
      colorText: cs.getPropertyValue("--color-text").trim(),
    };
  });
  console.log("A11Y VARS:", cssVars);

  const results = await new AxeBuilder({ page })
    .withTags(["wcag2a", "wcag2aa"])
    .disableRules([])
    .include("body")
    .analyze();

  const contrastViolations = results.violations.filter(
    (v) =>
      v.id === "color-contrast" &&
      (STRICT
        ? v.impact === "serious" || v.impact === "critical"
        : v.impact === "critical")
  );

  const offenders: ContrastIssue[] = [];
  for (const v of contrastViolations) {
    for (const n of v.nodes as NodeResult[]) {
      const firstTarget = (Array.isArray(n.target) ? n.target[0] : n.target) as
        | string
        | undefined;
      if (firstTarget) {
        offenders.push({
          id: v.id,
          impact: v.impact!,
          help: v.help,
          selector: firstTarget,
        });
      }
    }
  }

  const uniqueSelectors = [...new Set(offenders.map((o) => o.selector))].slice(
    0,
    10
  );

  const styleProbe = await page.evaluate((sels) => {
    return sels.map((sel) => {
      const el = document.querySelector(sel) as HTMLElement | null;
      if (!el) return { sel, found: false as const };
      const cs = getComputedStyle(el);
      return {
        sel,
        found: true as const,
        textSnippet: (el.textContent || "").trim().slice(0, 80),
        color: cs.color,
        backgroundColor: cs.backgroundColor,
        fontSize: cs.fontSize,
        fontWeight: cs.fontWeight,
      };
    });
  }, uniqueSelectors);

  console.log("A11Y CONTRAST OFFENDERS (top):", styleProbe);

  return { contrastViolations, cssVars };
}

test(
  "Color contrast report" +
    (STRICT ? " (strict: serious+critical)" : " (no *critical* issues)"),
  async ({ page }) => {
    await page.goto("/");

    const { contrastViolations } = await analyzeContrast(page);

    console.log(
      JSON.stringify(
        contrastViolations.map((v) => ({
          id: v.id,
          impact: v.impact,
          nodes: v.nodes.length,
        })),
        null,
        2
      )
    );

    // Fail-Policy
    expect(
      contrastViolations.length,
      STRICT
        ? "Expected no serious/critical color-contrast violations"
        : "Expected no critical color-contrast violations"
    ).toBe(0);
  }
);
