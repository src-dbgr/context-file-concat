pub mod settings;

use std::collections::HashSet;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use anyhow::Result;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub ignore_patterns: HashSet<String>,
    pub last_directory: Option<PathBuf>,
    pub output_directory: Option<PathBuf>,
    pub case_sensitive_search: bool,
    pub show_binary_files: bool,
    pub include_tree_by_default: bool,
    pub remove_empty_directories: bool,
    pub window_size: (f32, f32),
    pub window_position: Option<(f32, f32)>,
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        settings::load_config()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut ignore_patterns = HashSet::new();
        
        // Default ignore patterns for build artifacts and caches
        ignore_patterns.insert("node_modules".to_string());
        ignore_patterns.insert("target".to_string());
        ignore_patterns.insert(".idea".to_string());
        ignore_patterns.insert(".git".to_string());
        ignore_patterns.insert("*.log".to_string());
        ignore_patterns.insert("*.tmp".to_string());
        ignore_patterns.insert(".DS_Store".to_string());
        ignore_patterns.insert("Thumbs.db".to_string());
        ignore_patterns.insert("__pycache__".to_string());
        ignore_patterns.insert("*.pyc".to_string());
        ignore_patterns.insert("*.pyo".to_string());
        ignore_patterns.insert("*.class".to_string());
        ignore_patterns.insert("*.o".to_string());
        ignore_patterns.insert("*.obj".to_string());
        ignore_patterns.insert("package-lock.json".to_string());
        ignore_patterns.insert("*.lock".to_string());
        ignore_patterns.insert(".gitignore".to_string());

        // *** HINZUGEFÜGT: Common image file patterns to ignore ***
        let image_extensions = ["*.png", "*.jpg", "*.jpeg", "*.gif", "*.bmp", "*.ico", "*.webp", "*.tiff", "*.tif", "*.heic", "*.heif", "*.avif", "*.raw", "*.icns"];
        for ext in image_extensions {
            ignore_patterns.insert(ext.to_string());
        }

        // *** HINZUGEFÜGT: Common binary file patterns to ignore ***
        let binary_extensions = ["*.exe", "*.dll", "*.so", "*.dylib", "*.app", "*.deb", "*.rpm", "*.msi", "*.jar", "*.war", "*.a", "*.lib", "*.rlib", "*.pdf", "*.doc", "*.docx", "*.xls", "*.xlsx", "*.ppt", "*.pptx", "*.zip", "*.tar", "*.gz", "*.7z", "*.rar", "*.bin", "*.dat", "*.db", "*.sqlite", "*.mp4", "*.mp3"];
        for ext in binary_extensions {
            ignore_patterns.insert(ext.to_string());
        }
        
        Self {
            ignore_patterns,
            last_directory: None,
            output_directory: dirs::desktop_dir(),
            case_sensitive_search: false,
            show_binary_files: true,
            include_tree_by_default: true,
            remove_empty_directories: false,
            window_size: (1200.0, 800.0),
            window_position: None,
        }
    }
}