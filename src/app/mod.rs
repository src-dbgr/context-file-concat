pub mod main_window;

use eframe::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::{mpsc, Mutex};
use std::sync::{Arc, atomic::AtomicBool};
use anyhow::Result;

use crate::core::{FileItem, ScanProgress};
use crate::config::AppConfig;

pub struct ContextFileConcatApp {
    // UI State
    current_path: String,
    selected_files: HashSet<PathBuf>,
    file_tree: Vec<FileItem>,
    filtered_files: Vec<FileItem>,
    expanded_dirs: HashSet<PathBuf>,
    
    // Search and Filter
    search_query: String,
    file_extension_filter: String,
    search_in_files_query: String,
    case_sensitive: bool,
    
    // Input field states
    new_ignore_pattern: String,
    new_tree_pattern: String,
    ignore_pattern_filter: String,
    
    // Progress
    scan_progress: Option<ScanProgress>,
    is_scanning: bool,
    is_generating: bool,
    is_searching_content: bool,
    
    // Config
    config: AppConfig,
    
    // Output settings
    output_path: String,
    output_filename: String,
    include_tree: bool,
    tree_ignore_patterns: HashSet<String>,
    use_relative_paths: bool,
    
    // File preview
    preview_content: String,
    preview_file: Option<PathBuf>,
    generated_content: Option<String>,
    
    // Layout state
    file_list_height: f32,
    
    // *** MODIFIZIERT: Speichert Hervorhebungen pro Zeile ***
    highlighted_preview_lines: Vec<Vec<PreviewSegment>>,

    // Large files warning
    large_files_count: usize,
    large_files_names: Vec<String>,
    show_large_files_warning: bool,

    // Async communication
    progress_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<ScanProgress>>>>,
    file_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<(Vec<FileItem>, usize, Vec<String>)>>>>,
    content_search_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<Vec<FileItem>>>>>,
    generation_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<Result<(String, u64, usize), String>>>>>,
    
    // For cancelling scans
    cancel_flag: Option<Arc<AtomicBool>>,
    generation_cancel_flag: Option<Arc<AtomicBool>>,
    save_error_message: Option<String>,
    
    // Speichert Patterns, die durch "Remove empty dirs" erzeugt wurden
    auto_removed_dir_patterns: HashSet<String>,
}

#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub text: String,
    pub is_match: bool,
}

// in src/app/mod.rs

impl ContextFileConcatApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let config = AppConfig::load().unwrap_or_default();
        
        let output_filename = format!(
            "cfc_output_{}.txt", 
            chrono::Local::now().format("%d%m%Y_%H%M%S")
        );
        
        Self {
            current_path: String::new(),
            selected_files: HashSet::new(),
            file_tree: Vec::new(),
            filtered_files: Vec::new(),
            expanded_dirs: HashSet::new(),
            search_query: String::new(),
            file_extension_filter: String::new(),
            search_in_files_query: String::new(),
            case_sensitive: false,
            new_ignore_pattern: String::new(),
            new_tree_pattern: String::new(),
            ignore_pattern_filter: String::new(),
            scan_progress: None,
            is_scanning: false,
            is_generating: false,
            is_searching_content: false,
            config: config.clone(),
            output_path: dirs::desktop_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
                .to_string_lossy()
                .to_string(),
            output_filename,
            include_tree: config.include_tree_by_default,
            tree_ignore_patterns: HashSet::new(),
            use_relative_paths: true,
            preview_content: String::new(),
            preview_file: None,
            generated_content: None,
            file_list_height: 400.0,
            highlighted_preview_lines: Vec::new(),
            large_files_count: 0,
            large_files_names: Vec::new(),
            show_large_files_warning: false,
            progress_receiver: None,
            file_receiver: None,
            content_search_receiver: None,
            generation_receiver: None,
            cancel_flag: None,
            generation_cancel_flag: None,
            save_error_message: None,
            auto_removed_dir_patterns: HashSet::new(),
        }
    }
}

impl eframe::App for ContextFileConcatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_progress();
        
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui, ctx);
        });
        
        if self.is_scanning || self.is_searching_content || self.is_generating {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(config_json) = serde_json::to_string(&self.config) {
            storage.set_string("app_config", config_json);
        }
    }
}