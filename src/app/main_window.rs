use eframe::egui;
use std::path::PathBuf;
use tokio::sync::mpsc;
use std::sync::Arc;

use super::ContextFileConcatApp;
use crate::core::{DirectoryScanner, FileHandler, SearchEngine, TreeGenerator, SearchFilter};

impl ContextFileConcatApp {
    pub fn render_main_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
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
            
            ui.separator();
            ui.label(format!("{} files found, {} selected", 
                self.filtered_files.len(), 
                self.selected_files.len()
            ));
        });
        
        ui.separator();
        
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(300.0)
            .show(ui, |ui| {
                for file_item in &self.filtered_files.clone() {
                    self.render_file_item(ui, file_item);
                }
            });
    }
    
    fn render_file_item(&mut self, ui: &mut egui::Ui, file_item: &crate::core::FileItem) {
        ui.horizontal(|ui| {
            // Checkbox
            let mut is_selected = self.selected_files.contains(&file_item.path);
            if ui.checkbox(&mut is_selected, "").changed() {
                if is_selected {
                    self.selected_files.insert(file_item.path.clone());
                } else {
                    self.selected_files.remove(&file_item.path);
                }
            }
            
            // File icon
            let icon = if file_item.is_directory {
                "📁"
            } else if file_item.is_binary {
                "🔧"
            } else {
                "📄"
            };
            ui.label(icon);
            
            // File name (clickable for preview)
            let name = file_item.path.file_name()
                .unwrap_or_default()
                .to_string_lossy();
                
            if ui.selectable_label(
                self.preview_file.as_ref() == Some(&file_item.path),
                name.as_ref()
            ).clicked() && !file_item.is_directory {
                self.load_file_preview(&file_item.path);
            }
            
            // File size
            if !file_item.is_directory {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(format_file_size(file_item.size));
                });
            }
        });
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
                    .desired_width(300.0)
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
                    .desired_width(200.0)
            );
            
            ui.separator();
            
            ui.checkbox(&mut self.include_tree, "Include directory tree");
            
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
            Some(TreeGenerator::generate_tree(
                &self.file_tree,
                &root_path,
                &self.config.ignore_patterns
            ))
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