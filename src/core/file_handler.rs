use std::path::{Path, PathBuf};
use std::fs;
use std::io::{BufRead, BufReader};
use anyhow::Result;
use tokio::sync::mpsc;
use std::collections::HashSet;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use super::{FileItem, ScanProgress, TreeGenerator};
use crate::utils::file_detection::is_text_file;

// KORREKTUR 1: Die fehlende Struktur-Definition wird hier hinzugefügt.
pub struct FileHandler;

impl FileHandler {

    pub async fn generate_concatenated_content(
        selected_files: &[PathBuf],
        root_path: &Path,
        use_relative_paths: bool,
        progress_sender: mpsc::UnboundedSender<ScanProgress>,
        cancel_flag: Arc<AtomicBool>,
        include_tree: bool,
        items_for_tree: Vec<FileItem>, // <-- Geänderter Parameter
        tree_ignore_patterns: HashSet<String>, // <-- Geänderter Parameter
    ) -> Result<(String, u64, usize)> {
        let mut content = String::new();
        let total_files = selected_files.len();
        
        content.push_str("# ContextFileConcat Output\n");
        content.push_str(&format!("# Generated: {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S")));
        content.push_str(&format!("# Total files: {}\n", total_files));
        content.push_str("\n");
        
        // Schritt 1: Verarbeite alle ausgewählten Dateien (unverändert)
        for (i, file_path) in selected_files.iter().enumerate() {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(anyhow::anyhow!("Content generation cancelled by user."));
            }

            progress_sender.send(ScanProgress {
                current_file: file_path.clone(),
                processed: i,
                total: total_files,
                status: format!("Processing file {}/{}", i + 1, total_files),
                file_size: None,
                line_count: None,
            })?;
            
            if file_path.is_dir() { continue; }

            let display_path = if use_relative_paths {
                if let Some(parent) = root_path.parent() {
                    file_path.strip_prefix(parent).unwrap_or(file_path).display().to_string()
                } else { file_path.display().to_string() }
            } else { file_path.display().to_string() };
            
            if !is_text_file(file_path).unwrap_or(false) {
                content.push_str(&format!("{}\n", display_path));
                content.push_str("=====================FILE-START==================\n");
                content.push_str("[BINARY FILE - CONTENT SKIPPED]\n");
                content.push_str("----------------------FILE-END-------------------\n\n");
                continue;
            }
            
            content.push_str(&format!("{}\n", display_path));
            content.push_str("=====================FILE-START==================\n");
            
            match Self::read_file_content(file_path) {
                Ok(file_content) => {
                    content.push_str(&file_content);
                    if !file_content.ends_with('\n') { content.push('\n'); }
                }
                Err(e) => { content.push_str(&format!("[ERROR READING FILE: {}]\n", e)); }
            }
            
            content.push_str("----------------------FILE-END-------------------\n\n");
            
            if i % 10 == 0 { tokio::task::yield_now().await; }
        }
        
        // Schritt 2: Generiere den Verzeichnisbaum HIER im Hintergrund-Thread
        if include_tree {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(anyhow::anyhow!("Content generation cancelled by user."));
            }
            
            // Die Logik zum Filtern der `items_for_tree` ist nun im UI-Thread,
            // also können wir sie hier direkt verwenden.
            let tree = TreeGenerator::generate_tree(&items_for_tree, root_path, &tree_ignore_patterns);
            
            content.push_str("# DIRECTORY TREE\n");
            content.push_str("====================================================\n");
            content.push_str(&tree);
            content.push_str("====================================================\n");
        }
        
        progress_sender.send(ScanProgress {
            current_file: PathBuf::from("Finalizing..."),
            processed: total_files, total: total_files,
            status: "Finalizing content...".to_string(),
            file_size: None, line_count: None,
        })?;
        
        let file_size = content.len() as u64;
        let line_count = content.lines().count();

        progress_sender.send(ScanProgress {
            current_file: PathBuf::from("Generated Preview"),
            processed: total_files, total: total_files,
            status: "Complete!".to_string(),
            file_size: Some(file_size),
            line_count: Some(line_count),
        })?;
        
        tracing::info!("Successfully generated concatenated content in memory.");
        Ok((content, file_size, line_count))
    }

    fn read_file_content(file_path: &Path) -> Result<String> {
        let metadata = fs::metadata(file_path)?;
        if metadata.len() > 20 * 1024 * 1024 {
            return Ok(format!("[FILE TOO LARGE: {} bytes - CONTENT SKIPPED]", metadata.len()));
        }
        
        match fs::read_to_string(file_path) {
            Ok(content) => Ok(content),
            Err(_) => {
                let bytes = fs::read(file_path)?;
                match String::from_utf8_lossy(&bytes) {
                    content if content.contains('\u{FFFD}') => {
                        Ok("[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]".to_string())
                    }
                    // KORREKTUR 2: Der temporäre Slice wird in einen eigenen String umgewandelt.
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
}