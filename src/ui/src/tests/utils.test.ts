import { describe, it, expect } from "vitest";
import {
  formatFileSize,
  generateStatsString,
  splitPathForDisplay,
} from "$lib/utils";

describe("utils.formatFileSize", () => {
  it("formats 0 bytes", () => {
    expect(formatFileSize(0)).toBe("0 B");
  });

  it("formats < 1KB without decimals", () => {
    expect(formatFileSize(1023)).toBe("1023 B");
  });

  it("formats 1KB as KB without trailing .0", () => {
    expect(formatFileSize(1024)).toBe("1 KB");
  });

  it("formats MB and GB ranges", () => {
    expect(formatFileSize(1024 * 1024)).toBe("1 MB");
    expect(formatFileSize(3.5 * 1024 * 1024)).toBe("3.5 MB");
  });
});

describe("utils.generateStatsString", () => {
  it("generates stats for plain content", () => {
    const s = generateStatsString("Hello world", "read-only");
    expect(s).toMatch(/1 lines/);
    expect(s).toMatch(/2 words/);
    expect(s).toMatch(/11 chars/);
    expect(s).toMatch(/11 B/);
    expect(s).toMatch(/read-only/);
  });

  it("includes token count when provided", () => {
    const s = generateStatsString("a b c", "editable", 1500);
    expect(s).toMatch(/1 lines/);
    expect(s).toMatch(/3 words/);
    expect(s).toMatch(/1\.5K tokens/);
    expect(s).toMatch(/editable/);
  });
});

describe("utils.splitPathForDisplay", () => {
  it("returns filename and pathPart relative to current directory (POSIX)", () => {
    const { pathPart, filename } = splitPathForDisplay(
      "/home/user/proj/src/index.ts",
      "/home/user/proj"
    );
    expect(pathPart).toBe("src/");
    expect(filename).toBe("index.ts");
  });

  it("handles Windows-style separators", () => {
    const { pathPart, filename } = splitPathForDisplay(
      "C:\\work\\repo\\app\\main.rs",
      "C:\\work\\repo"
    );
    expect(pathPart).toBe("app/");
    expect(filename).toBe("main.rs");
  });

  it("falls back to absolute-like path when outside current dir", () => {
    const { pathPart, filename } = splitPathForDisplay(
      "/other/file.txt",
      "/home"
    );
    expect(pathPart).toBe("/other/");
    expect(filename).toBe("file.txt");
  });

  it("handles root-level files", () => {
    const { pathPart, filename } = splitPathForDisplay("README.md", "/proj");
    expect(pathPart).toBe("");
    expect(filename).toBe("README.md");
  });
});
