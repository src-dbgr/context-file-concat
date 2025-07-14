pub mod main_window;

use eframe::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;
use anyhow::Result; // Import Result for the receiver

use crate::core::{FileItem, ScanProgress};
use crate::config::AppConfig;

pub struct ContextFileConcatApp {
    // UI State
    current_path: String,
    selected_files: HashSet<PathBuf>,
    file_tree: Vec<FileItem>,
    filtered_files: Vec<FileItem>,
    expanded_dirs: HashSet<PathBuf>,  // Track which directories are expanded
    
    // Search and Filter
    search_query: String,
    file_extension_filter: String,
    search_in_files_query: String,  // New: Search inside file content
    case_sensitive: bool,
    show_binary_files: bool,
    
    // Input field states
    new_ignore_pattern: String,
    new_tree_pattern: String,
    
    // Progress
    scan_progress: Option<ScanProgress>,
    is_scanning: bool,
    is_generating: bool, // NEW: Track generation progress separately
    is_searching_content: bool,  // New: Track content search progress
    
    // Config
    config: AppConfig,
    
    // Output settings
    output_path: String,
    output_filename: String,
    include_tree: bool,
    tree_full_mode: bool,  // New: Full tree vs selected files only
    tree_ignore_patterns: HashSet<String>,  // New: Separate ignore patterns for tree
    
    // File preview
    preview_content: String,
    preview_file: Option<PathBuf>,
    generated_content: Option<String>, // NEW: To store the concatenated output for preview
    
    // Layout state
    file_list_height: f32,  // New: Manual height control
    
    // Search highlighting
    highlighted_preview_content: Vec<PreviewSegment>,  // New: For search highlighting

    // Large files warning
    large_files_count: usize,  // NEW: Track files skipped due to size
    large_files_names: Vec<String>,  // NEW: Names of skipped files
    show_large_files_warning: bool,  // NEW: Show warning popup

    // Async communication
    progress_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<ScanProgress>>>>,
    file_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<(Vec<FileItem>, usize, Vec<String>)>>>>,
    content_search_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<Vec<FileItem>>>>>,
    // NEW: Receiver for the generated content string. Result<(content, size, lines), error_string>
    generation_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<Result<(String, u64, usize), String>>>>>,
}

// New: For search highlighting in preview
#[derive(Clone, Debug)]
pub struct PreviewSegment {
    pub text: String,
    pub is_match: bool,
}

impl ContextFileConcatApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let config = AppConfig::load().unwrap_or_default();
        
        let output_filename = format!(
            "output_{}.txt", 
            chrono::Local::now().format("%Y%m%d_%H%M%S")
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
            show_binary_files: true,
            new_ignore_pattern: String::new(),
            new_tree_pattern: String::new(),
            scan_progress: None,
            is_scanning: false,
            is_generating: false, // NEW
            is_searching_content: false,
            config,
            output_path: dirs::desktop_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
                .to_string_lossy()
                .to_string(),
            output_filename,
            include_tree: false,
            tree_full_mode: false,
            tree_ignore_patterns: HashSet::new(),
            preview_content: String::new(),
            preview_file: None,
            generated_content: None, // NEW
            file_list_height: 400.0,  // New: Default height
            highlighted_preview_content: Vec::new(),  // New
            large_files_count: 0,  // NEW
            large_files_names: Vec::new(),  // NEW
            show_large_files_warning: false,  // NEW
            progress_receiver: None,
            file_receiver: None,
            content_search_receiver: None,
            generation_receiver: None, // NEW
        }
    }
}

impl eframe::App for ContextFileConcatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_progress();
        
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui, ctx);
        });
        
        // Request repaint häufiger für bessere Progress Updates
        if self.is_scanning || self.is_searching_content || self.is_generating { // MODIFIED
            ctx.request_repaint_after(std::time::Duration::from_millis(50)); // 20 FPS
        }
    }
    
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Save app state
        if let Ok(config_json) = serde_json::to_string(&self.config) {
            storage.set_string("app_config", config_json);
        }
    }
}