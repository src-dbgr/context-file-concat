pub mod settings;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

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
    pub auto_load_last_directory: bool,
    /// Size limit for files in MiB: larger files are not listed by the scan, not read
    /// by the content search and not inlined into the generated output.
    pub max_file_size_mb: u64,
    pub scan_chunk_size: usize,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        settings::load_config(None)
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
            auto_load_last_directory: false,
            max_file_size_mb: 20,
            scan_chunk_size: 100,
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
}
