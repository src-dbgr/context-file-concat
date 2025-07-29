// Central store for the application state, encapsulated in a revealing module pattern.
export const state = (() => {
  // --- Private State ---
  let _appState = {
    is_scanning: false,
    is_generating: false,
    config: {
      ignore_patterns: [],
      case_sensitive_search: false,
      include_tree_by_default: false,
      use_relative_paths: false,
      remove_empty_directories: false,
    },
    tree: [],
    current_path: "",
    status_message: "Ready.",
    selected_files_count: 0,
  };

  let _editor = null;
  let _currentDecorations = [];
  let _currentPreviewedPath = null;
  let _currentPatternFilter = "";

  // --- Public API ---
  // We export an object with getters and setters to control access to the state.
  return {
    get: () => _appState,
    set: (newState) => {
      _appState = newState;
    },
    getEditor: () => _editor,
    setEditor: (e) => {
      _editor = e;
    },
    getDecorations: () => _currentDecorations,
    setDecorations: (decs) => {
      _currentDecorations = decs;
    },
    getPreviewedPath: () => _currentPreviewedPath,
    setPreviewedPath: (path) => {
      _currentPreviewedPath = path;
    },
    getPatternFilter: () => _currentPatternFilter,
    setPatternFilter: (filter) => {
      _currentPatternFilter = filter;
    },
  };
})();
