/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import {
  translate,
  register,
  setLocale,
  availableLocales,
  getLocale,
} from "$lib/i18n";

describe("i18n registry and fallbacks", () => {
  it("falls back to EN when locale not registered", () => {
    register("en", { "_t.fallback": "Fallback-EN" });
    setLocale("fr");
    expect(translate("_t.fallback")).toBe("Fallback-EN");
  });

  it("returns key when no message found in any locale", () => {
    setLocale("en");
    const unknown = "__missing.key";
    expect(translate(unknown)).toBe(unknown);
  });

  it("supports parameter interpolation and function messages", () => {
    register("en", {
      "greet.plain": "Hello {name}",
      "greet.fn": (p) => `Hi ${(p as { name: string }).name}!`,
    });
    setLocale("en");
    expect(translate("greet.plain", { name: "Ada" })).toBe("Hello Ada");
    expect(translate("greet.fn", { name: "Ada" })).toBe("Hi Ada!");
  });

  it("locale switching works and merges", () => {
    register("de", { "_t.msg": "Hallo" });
    setLocale("de");
    expect(translate("_t.msg")).toBe("Hallo");
    setLocale("en");
    expect(["en", "de"].every((c) => availableLocales().includes(c))).toBe(
      true
    );
    expect(getLocale()).toBe("en");
  });
});
