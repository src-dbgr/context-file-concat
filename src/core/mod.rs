//! The `core` module contains the primary business logic of the application.
//!
//! This includes functionalities like scanning directories, filtering files based on various criteria,
//! generating file trees, and handling file content. It is designed to be independent of the UI
//! and could potentially be used in other contexts (e.g., a command-line tool).

pub mod error;
pub mod file_handler;
pub mod scanner;
pub mod search;
pub mod tree_generator;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// Re-export CoreError to make it accessible from the app module.
pub use error::CoreError;

/// Size limit shared by the whole core: files larger than this are not listed by the
/// scanner, not read by the content search and not inlined into the generated output.
pub(crate) const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Represents a single item (file or directory) found during a directory scan.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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

// The public vocabulary of the core, so that callers do not depend on its file layout.
pub use file_handler::FileHandler;
pub use scanner::{DirectoryScanner, ScanProgress, ScanRequest};
pub use search::{is_blank, ContentSearch, ContentSearchRequest, FileNameMatcher, SearchEngine};
pub use tree_generator::TreeGenerator;
