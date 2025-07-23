pub mod file_handler;
pub mod ignore;
pub mod scanner;
pub mod search;
pub mod tree_generator;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItem {
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    pub depth: usize,
    pub parent: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub query: String,
    pub extension: String,
    pub case_sensitive: bool,
    pub ignore_patterns: std::collections::HashSet<String>,
}

pub use file_handler::FileHandler;
pub use ignore::build_globset_from_patterns;
pub use scanner::DirectoryScanner;
pub use search::SearchEngine;
pub use tree_generator::TreeGenerator;
