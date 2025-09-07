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
    pub max_file_size_mb: u64,
    pub scan_chunk_size: usize,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        settings::load_config(None)
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
            // VET: Use a deterministic, static filename for the default implementation.
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
