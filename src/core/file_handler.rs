use std::path::{Path, PathBuf};
use std::fs;
use std::io::{BufRead, BufReader};
use anyhow::Result;
use tokio::sync::mpsc;

use super::ScanProgress;
use crate::utils::file_detection::is_text_file;

pub struct FileHandler;

impl FileHandler {
    pub async fn generate_concatenated_file(
        selected_files: &[PathBuf],
        output_path: &Path,
        include_tree: bool,
        tree_content: Option<String>,
        progress_sender: mpsc::UnboundedSender<ScanProgress>,
    ) -> Result<()> {
        let mut content = String::new();
        let total_files = selected_files.len();
        
        // Add header
        content.push_str("# ContextFileConcat Output\n");
        content.push_str(&format!("# Generated: {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
        content.push_str(&format!("# Total files: {}\n", total_files));
        content.push_str("\n");
        
        // Process each file
        for (i, file_path) in selected_files.iter().enumerate() {
            progress_sender.send(ScanProgress {
                current_file: file_path.clone(),
                processed: i,
                total: total_files,
                status: format!("Processing file {}/{}", i + 1, total_files),
            })?;
            
            // Skip directories
            if file_path.is_dir() {
                continue;
            }
            
            // Check if file is text
            if !is_text_file(file_path).unwrap_or(false) {
                content.push_str(&format!("{}\n", file_path.display()));
                content.push_str("----------------------------------------------------\n");
                content.push_str("[BINARY FILE - CONTENT SKIPPED]\n");
                content.push_str("----------------------------------------------------\n\n");
                continue;
            }
            
            // Add file header
            content.push_str(&format!("{}\n", file_path.display()));
            content.push_str("----------------------------------------------------\n");
            
            // Read and add file content
            match Self::read_file_content(file_path) {
                Ok(file_content) => {
                    content.push_str(&file_content);
                    if !file_content.ends_with('\n') {
                        content.push('\n');
                    }
                }
                Err(e) => {
                    content.push_str(&format!("[ERROR READING FILE: {}]\n", e));
                }
            }
            
            content.push_str("----------------------------------------------------\n\n");
            
            // Yield control periodically
            if i % 10 == 0 {
                tokio::task::yield_now().await;
            }
        }
        
        // Add tree at the end if requested
        if include_tree {
            if let Some(tree) = tree_content {
                content.push_str("\n# DIRECTORY TREE\n");
                content.push_str("====================================================\n");
                content.push_str(&tree);
                content.push_str("====================================================\n");
            }
        }
        
        // Write final file
        progress_sender.send(ScanProgress {
            current_file: output_path.to_path_buf(),
            processed: total_files,
            total: total_files,
            status: "Writing output file...".to_string(),
        })?;
        
        fs::write(output_path, content)?;
        
        progress_sender.send(ScanProgress {
            current_file: output_path.to_path_buf(),
            processed: total_files,
            total: total_files,
            status: "Complete!".to_string(),
        })?;
        
        tracing::info!("Successfully generated concatenated file: {}", output_path.display());
        Ok(())
    }
    
    fn read_file_content(file_path: &Path) -> Result<String> {
        // Check file size first (100MB limit)
        let metadata = fs::metadata(file_path)?;
        if metadata.len() > 100 * 1024 * 1024 {
            return Ok(format!("[FILE TOO LARGE: {} bytes - CONTENT SKIPPED]", metadata.len()));
        }
        
        // Try to read as UTF-8 first
        match fs::read_to_string(file_path) {
            Ok(content) => Ok(content),
            Err(_) => {
                // If UTF-8 fails, try reading as bytes and converting
                let bytes = fs::read(file_path)?;
                match String::from_utf8_lossy(&bytes) {
                    content if content.contains('\u{FFFD}') => {
                        Ok("[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]".to_string())
                    }
                    content => Ok(content.to_string()),
                }
            }
        }
    }
    
    pub fn get_file_preview(file_path: &Path, max_lines: usize) -> Result<String> {
        if file_path.is_dir() {
            return Ok("[DIRECTORY]".to_string());
        }
        
        if !is_text_file(file_path).unwrap_or(false) {
            return Ok("[BINARY FILE]".to_string());
        }
        
        let file = fs::File::open(file_path)?;
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
    
    pub fn calculate_total_size(files: &[PathBuf]) -> u64 {
        files
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .filter(|metadata| metadata.is_file())
            .map(|metadata| metadata.len())
            .sum()
    }
    
    pub fn estimate_output_size(files: &[PathBuf]) -> u64 {
        let content_size = Self::calculate_total_size(files);
        let overhead_per_file = 100; // Estimated overhead for headers and separators
        let total_overhead = files.len() as u64 * overhead_per_file;
        
        content_size + total_overhead
    }
}