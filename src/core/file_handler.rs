//! Handles file content operations like reading, previewing, and concatenation.

use super::{CoreError, FileItem, TreeGenerator};
use crate::utils::file_detection::is_text_file;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// A utility struct for handling file-related operations.
///
/// This struct is stateless and provides methods as associated functions.
pub struct FileHandler;

impl FileHandler {
    /// Generates a single string by concatenating the content of selected files.
    ///
    /// It includes a header with metadata, an optional directory tree, and formatted
    /// content blocks for each selected file.
    pub async fn generate_concatenated_content_simple(
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String, CoreError> {
        let mut content = String::new();
        content.push_str(&format!(
            "# CFC Output - Generated: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        content.push_str(&format!("# Total files: {}\n\n", selected_files.len()));

        if include_tree {
            let tree =
                TreeGenerator::generate_tree(&items_for_tree, root_path, &tree_ignore_patterns);
            content.push_str("# DIRECTORY TREE\n");
            content.push_str("====================================================\n");
            content.push_str(&tree);
            content.push_str("====================================================\n\n");
        }

        for file_path in selected_files {
            if file_path.is_dir() {
                continue;
            }

            let display_path = if use_relative_paths {
                if let Some(parent) = root_path.parent() {
                    file_path.strip_prefix(parent)?.display().to_string()
                } else {
                    file_path.display().to_string()
                }
            } else {
                file_path.display().to_string()
            };

            content.push_str(&format!("{}\n", display_path));
            content.push_str("=====================FILE-START==================\n");

            match Self::read_file_content(file_path) {
                Ok(file_content) => {
                    content.push_str(&file_content);
                    if !file_content.ends_with('\n') {
                        content.push('\n');
                    }
                }
                Err(e) => {
                    tracing::warn!("Skipping unreadable file {}: {}", file_path.display(), e);
                    content.push_str("[BINARY OR UNREADABLE FILE - CONTENT SKIPPED]\n");
                }
            }
            content.push_str("----------------------FILE-END-------------------\n\n");
        }
        Ok(content)
    }

    /// Reads the content of a file, with safeguards for large or binary files.
    fn read_file_content(file_path: &Path) -> Result<String, CoreError> {
        let metadata =
            fs::metadata(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;
        if metadata.len() > 20 * 1024 * 1024 {
            return Ok(format!(
                "[FILE TOO LARGE: {} bytes - CONTENT SKIPPED]",
                metadata.len()
            ));
        }

        match fs::read_to_string(file_path) {
            Ok(content) => Ok(content),
            Err(_) => {
                let bytes =
                    fs::read(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;
                match String::from_utf8_lossy(&bytes) {
                    content if content.contains('\u{FFFD}') => {
                        Ok("[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]".to_string())
                    }
                    content => Ok(content.to_string()),
                }
            }
        }
    }

    /// Retrieves a truncated preview of a text file's content.
    ///
    /// Reads up to a specified maximum number of lines. Identifies directories and binary files.
    pub fn get_file_preview(file_path: &Path, max_lines: usize) -> Result<String, CoreError> {
        if file_path.is_dir() {
            return Ok("[DIRECTORY]".to_string());
        }

        if !is_text_file(file_path).map_err(|e| {
            CoreError::Io(
                std::io::Error::new(std::io::ErrorKind::Other, e.to_string()),
                file_path.to_path_buf(),
            )
        })? {
            return Ok("[BINARY FILE]".to_string());
        }

        let file =
            fs::File::open(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;
        let reader = BufReader::new(file);
        let mut preview = String::new();
        let mut line_count = 0;

        for line in reader.lines() {
            if line_count >= max_lines {
                preview.push_str("...\n[Preview truncated]");
                break;
            }
            match line {
                Ok(line_content) => {
                    preview.push_str(&line_content);
                    preview.push('\n');
                }
                Err(_) => {
                    preview.push_str("[ERROR READING LINE]\n");
                }
            }
            line_count += 1;
        }
        Ok(preview)
    }
}
