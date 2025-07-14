use eframe::egui;
use std::path::PathBuf;
use std::collections::HashSet;
use tokio::sync::mpsc;
use std::sync::Arc;

use super::ContextFileConcatApp;
use crate::core::{DirectoryScanner, FileHandler, SearchEngine, TreeGenerator, SearchFilter, FileItem};

impl ContextFileConcatApp {
    pub fn render_main_ui(&mut self, _ui: &mut egui::Ui, ctx: &egui::Context) {
        // Top toolbar panel
        egui::TopBottomPanel::top("toolbar")
            .default_height(60.0)
            .show(ctx, |ui| {
                self.render_toolbar(ui);
            });
        
        // Bottom panel for output settings
        egui::TopBottomPanel::bottom("output_panel")
            .default_height(60.0)
            .show(ctx, |ui| {
                self.render_bottom_panel(ui);
            });
        
        // Left side panel for search and filters
        egui::SidePanel::left("left_panel")
            .default_width(300.0)
            .min_width(250.0)
            .show(ctx, |ui| {
                self.render_left_panel(ui);
            });
        
        // Central panel for file list and preview
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_right_panel(ui);
        });
        
        // Progress overlay
        if self.is_scanning {
            self.render_progress_overlay(ctx);
        }
    }
    
    fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label("📁 Root Directory:");
        
        if ui.button("Select Directory").clicked() {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.current_path = path.to_string_lossy().to_string();
                self.start_directory_scan();
            }
        }
        
        ui.add(
            egui::TextEdit::singleline(&mut self.current_path)
                .desired_width(400.0)
                .hint_text("Enter directory path or use Select Directory button")
        );
        
        if ui.button("Scan").clicked() && !self.current_path.is_empty() {
            self.start_directory_scan();
        }
        
        ui.separator();
        
        // Config buttons
        if ui.button("💾 Save Config").clicked() {
            self.save_current_config();
        }
        
        if ui.button("📂 Load Config").clicked() {
            self.load_config_dialog();
        }
    }
    
    fn render_left_panel(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.heading("🔍 Search & Filter");
            
            // Search input
            ui.horizontal(|ui| {
                ui.label("Search:");
                if ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Search in filenames...")
                ).changed() {
                    self.apply_filters();
                }
            });
            
            // File extension filter
            ui.horizontal(|ui| {
                ui.label("Extension:");
                if ui.add(
                    egui::TextEdit::singleline(&mut self.file_extension_filter)
                        .hint_text("e.g., .rs, .js, .py")
                ).changed() {
                    self.apply_filters();
                }
            });
            
            // Filter options
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.case_sensitive, "Case sensitive").changed() {
                    self.apply_filters();
                }
                if ui.checkbox(&mut self.show_binary_files, "Show binary files").changed() {
                    self.apply_filters();
                }
            });
            
            ui.separator();
            
            // Ignore patterns
            ui.heading("🚫 Ignore Patterns");
            ui.label("Common patterns:");
            
            ui.horizontal_wrapped(|ui| {
                let common_patterns = [
                    "node_modules/", "target/", ".git/", "*.log", 
                    "*.tmp", ".DS_Store", "Thumbs.db", "*.class"
                ];
                
                for pattern in common_patterns {
                    if ui.small_button(pattern).clicked() {
                        self.config.ignore_patterns.insert(pattern.to_string());
                        self.apply_filters();
                    }
                }
            });
            
            // Custom ignore pattern input
            let mut new_pattern = String::new();
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut new_pattern)
                        .hint_text("Add custom pattern...")
                );
                if ui.button("Add").clicked() && !new_pattern.is_empty() {
                    self.config.ignore_patterns.insert(new_pattern);
                    self.apply_filters();
                }
            });
            
            // List current ignore patterns
            ui.collapsing("Current ignore patterns", |ui| {
                let patterns: Vec<String> = self.config.ignore_patterns.iter().cloned().collect();
                for pattern in patterns {
                    ui.horizontal(|ui| {
                        ui.label(&pattern);
                        if ui.small_button("❌").clicked() {
                            self.config.ignore_patterns.remove(&pattern);
                            self.apply_filters();
                        }
                    });
                }
            });
        });
    }
    
    fn render_right_panel(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            // File list section
            ui.group(|ui| {
                self.render_file_list(ui);
            });
            
            ui.separator();
            
            // File preview section
            ui.group(|ui| {
                self.render_file_preview(ui);
            });
        });
    }
    
    fn render_file_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("📄 Files");
        
        ui.horizontal(|ui| {
            if ui.button("Select All").clicked() {
                self.select_all_files();
            }
            if ui.button("Deselect All").clicked() {
                self.selected_files.clear();
            }
            if ui.button("Expand All").clicked() {
                self.expand_all_directories();
            }
            if ui.button("Collapse All").clicked() {
                self.expanded_dirs.clear();
            }
            
            ui.separator();
            ui.label(format!("{} files found, {} selected", 
                self.filtered_files.len(), 
                self.selected_files.len()
            ));
        });
        
        ui.separator();
        
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .max_height(300.0)
            .show(ui, |ui| {
                self.render_file_tree_recursive(ui);
            });
    }
    
    fn render_file_tree_recursive(&mut self, ui: &mut egui::Ui) {
        // Build a hierarchical structure from flat file list
        let mut root_items = Vec::new();
        let current_root = PathBuf::from(&self.current_path);
        
        // Find direct children of the current root
        for item in &self.filtered_files {
            if let Ok(relative) = item.path.strip_prefix(&current_root) {
                if relative.components().count() == 1 {
                    root_items.push(item.clone());
                }
            }
        }
        
        // Sort: directories first, then files
        root_items.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.path.file_name().cmp(&b.path.file_name()),
            }
        });
        
        // Render each root item
        for item in root_items {
            self.render_tree_item(ui, &item, 0);
        }
    }
    
    fn render_tree_item(&mut self, ui: &mut egui::Ui, item: &FileItem, indent_level: usize) {
        ui.horizontal(|ui| {
            // Indentation
            for _ in 0..indent_level {
                ui.add_space(20.0);
            }
            
            if item.is_directory {
                // Expand/collapse button for directories
                let is_expanded = self.expanded_dirs.contains(&item.path);
                let expand_symbol = if is_expanded { "▼" } else { "▶" };
                
                if ui.small_button(expand_symbol).clicked() {
                    if is_expanded {
                        self.expanded_dirs.remove(&item.path);
                    } else {
                        self.expanded_dirs.insert(item.path.clone());
                    }
                }
                
                // Directory checkbox
                let mut dir_selected = self.is_directory_selected(&item.path);
                let mut should_toggle = false;
                
                if ui.checkbox(&mut dir_selected.0, "").changed() {
                    should_toggle = true;
                }
                
                if should_toggle {
                    self.toggle_directory_selection(&item.path);
                }
                
                // Directory icon and name
                ui.label("📁");
                ui.label(item.path.file_name().unwrap_or_default().to_string_lossy());
            } else {
                // File checkbox
                let mut is_selected = self.selected_files.contains(&item.path);
                if ui.checkbox(&mut is_selected, "").changed() {
                    if is_selected {
                        self.selected_files.insert(item.path.clone());
                    } else {
                        self.selected_files.remove(&item.path);
                    }
                }
                
                // File icon
                let icon = if item.is_binary { "🔧" } else { "📄" };
                ui.label(icon);
                
                // File name (clickable for preview)
                let name = item.path.file_name().unwrap_or_default().to_string_lossy();
                if ui.selectable_label(
                    self.preview_file.as_ref() == Some(&item.path),
                    name.as_ref()
                ).clicked() {
                    self.load_file_preview(&item.path);
                }
                
                // File size
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format_file_size(item.size));
                });
            }
        });
        
        // Render children if directory is expanded
        if item.is_directory && self.expanded_dirs.contains(&item.path) {
            let children = self.get_directory_children(&item.path);
            for child in children {
                self.render_tree_item(ui, &child, indent_level + 1);
            }
        }
    }
    
    fn get_directory_children(&self, dir_path: &PathBuf) -> Vec<FileItem> {
        let mut children = Vec::new();
        
        for item in &self.filtered_files {
            if let Some(parent) = item.path.parent() {
                if parent == dir_path {
                    children.push(item.clone());
                }
            }
        }
        
        // Sort: directories first, then files
        children.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.path.file_name().cmp(&b.path.file_name()),
            }
        });
        
        children
    }
    
    fn is_directory_selected(&self, dir_path: &PathBuf) -> (bool, bool) {
        let children = self.get_all_files_in_directory(dir_path);
        if children.is_empty() {
            return (false, false);
        }
        
        let selected_count = children.iter()
            .filter(|path| self.selected_files.contains(*path))
            .count();
            
        if selected_count == 0 {
            (false, false) // None selected
        } else if selected_count == children.len() {
            (true, false) // All selected
        } else {
            (true, true) // Partially selected (indeterminate)
        }
    }
    
    fn get_all_files_in_directory(&self, dir_path: &PathBuf) -> Vec<PathBuf> {
        let mut files = Vec::new();
        
        for item in &self.filtered_files {
            if !item.is_directory && item.path.starts_with(dir_path) {
                files.push(item.path.clone());
            }
        }
        
        files
    }
    
    fn toggle_directory_selection(&mut self, dir_path: &PathBuf) {
        let files_in_dir = self.get_all_files_in_directory(dir_path);
        let (is_selected, _) = self.is_directory_selected(dir_path);
        
        if is_selected {
            // Deselect all files in directory
            for file_path in files_in_dir {
                self.selected_files.remove(&file_path);
            }
        } else {
            // Select all files in directory
            for file_path in files_in_dir {
                self.selected_files.insert(file_path);
            }
        }
    }
    
    fn expand_all_directories(&mut self) {
        for item in &self.filtered_files {
            if item.is_directory {
                self.expanded_dirs.insert(item.path.clone());
            }
        }
    }
    
    fn render_file_preview(&mut self, ui: &mut egui::Ui) {
        ui.heading("👁️ Preview");
        
        if let Some(preview_file) = &self.preview_file {
            ui.label(format!("📄 {}", preview_file.file_name().unwrap_or_default().to_string_lossy()));
            ui.separator();
            
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .max_height(200.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.preview_content)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(10)
                    );
                });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a file to preview");
            });
        }
    }
    
    fn render_bottom_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("📤 Output Settings");
            
            ui.separator();
            
            ui.label("Output Directory:");
            ui.add(
                egui::TextEdit::singleline(&mut self.output_path)
                    .desired_width(200.0)
            );
            
            if ui.button("Browse").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.output_path = path.to_string_lossy().to_string();
                }
            }
            
            ui.separator();
            
            ui.label("Filename:");
            ui.add(
                egui::TextEdit::singleline(&mut self.output_filename)
                    .desired_width(150.0)
            );
            
            ui.separator();
            
            // Tree options
            ui.vertical(|ui| {
                ui.checkbox(&mut self.include_tree, "Include directory tree");
                
                if self.include_tree {
                    ui.horizontal(|ui| {
                        ui.add_space(20.0);
                        if ui.checkbox(&mut self.tree_full_mode, "Full tree mode").changed() {
                            // Reset tree ignore patterns when switching modes
                            if !self.tree_full_mode {
                                self.tree_ignore_patterns.clear();
                            }
                        }
                    });
                    
                    if self.tree_full_mode {
                        ui.horizontal(|ui| {
                            ui.add_space(20.0);
                            ui.label("Tree ignore patterns:");
                        });
                        
                        // Tree-specific ignore patterns
                        ui.horizontal(|ui| {
                            ui.add_space(40.0);
                            let mut new_tree_pattern = String::new();
                            ui.add(
                                egui::TextEdit::singleline(&mut new_tree_pattern)
                                    .hint_text("Add tree ignore pattern...")
                                    .desired_width(150.0)
                            );
                            if ui.button("Add").clicked() && !new_tree_pattern.is_empty() {
                                self.tree_ignore_patterns.insert(new_tree_pattern);
                            }
                        });
                        
                        // Show current tree ignore patterns
                        if !self.tree_ignore_patterns.is_empty() {
                            ui.horizontal(|ui| {
                                ui.add_space(40.0);
                                ui.vertical(|ui| {
                                    let patterns: Vec<String> = self.tree_ignore_patterns.iter().cloned().collect();
                                    for pattern in patterns {
                                        ui.horizontal(|ui| {
                                            ui.label(&pattern);
                                            if ui.small_button("❌").clicked() {
                                                self.tree_ignore_patterns.remove(&pattern);
                                            }
                                        });
                                    }
                                });
                            });
                        }
                    }
                }
            });
            
            ui.separator();
            
            let can_generate = !self.selected_files.is_empty() && !self.is_scanning;
            
            if ui.add_enabled(can_generate, egui::Button::new("🚀 Generate").min_size(egui::Vec2::new(100.0, 30.0)))
                .clicked() {
                self.generate_output();
            }
        });
    }
}

fn format_file_size(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut size = size as f64;
    let mut unit_index = 0;
    
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }
    
    if unit_index == 0 {
        format!("{} {}", size as u64, UNITS[unit_index])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}

// Additional implementation methods
impl ContextFileConcatApp {
    pub fn start_directory_scan(&mut self) {
        if self.current_path.is_empty() || self.is_scanning {
            return;
        }
        
        self.is_scanning = true;
        self.scan_progress = None;
        self.file_tree.clear();
        self.filtered_files.clear();
        
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (file_sender, file_receiver) = mpsc::unbounded_channel();
        
        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        
        let path = PathBuf::from(&self.current_path);
        let ignore_patterns = self.config.ignore_patterns.clone();
        
        // Store the file receiver to get scan results
        let file_receiver = Arc::new(tokio::sync::Mutex::new(file_receiver));
        
        // Spawn async scanning task
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let scanner = DirectoryScanner::new(ignore_patterns);
                
                match scanner.scan_directory(&path, progress_sender).await {
                    Ok(files) => {
                        // Send the files through the channel
                        let _ = file_sender.send(files);
                        tracing::info!("Scan completed");
                    }
                    Err(e) => {
                        tracing::error!("Scan failed: {}", e);
                    }
                }
            });
        });
        
        // Store the file receiver for later retrieval
        self.file_receiver = Some(file_receiver);
    }
    
    pub fn update_progress(&mut self) {
        let mut should_stop_scanning = false;
        let mut progress_update = None;
        let mut new_files = None;
        
        // Check for progress updates
        if let Some(receiver) = &self.progress_receiver {
            if let Ok(mut rx) = receiver.try_lock() {
                while let Ok(progress) = rx.try_recv() {
                    progress_update = Some(progress.clone());
                    
                    // If scan is complete, update file list
                    if progress.processed >= progress.total && progress.status.contains("complete") {
                        should_stop_scanning = true;
                    }
                }
            }
        }
        
        // Check for file updates
        if let Some(file_receiver) = &self.file_receiver {
            if let Ok(mut rx) = file_receiver.try_lock() {
                if let Ok(files) = rx.try_recv() {
                    new_files = Some(files);
                }
            }
        }
        
        if let Some(progress) = progress_update {
            self.scan_progress = Some(progress);
        }
        
        if let Some(files) = new_files {
            self.file_tree = files;
        }
        
        if should_stop_scanning {
            self.is_scanning = false;
            self.apply_filters();
        }
    }
    
    pub fn apply_filters(&mut self) {
        let filter = SearchFilter {
            query: self.search_query.clone(),
            extension: self.file_extension_filter.clone(),
            case_sensitive: self.case_sensitive,
            show_binary: self.show_binary_files,
            ignore_patterns: self.config.ignore_patterns.clone(),
        };
        
        self.filtered_files = SearchEngine::filter_files(&self.file_tree, &filter);
    }
    
    pub fn select_all_files(&mut self) {
        for file in &self.filtered_files {
            if !file.is_directory {
                self.selected_files.insert(file.path.clone());
            }
        }
    }
    
    pub fn load_file_preview(&mut self, file_path: &PathBuf) {
        self.preview_file = Some(file_path.clone());
        
        match FileHandler::get_file_preview(file_path, 20) {
            Ok(content) => {
                self.preview_content = content;
            }
            Err(e) => {
                self.preview_content = format!("Error loading preview: {}", e);
            }
        }
    }
    
    pub fn save_current_config(&mut self) {
        if let Err(e) = self.config.save() {
            tracing::error!("Failed to save config: {}", e);
        } else {
            tracing::info!("Config saved successfully");
        }
    }
    
    pub fn load_config_dialog(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("JSON Config", &["json"])
            .pick_file()
        {
            match crate::config::settings::import_config(&file) {
                Ok(config) => {
                    self.config = config;
                    tracing::info!("Config loaded from {:?}", file);
                }
                Err(e) => {
                    tracing::error!("Failed to load config: {}", e);
                }
            }
        }
    }
    
    pub fn generate_output(&mut self) {
        if self.selected_files.is_empty() || self.is_scanning {
            return;
        }
        
        let output_path = PathBuf::from(&self.output_path).join(&self.output_filename);
        let selected_files: Vec<PathBuf> = self.selected_files.iter().cloned().collect();
        let include_tree = self.include_tree;
        
        // Generate tree if needed
        let tree_content = if include_tree {
            let root_path = PathBuf::from(&self.current_path);
            
            if self.tree_full_mode {
                // Full tree mode with separate ignore patterns
                Some(TreeGenerator::generate_tree(
                    &self.file_tree,
                    &root_path,
                    &self.tree_ignore_patterns
                ))
            } else {
                // Selected files only mode
                let selected_items: Vec<_> = self.file_tree.iter()
                    .filter(|item| {
                        if item.is_directory {
                            // Include directory if any selected file is inside it
                            selected_files.iter().any(|f| f.starts_with(&item.path))
                        } else {
                            // Include file if it's selected
                            selected_files.contains(&item.path)
                        }
                    })
                    .cloned()
                    .collect();
                    
                Some(TreeGenerator::generate_tree(
                    &selected_items,
                    &root_path,
                    &HashSet::new() // No additional ignores for selected files mode
                ))
            }
        } else {
            None
        };
        
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        self.is_scanning = true; // Reuse progress system for generation
        
        // Spawn async generation task
        let rt = tokio::runtime::Runtime::new().unwrap();
        std::thread::spawn(move || {
            rt.block_on(async move {
                match FileHandler::generate_concatenated_file(
                    &selected_files,
                    &output_path,
                    include_tree,
                    tree_content,
                    progress_sender,
                ).await {
                    Ok(_) => {
                        tracing::info!("File generation completed: {}", output_path.display());
                    }
                    Err(e) => {
                        tracing::error!("File generation failed: {}", e);
                    }
                }
            });
        });
    }
    
    pub fn render_progress_overlay(&mut self, ctx: &egui::Context) {
        if let Some(progress) = &self.scan_progress.clone() {
            egui::Window::new("Progress")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("Processing...");
                        
                        let progress_fraction = if progress.total > 0 {
                            progress.processed as f32 / progress.total as f32
                        } else {
                            0.0
                        };
                        
                        ui.add(egui::ProgressBar::new(progress_fraction)
                            .text(format!("{}/{}", progress.processed, progress.total)));
                        
                        ui.label(&progress.status);
                        
                        if let Some(file_name) = progress.current_file.file_name() {
                            ui.label(format!("Current: {}", file_name.to_string_lossy()));
                        }
                        
                        if ui.button("Cancel").clicked() {
                            // Handle cancel outside this closure
                        }
                    });
                });
        }
    }
}