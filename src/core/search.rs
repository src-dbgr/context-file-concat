use regex::Regex;
use std::path::Path;

use super::{FileItem, SearchFilter};

pub struct SearchEngine;

impl SearchEngine {
    pub fn filter_files(files: &[FileItem], filter: &SearchFilter) -> Vec<FileItem> {
        files
            .iter()
            .filter(|file| Self::matches_filter(file, filter))
            .cloned()
            .collect()
    }
    
    fn matches_filter(file: &FileItem, filter: &SearchFilter) -> bool {
        // Check if binary files should be shown
        if file.is_binary && !filter.show_binary {
            return false;
        }
        
        // Check ignore patterns
        if Self::matches_ignore_patterns(&file.path, &filter.ignore_patterns) {
            return false;
        }
        
        // Check search query
        if !filter.query.is_empty() && !Self::matches_search_query(&file.path, &filter.query, filter.case_sensitive) {
            return false;
        }
        
        // Check file extension filter
        if !filter.extension.is_empty() && !Self::matches_extension(&file.path, &filter.extension) {
            return false;
        }
        
        true
    }
    
    fn matches_ignore_patterns(path: &Path, ignore_patterns: &std::collections::HashSet<String>) -> bool {
        let path_str = path.to_string_lossy();
        
        for pattern in ignore_patterns {
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
        
        false
    }
    
    fn matches_search_query(path: &Path, query: &str, case_sensitive: bool) -> bool {
        let file_name = path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        
        let path_str = path.to_string_lossy();
        
        if case_sensitive {
            file_name.contains(query) || path_str.contains(query)
        } else {
            let query_lower = query.to_lowercase();
            file_name.to_lowercase().contains(&query_lower) || 
            path_str.to_lowercase().contains(&query_lower)
        }
    }
    
    fn matches_extension(path: &Path, extension_filter: &str) -> bool {
        if let Some(ext) = path.extension().and_then(|ext| ext.to_str()) {
            let filter = if extension_filter.starts_with('.') {
                &extension_filter[1..]
            } else {
                extension_filter
            };
            
            ext.eq_ignore_ascii_case(filter)
        } else {
            // If file has no extension, only match if filter is empty or "no extension"
            extension_filter.is_empty() || extension_filter == "no extension"
        }
    }
    
    pub fn build_regex_filter(pattern: &str, case_sensitive: bool) -> Result<Regex, regex::Error> {
        let flags = if case_sensitive { "" } else { "(?i)" };
        let full_pattern = format!("{}{}", flags, pattern);
        Regex::new(&full_pattern)
    }
    
    pub fn filter_with_regex(files: &[FileItem], regex: &Regex) -> Vec<FileItem> {
        files
            .iter()
            .filter(|file| {
                let file_name = file.path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                regex.is_match(file_name)
            })
            .cloned()
            .collect()
    }
    
    pub fn get_file_extensions(files: &[FileItem]) -> Vec<String> {
        let mut extensions = std::collections::HashSet::new();
        
        for file in files {
            if !file.is_directory {
                if let Some(ext) = file.path.extension().and_then(|ext| ext.to_str()) {
                    extensions.insert(format!(".{}", ext));
                }
            }
        }
        
        let mut ext_vec: Vec<String> = extensions.into_iter().collect();
        ext_vec.sort();
        ext_vec
    }
}