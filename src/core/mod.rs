//! The `core` module contains the primary business logic of the application.
//!
//! This includes functionalities like scanning directories, filtering files based on various criteria,
//! generating file trees, and handling file content. It is designed to be independent of the UI
//! and could potentially be used in other contexts (e.g., a command-line tool).

pub mod error;
pub mod file_handler;
pub mod ignore;
pub mod scanner;
pub mod search;
pub mod tree_generator;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Re-export CoreError to make it accessible from the app module.
pub use error::CoreError;

/// Represents a single item (file or directory) found during a directory scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    /// The full path to the file or directory.
    pub path: PathBuf,
    /// `true` if the item is a directory.
    pub is_directory: bool,
    /// `true` if the file is detected as binary.
    pub is_binary: bool,
    /// The size of the file in bytes. For directories, this is typically 0.
    pub size: u64,
    /// The depth of the item in the directory tree, relative to the scan root.
    pub depth: usize,
    /// The path of the parent directory, if it exists.
    pub parent: Option<PathBuf>,
}

/// Defines the criteria for filtering files.
#[derive(Debug, Clone)]
pub struct SearchFilter {
    /// A string to match against filenames.
    pub query: String,
    /// A file extension to filter by (e.g., "rs", "txt").
    pub extension: String,
    /// `true` if the filename query should be case-sensitive.
    pub case_sensitive: bool,
    /// A set of `.gitignore`-style patterns to exclude files and directories.
    pub ignore_patterns: std::collections::HashSet<String>,
}

// Re-export der ScanProgress aus scanner
pub use scanner::ScanProgress;

pub use file_handler::FileHandler;
pub use ignore::build_globset_from_patterns;
pub use scanner::DirectoryScanner;
pub use search::SearchEngine;
pub use tree_generator::TreeGenerator;
