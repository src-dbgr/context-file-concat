use super::{FileItem, TreeGenerator};
use crate::utils::file_detection::is_text_file;
use anyhow::Result;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

pub struct FileHandler;

impl FileHandler {
    pub async fn generate_concatenated_content_simple(
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> Result<String> {
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

            // KORRIGIERTE PFAD-LOGIK
            let display_path = if use_relative_paths {
                // Relativer Pfad: Beinhaltet das Hauptverzeichnis
                // z.B. context-file-concat/src/main.rs
                if let Some(parent) = root_path.parent() {
                    file_path
                        .strip_prefix(parent)
                        .unwrap_or(file_path)
                        .display()
                        .to_string()
                } else {
                    file_path.display().to_string()
                }
            } else {
                // Absoluter Pfad
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
                Err(_) => {
                    content.push_str("[BINARY OR UNREADABLE FILE - CONTENT SKIPPED]\n");
                }
            }
            content.push_str("----------------------FILE-END-------------------\n\n");
        }
        Ok(content)
    }

    fn read_file_content(file_path: &Path) -> Result<String> {
        let metadata = fs::metadata(file_path)?;
        if metadata.len() > 20 * 1024 * 1024 {
            return Ok(format!(
                "[FILE TOO LARGE: {} bytes - CONTENT SKIPPED]",
                metadata.len()
            ));
        }

        match fs::read_to_string(file_path) {
            Ok(content) => Ok(content),
            Err(_) => {
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
}
