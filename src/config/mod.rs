pub mod settings;
pub mod shared;
pub mod store;

pub use shared::{ImportedConfig, SharedConfig};
pub use store::{ConfigSnapshot, ConfigStore};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// The persisted settings of the application.
///
/// Fields that are not part of this struct are ignored when a configuration is read
/// (deliberately no `deny_unknown_fields`), so files written by an older version keep
/// loading after a setting has been retired, e.g. the former `scan_chunk_size` and
/// `auto_load_last_directory`.
///
/// A field that a file does not name has its value of [`AppConfig::default`]
/// (`#[serde(default)]`): a file written before a setting existed keeps loading, with
/// all its other settings, and no list of fields has to be kept in step for that.
///
/// The geometry of the window is not a setting and not kept here: the composition root
/// owns it and keeps it in a file of its own (see `app::window_geometry`). Versions up
/// to 0.3.1 stored it in this file (`window_size`, `window_position`,
/// `geometry_units`); like any field this struct does not have, those are ignored when
/// such a file is read and gone once it is written again.
///
/// Which of the fields are settings to share and which describe this installation is
/// defined by [`SharedConfig`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub ignore_patterns: HashSet<String>,
    pub tree_ignore_patterns: HashSet<String>,
    pub last_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub output_filename: String,
    pub case_sensitive_search: bool,
    pub include_tree_by_default: bool,
    pub use_relative_paths: bool,
    pub remove_empty_directories: bool,
    /// Size limit for files in MiB: larger files are not listed by the scan, not read
    /// by the content search and not inlined into the generated output.
    ///
    /// A configuration that is read never holds less than
    /// [`MIN_MAX_FILE_SIZE_MB`]: smaller values are raised when the file is loaded.
    #[serde(deserialize_with = "deserialize_max_file_size_mb")]
    pub max_file_size_mb: u64,
}

/// The smallest file size limit a stored configuration can ask for, in MiB.
///
/// The limit is not editable in the UI, so a value of 0 in the file (which would hide
/// every file that is not empty, without a word) is a mistake rather than a wish.
pub const MIN_MAX_FILE_SIZE_MB: u64 = 1;

/// Reads [`AppConfig::max_file_size_mb`], raising it to [`MIN_MAX_FILE_SIZE_MB`] if it
/// is smaller. Being part of the deserialization, this holds for every way a
/// configuration is read: the application's own file, an import, a migrated file.
fn deserialize_max_file_size_mb<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let stored = u64::deserialize(deserializer)?;
    if stored < MIN_MAX_FILE_SIZE_MB {
        tracing::warn!(
            "max_file_size_mb is {stored}; using the minimum of {MIN_MAX_FILE_SIZE_MB} instead"
        );
    }
    Ok(stored.max(MIN_MAX_FILE_SIZE_MB))
}

impl AppConfig {
    /// Takes over what an imported file holds: the settings a user edits and shares
    /// (patterns, search and output options, the file size limit, see
    /// [`SharedConfig`]), and the output directory if the file names one.
    ///
    /// What describes this installation rather than a preference stays as it is: the
    /// directory that was opened last. (Whether the directory named by the imported
    /// file is opened is up to the caller; opening it is what makes it the last
    /// directory.) An output directory the file does not name stays as well: a file
    /// exported for sharing names none. A window geometry the file may hold (a complete
    /// configuration file of an older version) is not read at all.
    pub fn adopt_user_settings(&mut self, imported: ImportedConfig) {
        imported.settings.apply_to(self);
        if let Some(output_directory) = imported.output_directory {
            self.output_directory = Some(output_directory);
        }
    }

    /// The file size limit ([`AppConfig::max_file_size_mb`]) in bytes, the unit the
    /// core works with.
    pub fn max_file_size_bytes(&self) -> u64 {
        const BYTES_PER_MIB: u64 = 1024 * 1024;
        self.max_file_size_mb.saturating_mul(BYTES_PER_MIB)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut ignore_patterns = HashSet::new();
        let common_patterns = [
            "node_modules",
            "venv",
            "target",
            ".idea",
            ".git",
            "*.env",
            "*.log",
            "*.tmp",
            ".DS_Store",
            "Thumbs.db",
            "__pycache__",
            "*.pyc",
            "*.class",
            "*.o",
            "*.obj",
            "package-lock.json",
            "*.lock",
            ".gitignore",
        ];
        for pattern in common_patterns {
            ignore_patterns.insert(pattern.to_string());
        }

        let image_extensions = [
            "*.png", "*.jpg", "*.jpeg", "*.gif", "*.bmp", "*.ico", "*.webp", "*.tiff", "*.tif",
            "*.heic", "*.heif", "*.avif", "*.raw", "*.icns",
        ];
        for ext in image_extensions {
            ignore_patterns.insert(ext.to_string());
        }

        let binary_extensions = [
            "*.exe", "*.dll", "*.so", "*.dylib", "*.app", "*.deb", "*.rpm", "*.msi", "*.jar",
            "*.war", "*.a", "*.lib", "*.rlib", "*.pdf", "*.doc", "*.docx", "*.xls", "*.xlsx",
            "*.ppt", "*.pptx", "*.zip", "*.tar", "*.gz", "*.7z", "*.rar", "*.bin", "*.dat", "*.db",
            "*.sqlite", "*.mp4", "*.mp3",
        ];
        for ext in binary_extensions {
            ignore_patterns.insert(ext.to_string());
        }

        Self {
            ignore_patterns,
            tree_ignore_patterns: HashSet::new(),
            last_directory: None,
            output_directory: desktop_dir(),
            // A static name keeps `Default` deterministic. It stays until the user changes
            // it: a generation only renews a name of the form `cfc_output_<timestamp>.txt`
            // (see `commands::generate_preview`).
            output_filename: "cfc_output.txt".to_string(),
            case_sensitive_search: false,
            include_tree_by_default: true,
            use_relative_paths: true,
            remove_empty_directories: false,
            max_file_size_mb: 20,
        }
    }
}

/// The desktop directory of the current user, where the output goes by default.
fn desktop_dir() -> Option<PathBuf> {
    directories::UserDirs::new()?
        .desktop_dir()
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh configuration must behave exactly like the core does on its own.
    #[test]
    fn default_file_size_limit_is_the_limit_of_the_core() {
        assert_eq!(
            AppConfig::default().max_file_size_bytes(),
            crate::core::MAX_FILE_SIZE
        );
    }

    #[test]
    fn file_size_limit_is_converted_from_mib_without_overflow() {
        let limit_of = |max_file_size_mb| {
            AppConfig {
                max_file_size_mb,
                ..Default::default()
            }
            .max_file_size_bytes()
        };

        assert_eq!(limit_of(3), 3 * 1024 * 1024);
        assert_eq!(limit_of(0), 0);
        assert_eq!(limit_of(u64::MAX), u64::MAX);
    }

    #[test]
    fn file_size_limit_below_the_minimum_is_raised_when_a_config_is_read() {
        let read_with_limit = |limit: u64| {
            let mut json = serde_json::to_value(AppConfig::default()).unwrap();
            json["max_file_size_mb"] = limit.into();
            serde_json::from_value::<AppConfig>(json)
                .unwrap()
                .max_file_size_mb
        };

        assert_eq!(read_with_limit(0), MIN_MAX_FILE_SIZE_MB);
        assert_eq!(read_with_limit(1), 1);
        assert_eq!(read_with_limit(20), 20);
        assert_eq!(read_with_limit(4096), 4096);
    }

    /// A file of a version that kept the window geometry in the configuration (up to
    /// 0.3.1) is read like any other: the geometry is no field of the configuration.
    #[test]
    fn a_config_that_holds_a_window_geometry_is_read_without_it() {
        let mut json = serde_json::to_value(foreign_config()).unwrap();
        let fields = json.as_object_mut().unwrap();
        fields.insert("window_size".into(), serde_json::json!([4000.0, 3000.0]));
        fields.insert(
            "window_position".into(),
            serde_json::json!([-900.0, -900.0]),
        );
        fields.insert("geometry_units".into(), serde_json::json!("physical"));

        let read: AppConfig = serde_json::from_value(json).unwrap();

        assert_eq!(read, foreign_config());
        let written = serde_json::to_string(&read).unwrap();
        for field in ["window_size", "window_position", "geometry_units"] {
            assert!(!written.contains(field), "{field} in {written}");
        }
    }

    /// A configuration that differs from the default in everything an import can
    /// touch, with the last directory of another installation.
    fn foreign_config() -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::from(["*.imported".to_string()]),
            tree_ignore_patterns: HashSet::from(["docs".to_string()]),
            output_directory: Some(PathBuf::from("/imported/out")),
            output_filename: "imported.txt".to_string(),
            case_sensitive_search: true,
            include_tree_by_default: false,
            use_relative_paths: false,
            remove_empty_directories: true,
            max_file_size_mb: 3,
            last_directory: Some(PathBuf::from("/somebody/elses/project")),
        }
    }

    fn current_config() -> AppConfig {
        AppConfig {
            last_directory: Some(PathBuf::from("/current/project")),
            output_directory: Some(PathBuf::from("/current/out")),
            ..AppConfig::default()
        }
    }

    /// A complete configuration file, as earlier versions exported it.
    #[test]
    fn adopting_user_settings_keeps_the_last_directory() {
        let mut current = current_config();
        let imported = foreign_config();

        current.adopt_user_settings(
            serde_json::from_value(serde_json::to_value(&imported).unwrap()).unwrap(),
        );

        assert_eq!(
            current,
            AppConfig {
                last_directory: Some(PathBuf::from("/current/project")),
                ..imported
            }
        );
    }

    /// A file exported for sharing names no directories: the output directory of this
    /// installation stays, too.
    #[test]
    fn adopting_shared_settings_keeps_the_output_directory_as_well() {
        let mut current = current_config();
        let exported = serde_json::to_value(SharedConfig::from(&foreign_config())).unwrap();

        current.adopt_user_settings(serde_json::from_value(exported).unwrap());

        assert_eq!(
            current,
            AppConfig {
                last_directory: Some(PathBuf::from("/current/project")),
                output_directory: Some(PathBuf::from("/current/out")),
                ..foreign_config()
            }
        );
    }

    /// `#[serde(default)]`: a field that is missing has its default, whichever it is.
    /// The test goes through all fields, so one added later is covered without being
    /// named anywhere.
    #[test]
    fn every_missing_field_has_its_default() {
        let complete = serde_json::to_value(foreign_config()).unwrap();
        let defaults = serde_json::to_value(AppConfig::default()).unwrap();

        for field in complete.as_object().unwrap().keys() {
            let mut partial = complete.clone();
            partial.as_object_mut().unwrap().remove(field);

            let read: AppConfig =
                serde_json::from_value(partial).unwrap_or_else(|e| panic!("without {field}: {e}"));

            // Compared as configurations: the order of a set in JSON is arbitrary.
            let mut expected = complete.clone();
            expected[field] = defaults[field].clone();
            let expected: AppConfig = serde_json::from_value(expected).unwrap();
            assert_eq!(read, expected, "without {field}");
        }
    }
}
