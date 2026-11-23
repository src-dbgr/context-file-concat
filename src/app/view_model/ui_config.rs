//! The part of the configuration the UI gets to see.

use crate::config::AppConfig;
use serde::Serialize;
use std::path::PathBuf;

/// The view of [`AppConfig`] that is sent to the UI.
///
/// It holds exactly the settings the UI displays; each of them is edited with an
/// intent command (`addIgnorePattern`, `removeIgnorePattern`, `clearIgnorePatterns`,
/// `setConfigOption`, `pickOutputDirectory`). What only the host works with (the
/// last directory, the file size limit, the tree ignore patterns) is
/// deliberately not part of it: the UI has no use for it, and what is never sent can
/// never come back outdated.
///
/// Being a type of its own also keeps the persisted format and the wire format
/// independent: a field added to [`AppConfig`] is not sent unless it is added here.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct UiConfig {
    /// Sorted, so that equal configurations serialize identically.
    pub ignore_patterns: Vec<String>,
    pub case_sensitive_search: bool,
    pub remove_empty_directories: bool,
    pub include_tree_by_default: bool,
    pub use_relative_paths: bool,
    #[serde(serialize_with = "super::lossy_path::serialize_option")]
    pub output_directory: Option<PathBuf>,
    pub output_filename: String,
}

impl From<&AppConfig> for UiConfig {
    fn from(config: &AppConfig) -> Self {
        let mut ignore_patterns: Vec<String> = config.ignore_patterns.iter().cloned().collect();
        ignore_patterns.sort_unstable();

        Self {
            ignore_patterns,
            case_sensitive_search: config.case_sensitive_search,
            remove_empty_directories: config.remove_empty_directories,
            include_tree_by_default: config.include_tree_by_default,
            use_relative_paths: config.use_relative_paths,
            output_directory: config.output_directory.clone(),
            output_filename: config.output_filename.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::from(["b".to_string(), "c".to_string(), "a".to_string()]),
            tree_ignore_patterns: HashSet::from(["assets/".to_string()]),
            last_directory: Some(PathBuf::from("/secret/last")),
            output_directory: Some(PathBuf::from("/out")),
            output_filename: "out.txt".to_string(),
            case_sensitive_search: true,
            include_tree_by_default: false,
            use_relative_paths: true,
            remove_empty_directories: true,
            max_file_size_mb: 42,
        }
    }

    #[test]
    fn carries_the_settings_the_ui_displays() {
        let ui_config = UiConfig::from(&config());

        assert_eq!(ui_config.ignore_patterns, ["a", "b", "c"]);
        assert!(ui_config.case_sensitive_search);
        assert!(ui_config.remove_empty_directories);
        assert!(!ui_config.include_tree_by_default);
        assert!(ui_config.use_relative_paths);
        assert_eq!(ui_config.output_directory, Some(PathBuf::from("/out")));
        assert_eq!(ui_config.output_filename, "out.txt");
    }

    /// The wire format is an allow-list. A new field has to be added here on purpose,
    /// together with the UI schema and the contract fixture.
    #[test]
    fn serializes_exactly_the_allow_listed_fields() {
        let json = serde_json::to_value(UiConfig::from(&config())).unwrap();
        let mut fields: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        fields.sort_unstable();

        assert_eq!(
            fields,
            [
                "case_sensitive_search",
                "ignore_patterns",
                "include_tree_by_default",
                "output_directory",
                "output_filename",
                "remove_empty_directories",
                "use_relative_paths",
            ]
        );
    }
}
