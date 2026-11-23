/* @vitest-environment jsdom */

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/svelte";
import { tick } from "svelte";
import ScanProgress from "$lib/components/ScanProgress.svelte";
import { scanProgress } from "$lib/stores/scanProgress";
import { setLocale } from "$lib/i18n";

vi.mock("$lib/services/backend", () => ({ post: vi.fn() }));
import { post } from "$lib/services/backend";

describe("ScanProgress", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    scanProgress.reset();
    setLocale("en");
  });
  afterEach(() => setLocale("en"));

  it("shows an indeterminate bar – no made-up percentage", () => {
    render(ScanProgress);

    const bar = screen.getByRole("progressbar", { name: "Scan progress" });
    expect(bar).toHaveAttribute("aria-busy", "true");
    expect(bar).not.toHaveAttribute("aria-valuenow");
    expect(bar.querySelector(".scan-progress-fill")).not.toHaveAttribute(
      "style",
    );
  });

  it("starts without numbers of a previous scan", () => {
    render(ScanProgress);

    expect(screen.getByText("Files scanned: 0")).toBeInTheDocument();
    expect(screen.getByText("Starting scan...")).toBeInTheDocument();
    expect(document.getElementById("scan-skipped-count")).toBeNull();
  });

  it("renders what the store reports: files, skipped files, current path", async () => {
    render(ScanProgress);

    scanProgress.report({
      files_scanned: 1234,
      large_files_skipped: 2,
      current_scanning_path: "/repo/src/main.rs",
    });
    await tick();

    expect(screen.getByText("Files scanned: 1234")).toBeInTheDocument();
    expect(screen.getByText("Large files skipped: 2")).toBeInTheDocument();
    expect(screen.getByText("/repo/src/main.rs")).toBeInTheDocument();
  });

  it("is translated", async () => {
    setLocale("de");
    render(ScanProgress);

    scanProgress.report({
      files_scanned: 7,
      large_files_skipped: 1,
      current_scanning_path: "",
    });
    await tick();

    expect(screen.getByText("Gescannte Dateien: 7")).toBeInTheDocument();
    expect(
      screen.getByText("Große Dateien übersprungen: 1"),
    ).toBeInTheDocument();
    expect(screen.getByText("Scan wird gestartet...")).toBeInTheDocument();
    expect(
      screen.getByText("Verzeichnis wird gescannt..."),
    ).toBeInTheDocument();
  });

  describe("compact (above the tree, for the rest of the scan)", () => {
    it("shows the progress and cancels the scan", async () => {
      render(ScanProgress, { compact: true });
      scanProgress.report({
        files_scanned: 1234,
        large_files_skipped: 0,
        current_scanning_path: "/repo/src/main.rs",
      });
      await tick();

      const bar = screen.getByRole("group", { name: "Scan progress" });
      expect(bar).toHaveTextContent("Scanning directory...");
      expect(bar).toHaveTextContent("Files scanned: 1234");
      expect(bar).toHaveTextContent("/repo/src/main.rs");
      // The numbers change many times a second: they are not a live region.
      expect(screen.getByRole("status")).not.toHaveTextContent("1234");

      await fireEvent.click(
        screen.getByRole("button", { name: "Cancel the scan" }),
      );
      expect(post).toHaveBeenCalledWith("cancelScan");
    });

    it("is translated", () => {
      setLocale("de");
      render(ScanProgress, { compact: true });

      expect(
        screen.getByRole("button", { name: "Scan abbrechen" }),
      ).toHaveTextContent("Abbrechen");
      expect(screen.getByRole("status")).toHaveTextContent(
        "Verzeichnis wird gescannt...",
      );
    });
  });

  it("cancels the scan", async () => {
    render(ScanProgress);

    await fireEvent.click(screen.getByRole("button", { name: /cancel/i }));

    expect(post).toHaveBeenCalledWith("cancelScan");
  });
});
