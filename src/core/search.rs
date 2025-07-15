use std::path::Path;
use rayon::prelude::*;
use super::{FileItem, SearchFilter, build_globset_from_patterns}; // MODIFIED

pub struct SearchEngine;

impl SearchEngine {
    // Stellt die Logik zum Filtern nach Ignore-Patterns wieder her.
    pub fn filter_files(files: &[FileItem], filter: &SearchFilter) -> Vec<FileItem> {
        let ignore_glob_set = build_globset_from_patterns(&filter.ignore_patterns);
        
        files
            .par_iter()
            .filter(|file| Self::matches_filter(file, filter, &ignore_glob_set))
            .cloned()
            .collect()
    }
    
    // Berücksichtigt wieder das ignore_glob_set.
    fn matches_filter(file: &FileItem, filter: &SearchFilter, ignore_glob_set: &globset::GlobSet) -> bool {
        if file.is_binary && !filter.show_binary {
            return false;
        }
        
        // Die Prüfung ist zurück, um die UI-Filterung in Echtzeit zu ermöglichen.
        if file.path.components().any(|c| c.as_os_str() == ".git") || ignore_glob_set.is_match(&file.path) {
            return false;
        }
        
        if !filter.query.is_empty() && !Self::matches_search_query(&file.path, &filter.query, filter.case_sensitive) {
            return false;
        }
        
        if !filter.extension.is_empty() && !Self::matches_extension(&file.path, &filter.extension) {
            return false;
        }
        
        true
    }
    
    // Die restlichen Methoden bleiben unverändert.
    fn matches_search_query(path: &Path, query: &str, case_sensitive: bool) -> bool {
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
            extension_filter.is_empty() || extension_filter == "no extension"
        }
    }

    pub fn remove_empty_directories(mut files: Vec<FileItem>) -> Vec<FileItem> {
        let mut has_changes = true;
        
        while has_changes {
            has_changes = false;
            let files_before_len = files.len();
            
            let mut dirs_to_remove = Vec::new();
            
            for item in &files {
                if item.is_directory {
                    let has_children = files.iter().any(|other| {
                        other.path != item.path && other.path.starts_with(&item.path)
                    });
                    
                    if !has_children {
                        dirs_to_remove.push(item.path.clone());
                    }
                }
            }
            
            if !dirs_to_remove.is_empty() {
                files.retain(|item| !dirs_to_remove.contains(&item.path));
                if files.len() != files_before_len {
                    has_changes = true;
                }
            }
        }
        
        files
    }
}