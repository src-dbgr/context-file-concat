// Central store for the application state.
let appState = {
  is_scanning: false,
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

let editor = null;
let currentDecorations = [];
let currentPreviewedPath = null;
let currentPatternFilter = "";

// We export an object with getters and setters to control access to the state.
export const state = {
  get: () => appState,
  set: (newState) => {
    appState = newState;
  },
  getEditor: () => editor,
  setEditor: (e) => {
    editor = e;
  },
  getDecorations: () => currentDecorations,
  setDecorations: (decs) => {
    currentDecorations = decs;
  },
  getPreviewedPath: () => currentPreviewedPath,
  setPreviewedPath: (path) => {
    currentPreviewedPath = path;
  },
  getPatternFilter: () => currentPatternFilter,
  setPatternFilter: (filter) => {
    currentPatternFilter = filter;
  },
};
