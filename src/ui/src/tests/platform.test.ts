import { describe, it, expect } from "vitest";
import { isMac } from "$lib/platform";

describe("isMac", () => {
  it("reads the platform of the user agent data first (WebView2)", () => {
    expect(isMac({ userAgentData: { platform: "macOS" } })).toBe(true);
    expect(
      isMac({ userAgentData: { platform: "Windows" }, platform: "MacIntel" }),
    ).toBe(false);
  });

  it("falls back to navigator.platform (WKWebView, WebKitGTK)", () => {
    expect(isMac({ platform: "MacIntel" })).toBe(true);
    expect(isMac({ platform: "Linux x86_64" })).toBe(false);
    expect(isMac({ platform: "Win32" })).toBe(false);
  });

  it("falls back to the user agent when no platform is given", () => {
    expect(
      isMac({
        platform: "",
        userAgent:
          "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      }),
    ).toBe(true);
    expect(
      isMac({ userAgent: "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1" }),
    ).toBe(false);
  });

  it("is false where nothing is known", () => {
    expect(isMac({})).toBe(false);
  });
});
