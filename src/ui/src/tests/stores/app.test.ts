/* @vitest-environment jsdom */
import { describe, it, expect } from "vitest";
import { appState, getState } from "$lib/stores/app";
import type { AppState } from "$lib/types";

function baseState(): AppState {
  return {
    is_scanning: false,
    is_generating: false,
    is_fully_scanned: true,
    patterns_need_rescan: false,
    tree: [],
    current_path: "/repo",
    current_config_filename: null,
    status_message: "Status: Ready.",
    selected_files_count: 0,
    search_query: "",
    extension_filter: "",
    content_search_query: "",
    active_ignore_patterns: [],
    config: {
      ignore_patterns: [],
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
      output_directory: "",
      output_filename: "output.txt",
    },
  };
}

describe("app store", () => {
  it("getState reflects last set value", () => {
    const s = baseState();
    s.status_message = "Status: Hello.";
    appState.set(s);
    expect(getState().status_message).toBe("Status: Hello.");
  });

  it("update mutates state predictably", () => {
    appState.set(baseState());
    appState.update((s) => {
      s.selected_files_count = 2;
      return s;
    });
    expect(getState().selected_files_count).toBe(2);
  });
});
