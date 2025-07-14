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
        let file_name = path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        
        for pattern in ignore_patterns {
            if pattern.ends_with('/') {
                // Directory pattern
                let dir_pattern = &pattern[..pattern.len() - 1];
                if path_str.contains(dir_pattern) {
                    return true;
                }
            } else if pattern.contains('*') || pattern.contains('?') {
                // Wildcard pattern - check against filename
                if Self::wildcard_match(file_name, pattern) {
                    return true;
                }
                // Also check against full path for directory patterns
                if Self::wildcard_match(&path_str, pattern) {
                    return true;
                }
            } else if pattern.starts_with('*') {
                // Simple extension pattern (legacy support)
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

    // NEW: Wildcard matching function
    fn wildcard_match(text: &str, pattern: &str) -> bool {
        let mut text_chars = text.chars().peekable();
        let mut pattern_chars = pattern.chars().peekable();
        
        while let Some(&pattern_char) = pattern_chars.peek() {
            match pattern_char {
                '*' => {
                    pattern_chars.next(); // consume '*'
                    
                    // If * is the last character, match everything remaining
                    if pattern_chars.peek().is_none() {
                        return true;
                    }
                    
                    // Try to match the rest of the pattern at each position
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
                    pattern_chars.next(); // consume '?'
                    if text_chars.next().is_none() {
                        return false; // ? must match exactly one character
                    }
                }
                _ => {
                    pattern_chars.next(); // consume pattern char
                    if text_chars.next() != Some(pattern_char) {
                        return false;
                    }
                }
            }
        }
        
        // Both should be exhausted for a complete match
        text_chars.next().is_none()
    }
    
    fn matches_search_query(path: &Path, query: &str, case_sensitive: bool) -> bool {
        // Only check the actual filename, not the full path
        let file_name = path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        
        if case_sensitive {
            file_name.contains(query)
        } else {
            let query_lower = query.to_lowercase();
            file_name.to_lowercase().contains(&query_lower)
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

}