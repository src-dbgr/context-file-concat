pub mod settings;
pub mod store;

pub use store::{ConfigSnapshot, ConfigStore};

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// The unit [`AppConfig::window_size`] and [`AppConfig::window_position`] are stored in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GeometryUnits {
    /// Physical pixels. This is what versions up to 0.3.1 stored, without saying so:
    /// a configuration that does not name its unit is read as physical. Never written
    /// any more; the geometry is converted when the application starts (see
    /// `app::window_geometry`).
    Physical,
    /// Logical units, independent of the scale factor of the display.
    Logical,
}

impl GeometryUnits {
    /// The unit of a stored geometry that does not name one.
    fn of_unmarked_config() -> Self {
        Self::Physical
    }
}

/// The persisted settings of the application.
///
/// Fields that are not part of this struct are ignored when a configuration is read
/// (deliberately no `deny_unknown_fields`), so files written by an older version keep
/// loading after a setting has been retired, e.g. the former `scan_chunk_size` and
/// `auto_load_last_directory`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub window_size: (f64, f64),
    pub window_position: (f64, f64),
    /// The unit of `window_size` and `window_position`.
    #[serde(default = "GeometryUnits::of_unmarked_config")]
    pub geometry_units: GeometryUnits,
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
    /// Takes over the settings of `imported` that a user edits and shares: patterns,
    /// search and output options, the file size limit.
    ///
    /// What describes this installation rather than a preference stays as it is: the
    /// window geometry (an imported file may come from another display, or store it
    /// in another unit) and the directory that was opened last. (Whether the directory
    /// named by the imported file is opened is up to the caller; opening it is what
    /// makes it the last directory.)
    pub fn adopt_user_settings(&mut self, imported: AppConfig) {
        *self = AppConfig {
            window_size: self.window_size,
            window_position: self.window_position,
            geometry_units: self.geometry_units,
            last_directory: self.last_directory.take(),
            ..imported
        };
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
            output_directory: dirs::desktop_dir(),
            // A static name keeps `Default` deterministic; timestamped names are assigned
            // when a generation starts.
            output_filename: "cfc_output.txt".to_string(),
            case_sensitive_search: false,
            include_tree_by_default: true,
            use_relative_paths: true,
            remove_empty_directories: false,
            window_size: (1200.0, 800.0),
            window_position: (100.0, 100.0),
            geometry_units: GeometryUnits::Logical,
            max_file_size_mb: 20,
        }
    }
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

    #[test]
    fn a_config_that_does_not_name_its_geometry_unit_is_read_as_physical() {
        let mut json = serde_json::to_value(AppConfig::default()).unwrap();
        assert_eq!(json["geometry_units"], "logical");

        json.as_object_mut().unwrap().remove("geometry_units");
        let unmarked: AppConfig = serde_json::from_value(json).unwrap();

        assert_eq!(unmarked.geometry_units, GeometryUnits::Physical);
    }

    #[test]
    fn adopting_user_settings_keeps_geometry_and_last_directory() {
        let mut current = AppConfig {
            window_size: (1000.0, 700.0),
            window_position: (7.0, 8.0),
            geometry_units: GeometryUnits::Logical,
            last_directory: Some(PathBuf::from("/current/project")),
            ..AppConfig::default()
        };
        let imported = AppConfig {
            ignore_patterns: HashSet::from(["*.imported".to_string()]),
            tree_ignore_patterns: HashSet::from(["docs".to_string()]),
            output_directory: Some(PathBuf::from("/imported/out")),
            output_filename: "imported.txt".to_string(),
            case_sensitive_search: true,
            include_tree_by_default: false,
            use_relative_paths: false,
            remove_empty_directories: true,
            max_file_size_mb: 3,
            window_size: (4000.0, 3000.0),
            window_position: (-900.0, -900.0),
            geometry_units: GeometryUnits::Physical,
            last_directory: Some(PathBuf::from("/somebody/elses/project")),
        };

        current.adopt_user_settings(imported.clone());

        assert_eq!(
            current,
            AppConfig {
                window_size: (1000.0, 700.0),
                window_position: (7.0, 8.0),
                geometry_units: GeometryUnits::Logical,
                last_directory: Some(PathBuf::from("/current/project")),
                ..imported
            }
        );
    }
}
