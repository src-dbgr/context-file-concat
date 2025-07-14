pub mod main_window;

use eframe::egui;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::{mpsc, Mutex};
use std::sync::Arc;

use crate::core::{FileItem, ScanProgress};
use crate::config::AppConfig;

pub struct ContextFileConcatApp {
    // UI State
    current_path: String,
    selected_files: HashSet<PathBuf>,
    file_tree: Vec<FileItem>,
    filtered_files: Vec<FileItem>,
    
    // Search and Filter
    search_query: String,
    file_extension_filter: String,
    case_sensitive: bool,
    show_binary_files: bool,
    
    // Progress
    scan_progress: Option<ScanProgress>,
    is_scanning: bool,
    
    // Config
    config: AppConfig,
    
    // Output settings
    output_path: String,
    output_filename: String,
    include_tree: bool,
    
    // File preview
    preview_content: String,
    preview_file: Option<PathBuf>,
    
    // Async communication
    progress_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<ScanProgress>>>>,
    file_receiver: Option<Arc<Mutex<mpsc::UnboundedReceiver<Vec<FileItem>>>>>,
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
            search_query: String::new(),
            file_extension_filter: String::new(),
            case_sensitive: false,
            show_binary_files: true,
            scan_progress: None,
            is_scanning: false,
            config,
            output_path: dirs::desktop_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default())
                .to_string_lossy()
                .to_string(),
            output_filename,
            include_tree: false,
            preview_content: String::new(),
            preview_file: None,
            progress_receiver: None,
            file_receiver: None,
        }
    }
}

impl eframe::App for ContextFileConcatApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_progress();
        
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_main_ui(ui, ctx);
        });
        
        // Request repaint for smooth progress updates
        if self.is_scanning {
            ctx.request_repaint();
        }
    }
    
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        // Save app state
        if let Ok(config_json) = serde_json::to_string(&self.config) {
            storage.set_string("app_config", config_json);
        }
    }
}