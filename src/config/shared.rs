//! The part of the configuration that is meant to be passed on.
//!
//! [`AppConfig`] holds two kinds of things: the settings a user edits and may want to
//! share with a team (ignore patterns, search and output options, the size limit), and
//! what describes one installation on one machine (the directories used last). An
//! exported file is made to be shared, so it holds the first kind only: no path of the
//! person who exported it leaves their machine.
//!
//! - [`SharedConfig`] is that view, and the format of an exported file.
//! - [`ImportedConfig`] is what an import reads. It accepts the exported format as
//!   well as a complete configuration file (what earlier versions exported, or a copy
//!   of the application's own `config.json`).

use super::{deserialize_max_file_size_mb, AppConfig};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// The settings of an [`AppConfig`] that are shared: everything a user edits, nothing
/// that names a place on their machine.
///
/// A setting that a file does not name has its default value. The patterns are kept
/// sorted, so two exports of the same settings are the same file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SharedConfig {
    pub ignore_patterns: BTreeSet<String>,
    pub tree_ignore_patterns: BTreeSet<String>,
    pub output_filename: String,
    pub case_sensitive_search: bool,
    pub include_tree_by_default: bool,
    pub use_relative_paths: bool,
    pub remove_empty_directories: bool,
    /// See [`AppConfig::max_file_size_mb`]; raised to the minimum when read, too.
    #[serde(deserialize_with = "deserialize_max_file_size_mb")]
    pub max_file_size_mb: u64,
}

impl Default for SharedConfig {
    /// The shared settings of a default [`AppConfig`].
    fn default() -> Self {
        Self::from(&AppConfig::default())
    }
}

impl From<&AppConfig> for SharedConfig {
    fn from(config: &AppConfig) -> Self {
        Self {
            ignore_patterns: config.ignore_patterns.iter().cloned().collect(),
            tree_ignore_patterns: config.tree_ignore_patterns.iter().cloned().collect(),
            output_filename: config.output_filename.clone(),
            case_sensitive_search: config.case_sensitive_search,
            include_tree_by_default: config.include_tree_by_default,
            use_relative_paths: config.use_relative_paths,
            remove_empty_directories: config.remove_empty_directories,
            max_file_size_mb: config.max_file_size_mb,
        }
    }
}

impl SharedConfig {
    /// Replaces the shared settings of `config` by these. Everything else in `config`
    /// (the directories) stays as it is.
    pub fn apply_to(self, config: &mut AppConfig) {
        // Destructured without `..`: a setting added to the view cannot be forgotten.
        let Self {
            ignore_patterns,
            tree_ignore_patterns,
            output_filename,
            case_sensitive_search,
            include_tree_by_default,
            use_relative_paths,
            remove_empty_directories,
            max_file_size_mb,
        } = self;
        config.ignore_patterns = ignore_patterns.into_iter().collect();
        config.tree_ignore_patterns = tree_ignore_patterns.into_iter().collect();
        config.output_filename = output_filename;
        config.case_sensitive_search = case_sensitive_search;
        config.include_tree_by_default = include_tree_by_default;
        config.use_relative_paths = use_relative_paths;
        config.remove_empty_directories = remove_empty_directories;
        config.max_file_size_mb = max_file_size_mb;
    }
}

/// What an imported file holds: the shared settings, and the directories a complete
/// configuration file names besides them.
///
/// An exported file names no directories. A complete configuration file does, and an
/// import keeps honouring them as it always did; a window geometry such a file may
/// hold (versions up to 0.3.1 kept it there) is ignored.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ImportedConfig {
    #[serde(flatten)]
    pub settings: SharedConfig,
    /// The directory the output is saved to, if the file names one.
    #[serde(default)]
    pub output_directory: Option<PathBuf>,
    /// The directory that was open when the file was written, if the file names one.
    #[serde(default)]
    pub last_directory: Option<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::collections::HashSet;

    /// A configuration in which every field differs from the default.
    fn personal_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::from(["*.log".to_string(), "dist".to_string()]),
            tree_ignore_patterns: HashSet::from(["docs".to_string()]),
            last_directory: Some(PathBuf::from("/home/alice/secret-project")),
            output_directory: Some(PathBuf::from("/home/alice/Desktop")),
            output_filename: "context.txt".to_string(),
            case_sensitive_search: true,
            include_tree_by_default: false,
            use_relative_paths: false,
            remove_empty_directories: true,
            max_file_size_mb: 7,
        }
    }

    fn keys_of(value: &Value) -> BTreeSet<String> {
        value.as_object().unwrap().keys().cloned().collect()
    }

    /// The fields of an `AppConfig` that describe the installation and are not shared.
    const INSTALLATION_FIELDS: [&str; 2] = ["last_directory", "output_directory"];

    /// Every field of the configuration is either shared or named here as belonging
    /// to the installation. A field added to `AppConfig` fails this test until it is
    /// decided which of the two it is.
    #[test]
    fn every_config_field_is_either_shared_or_installation_specific() {
        let all = keys_of(&serde_json::to_value(personal_config()).unwrap());
        let shared = keys_of(&serde_json::to_value(SharedConfig::default()).unwrap());
        let installation: BTreeSet<String> =
            INSTALLATION_FIELDS.iter().map(|s| s.to_string()).collect();

        assert!(shared.is_disjoint(&installation));
        assert_eq!(
            all,
            shared
                .union(&installation)
                .cloned()
                .collect::<BTreeSet<_>>()
        );
    }

    #[test]
    fn shared_view_holds_no_path_and_no_geometry() {
        let exported = serde_json::to_string(&SharedConfig::from(&personal_config())).unwrap();

        for personal in ["alice", "secret-project", "Desktop", "1234", "567", "-89"] {
            assert!(!exported.contains(personal), "{personal} in {exported}");
        }
        for field in INSTALLATION_FIELDS {
            assert!(!exported.contains(field), "{field} in {exported}");
        }
    }

    #[test]
    fn shared_view_carries_every_setting_and_sorts_the_patterns() {
        let exported = serde_json::to_value(SharedConfig::from(&personal_config())).unwrap();

        assert_eq!(
            exported,
            json!({
                "ignore_patterns": ["*.log", "dist"],
                "tree_ignore_patterns": ["docs"],
                "output_filename": "context.txt",
                "case_sensitive_search": true,
                "include_tree_by_default": false,
                "use_relative_paths": false,
                "remove_empty_directories": true,
                "max_file_size_mb": 7,
            })
        );
    }

    /// Applying the view of a configuration to another one transfers exactly the
    /// shared settings.
    #[test]
    fn applying_the_view_replaces_the_shared_settings_only() {
        let source = personal_config();
        let mut target = AppConfig::default();
        let untouched = target.clone();

        SharedConfig::from(&source).apply_to(&mut target);

        assert_eq!(
            target,
            AppConfig {
                last_directory: untouched.last_directory,
                output_directory: untouched.output_directory,
                ..source
            }
        );
    }

    #[test]
    fn imported_export_names_no_directories() {
        let exported = serde_json::to_value(SharedConfig::from(&personal_config())).unwrap();

        let imported: ImportedConfig = serde_json::from_value(exported).unwrap();

        assert_eq!(imported.settings, SharedConfig::from(&personal_config()));
        assert_eq!(imported.output_directory, None);
        assert_eq!(imported.last_directory, None);
    }

    /// What earlier versions exported: the complete configuration.
    #[test]
    fn imported_complete_config_names_its_directories() {
        let complete = serde_json::to_value(personal_config()).unwrap();

        let imported: ImportedConfig = serde_json::from_value(complete).unwrap();

        assert_eq!(imported.settings, SharedConfig::from(&personal_config()));
        assert_eq!(
            imported.output_directory,
            Some(PathBuf::from("/home/alice/Desktop"))
        );
        assert_eq!(
            imported.last_directory,
            Some(PathBuf::from("/home/alice/secret-project"))
        );
    }

    #[test]
    fn settings_a_file_does_not_name_have_their_default() {
        let imported: ImportedConfig =
            serde_json::from_value(json!({ "case_sensitive_search": true })).unwrap();

        assert_eq!(
            imported.settings,
            SharedConfig {
                case_sensitive_search: true,
                ..SharedConfig::default()
            }
        );
        assert_eq!(
            SharedConfig::default(),
            SharedConfig::from(&AppConfig::default())
        );
    }

    #[test]
    fn imported_file_size_limit_is_raised_to_the_minimum() {
        let imported: ImportedConfig =
            serde_json::from_value(json!({ "max_file_size_mb": 0 })).unwrap();

        assert_eq!(
            imported.settings.max_file_size_mb,
            crate::config::MIN_MAX_FILE_SIZE_MB
        );
    }
}
