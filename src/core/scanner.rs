use std::path::Path;
use std::collections::HashSet;
use tokio::sync::mpsc;
use walkdir::WalkDir;
use anyhow::Result;

use super::{FileItem, ScanProgress};
use crate::utils::file_detection::is_text_file;

pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }
    
    pub async fn scan_directory(
        &self,
        root_path: &Path,
        progress_sender: mpsc::UnboundedSender<ScanProgress>,
    ) -> Result<Vec<FileItem>> {
        let mut files = Vec::new();
        let mut processed = 0;
        
        // First pass: count total items
        let total = WalkDir::new(root_path)
            .into_iter()
            .filter_map(|e| e.ok())
            .count();
            
        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed: 0,
            total,
            status: "Starting scan...".to_string(),
        })?;
        
        // Second pass: process items
        for entry in WalkDir::new(root_path) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!("Failed to access entry: {}", e);
                    continue;
                }
            };
            
            let path = entry.path();
            
            // Check ignore patterns
            if self.should_ignore(path) {
                continue;
            }
            
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(e) => {
                    tracing::warn!("Failed to get metadata for {}: {}", path.display(), e);
                    continue;
                }
            };
            
            let is_directory = metadata.is_dir();
            let size = metadata.len();
            
            // Determine if file is binary
            let is_binary = if is_directory {
                false
            } else {
                !is_text_file(path).unwrap_or(false)
            };
            
            // Calculate depth relative to root
            let depth = path.strip_prefix(root_path)
                .map(|p| p.components().count())
                .unwrap_or(0);
            
            let file_item = FileItem {
                path: path.to_path_buf(),
                is_directory,
                is_binary,
                size,
                depth,
                parent: path.parent().map(|p| p.to_path_buf()),
                children: Vec::new(),
            };
            
            files.push(file_item);
            processed += 1;
            
            // Send progress update
            if processed % 10 == 0 || processed == total {
                let _ = progress_sender.send(ScanProgress {
                    current_file: path.to_path_buf(),
                    processed,
                    total,
                    status: format!("Scanning... {}/{}", processed, total),
                });
            }
            
            // Check file size limit (100MB)
            if !is_directory && size > 100 * 1024 * 1024 {
                tracing::warn!("File {} exceeds 100MB limit, skipping", path.display());
                continue;
            }
            
            // Yield control periodically for UI responsiveness
            if processed % 100 == 0 {
                tokio::task::yield_now().await;
            }
        }
        
        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed: total,
            total,
            status: "Scan complete!".to_string(),
        })?;
        
        Ok(files)
    }
    
    fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        
        for pattern in &self.ignore_patterns {
            if pattern.ends_with('/') {
                // Directory pattern
                let dir_pattern = &pattern[..pattern.len() - 1];
                if path_str.contains(dir_pattern) {
                    return true;
                }
            } else if pattern.starts_with('*') {
                // Extension pattern
                let ext = &pattern[1..];
                if path_str.ends_with(ext) {
                    return true;
                }
            } else {
                // Exact match or contains
                if path_str.contains(pattern) {
                    return true;
                }
            }
        }
        
        // Default system ignores
        let file_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
            
        matches!(file_name, ".DS_Store" | "Thumbs.db" | "desktop.ini")
    }
}

impl Default for DirectoryScanner {
    fn default() -> Self {
        let mut ignore_patterns = HashSet::new();
        
        // Default ignore patterns
        ignore_patterns.insert("node_modules/".to_string());
        ignore_patterns.insert("target/".to_string());
        ignore_patterns.insert(".git/".to_string());
        ignore_patterns.insert("*.log".to_string());
        ignore_patterns.insert("*.tmp".to_string());
        
        Self { ignore_patterns }
    }
}