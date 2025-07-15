pub mod scanner;
pub mod file_handler;
pub mod search;
pub mod tree_generator;
pub mod ignore; // <-- HINZUGEFÜGT

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct FileItem {
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_binary: bool,
    pub size: u64,
    pub depth: usize,
    pub parent: Option<PathBuf>,
    pub children: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct ScanProgress {
    pub current_file: PathBuf,
    pub processed: usize,
    pub total: usize,
    pub status: String,
    pub file_size: Option<u64>,
    pub line_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct SearchFilter {
    pub query: String,
    pub extension: String,
    pub case_sensitive: bool,
    pub ignore_patterns: std::collections::HashSet<String>,
}

pub use scanner::DirectoryScanner;
pub use file_handler::FileHandler;
pub use search::SearchEngine;
pub use tree_generator::TreeGenerator;
pub use ignore::build_globset_from_patterns; // <-- HINZUGEFÜGT