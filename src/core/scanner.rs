use std::path::Path;
use std::collections::HashSet;
use tokio::sync::mpsc;
use walkdir::WalkDir;
use anyhow::Result;

use super::{FileItem, ScanProgress};

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
    ) -> Result<(Vec<FileItem>, usize, Vec<String>)> {
        let mut files = Vec::new();
        let mut processed = 0;
        let mut large_files_count = 0;
        let mut large_files_names = Vec::new();

        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed: 0,
            total: 0, // Unknown until complete
            status: "Starting scan...".to_string(),
            file_size: None,
            line_count: None,
        })?;

        // Single pass: process items directly without counting first
        for entry in WalkDir::new(root_path) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    tracing::warn!("Failed to access entry: {}", e);
                    continue;
                }
            };
            
            let path = entry.path();
            
            // Quick ignore check before expensive operations
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
            
            // Check file size limit (20MB)
            if !is_directory && size > 20 * 1024 * 1024 {
                large_files_count += 1;
                large_files_names.push(path.display().to_string());
                tracing::warn!("File {} exceeds 20MB limit, skipping", path.display());
                continue;
            }
            
            let is_binary = if is_directory {
                false
            } else {
                // 1. Size-based quick reject (auch für "kleinere" große Dateien)
                if !is_directory && size > 20 * 1024 * 1024 { // max 20mb files
                    large_files_count += 1;
                    large_files_names.push(path.display().to_string());
                    continue; // Skip komplett
                }
                
                // 2. Extension-only check (NO file content reading!)
                Self::is_likely_binary_by_extension(path)
            };
            
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
            
            // Send progress updates with DEBUG info
            if processed % 10 == 0 || processed < 100 { // Every 10 files OR first 100 files
                let _ = progress_sender.send(ScanProgress {
                    current_file: path.to_path_buf(),
                    processed,
                    total: 0, // Will be set at the end
                    status: format!("Scanning... {} items found", processed),
                    file_size: None,
                    line_count: None,
                });
            }

            // Yield control more frequently for responsive UI
            if processed % 50 == 0 {
                tokio::task::yield_now().await;
            }
        }

        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed,
            total: processed, // Now we know total
            status: "Scan complete!".to_string(),
            file_size: None,
            line_count: None,
        })?;

        Ok((files, large_files_count, large_files_names))
    }
    
    fn is_likely_binary_by_extension(path: &Path) -> bool {
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            let ext_lower = extension.to_lowercase();
            
            // Known text extensions - be VERY generous to avoid file reading
            const DEFINITELY_TEXT: &[&str] = &[
                "txt", "md", "markdown", "rst", "asciidoc", "adoc",
                "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "cpp", "cxx", "cc", "h", "hpp", "hxx",
                "go", "rb", "php", "swift", "kt", "kts", "scala", "clj", "cljs", "hs", "ml", "fs", "fsx",
                "html", "htm", "xml", "xhtml", "css", "scss", "sass", "less", "svg", "vue", "svelte",
                "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "config", "properties",
                "sql", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "dockerfile", "makefile", "cmake",
                "gradle", "maven", "pom", "build", "tex", "bib", "r", "m", "pl", "lua", "vim", "el", "lisp",
                "dart", "elm", "ex", "exs", "erl", "hrl", "nim", "crystal", "cr", "zig", "odin", "v",
                "log", "trace", "out", "err", "diff", "patch", "gitignore", "gitattributes", "editorconfig",
                "env", "example", "sample", "template", "spec", "test", "readme", "license", "changelog",
                "todo", "notes", "doc", "docs", "man", "help", "faq",
                "lock", "sum", "mod", "work", "pest", "ron", "d.ts", "mjs", "cjs", "coffee",
                "graphql", "gql", "prisma", "proto", "csv", "tsv", "data", "org", "R", "Rmd", "jl", "pyi",
                "rakefile", "gemfile", "procfile", "capfile", "jenkinsfile", "fastfile",
                "npmignore", "dockerignore", "eslintrc", "babelrc", "nvmrc", "rvmrc"
            ];
            
            // Known binary extensions
            const DEFINITELY_BINARY: &[&str] = &[
                "exe", "dll", "so", "dylib", "app", "deb", "rpm", "msi",
                "zip", "tar", "gz", "bz2", "7z", "rar", "jar", "war",
                "mp3", "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm", "m4a", "wav", "ogg",
                "jpg", "jpeg", "png", "gif", "bmp", "ico", "webp", "tiff", "tif", "raw", "heic", "heif",
                "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
                "bin", "dat", "db", "sqlite", "sqlite3", "dmg", "iso", "img", "pkg",
                "class", "pyc", "pyo", "o", "obj", "lib", "a", "rlib"
            ];
            
            if DEFINITELY_TEXT.contains(&ext_lower.as_str()) {
                return false; // Definitely text
            }
            
            if DEFINITELY_BINARY.contains(&ext_lower.as_str()) {
                return true; // Definitely binary
            }
        }
        
        // Unknown extension: assume TEXT (performance over accuracy)
        // Better to include some binary files than to read every unknown file
        false
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
            } else if pattern.contains('*') || pattern.contains('?') {
                // Wildcard pattern
                let file_name = path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                if Self::wildcard_match(file_name, pattern) || Self::wildcard_match(&path_str, pattern) {
                    return true;
                }
            } else if pattern.starts_with('*') {
                // Simple extension pattern (legacy)
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

    fn wildcard_match(text: &str, pattern: &str) -> bool {
        let mut text_chars = text.chars().peekable();
        let mut pattern_chars = pattern.chars().peekable();
        
        while let Some(&pattern_char) = pattern_chars.peek() {
            match pattern_char {
                '*' => {
                    pattern_chars.next();
                    if pattern_chars.peek().is_none() {
                        return true;
                    }
                    let remaining_pattern: String = pattern_chars.collect();
                    while text_chars.peek().is_some() {
                        let remaining_text: String = text_chars.clone().collect();
                        if Self::wildcard_match(&remaining_text, &remaining_pattern) {
                            return true;
                        }
                        text_chars.next();
                    }
                    return false;
                }
                '?' => {
                    pattern_chars.next();
                    if text_chars.next().is_none() {
                        return false;
                    }
                }
                _ => {
                    pattern_chars.next();
                    if text_chars.next() != Some(pattern_char) {
                        return false;
                    }
                }
            }
        }
        text_chars.next().is_none()
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