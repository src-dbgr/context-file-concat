use super::{build_globset_from_patterns, FileItem};
use crate::utils::file_detection::is_text_file;
use anyhow::Result;
use globset::GlobSet;
use std::collections::HashSet;
use std::path::Path;
use walkdir::WalkDir;

pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }

    pub async fn scan_directory_basic(&self, root_path: &Path) -> Result<Vec<FileItem>> {
        let ignore_glob_set = build_globset_from_patterns(&self.ignore_patterns);
        let mut files = Vec::new();

        for entry in WalkDir::new(root_path).into_iter().filter_map(Result::ok) {
            let path = entry.path();
            if Self::should_ignore(path, &ignore_glob_set) {
                if entry.file_type().is_dir() {
                    // This is how walkdir implements skipping directories
                    // entry.skip_subtree();
                }
                continue;
            }

            let metadata = match entry.metadata() {
                Ok(md) => md,
                Err(_) => continue,
            };

            if metadata.len() > 20 * 1024 * 1024 {
                // Skip files > 20MB
                continue;
            }

            files.push(FileItem {
                path: path.to_path_buf(),
                is_directory: metadata.is_dir(),
                is_binary: if metadata.is_file() {
                    !is_text_file(path).unwrap_or(false)
                } else {
                    false
                },
                size: metadata.len(),
                depth: entry.depth(),
                parent: path.parent().map(|p| p.to_path_buf()),
            });
        }
        Ok(files)
    }

    fn should_ignore(path: &Path, ignore_glob_set: &GlobSet) -> bool {
        if path.components().any(|c| c.as_os_str() == ".git") {
            return true;
        }
        ignore_glob_set.is_match(path)
    }
}
