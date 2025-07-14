use eframe::egui;
use std::path::PathBuf;
use std::collections::HashSet;
use tokio::sync::mpsc;
use std::sync::Arc;

use super::{ContextFileConcatApp, PreviewSegment};
use crate::core::{DirectoryScanner, FileHandler, SearchEngine, TreeGenerator, SearchFilter, FileItem, ScanProgress};
use crate::utils::file_detection::is_image_file;

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
            .min_height(150.0) // Increased height for new layout
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
        
        // Central panel for file list and preview - FIXED LAYOUT
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_right_panel_fixed(ui);
        });
        
        // Progress overlay
        if self.is_scanning || self.is_generating {
            self.render_progress_overlay(ctx);
        }

        // Large files warning
        if self.show_large_files_warning {
            self.render_large_files_warning(ctx);
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
        ui.add_space(1.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.current_path)
                .desired_width(400.0)
                .hint_text("Enter directory path or use Select Directory button")
        );
        ui.add_space(1.0);
        if ui.button("Scan").clicked() && !self.current_path.is_empty() {
            self.start_directory_scan();
        }
        
        ui.separator();
        
        // Config buttons
        if ui.button("💾 Export Config").clicked() {
            self.save_config_dialog();
        }
        ui.add_space(1.0);
        if ui.button("📂 Import Config").clicked() {
            self.load_config_dialog();
        }
        ui.add_space(1.0);
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
            
            ui.separator();
            
            // Search in files content
            ui.heading("🔍 Search in Files");
            ui.horizontal(|ui| {
                ui.label("Content:");
                if ui.add(
                    egui::TextEdit::singleline(&mut self.search_in_files_query)
                        .hint_text("Search text inside files...")
                ).changed() {
                    if !self.search_in_files_query.is_empty() {
                        self.start_content_search();
                    } else {
                        self.apply_filters(); // Reset to normal filtering
                    }
                    // Update preview highlighting when search changes
                    self.update_preview_highlighting();
                }
            });
            
            if self.is_searching_content {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Searching in files...");
                });
            }
            
            // Filter options
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.case_sensitive, "Case sensitive").changed() {
                    self.apply_filters();
                    self.update_preview_highlighting();
                }
                if ui.checkbox(&mut self.show_binary_files, "Show binary files").changed() {
                    self.apply_filters();
                }
            });
            
            ui.separator();

            // Ignore patterns
            ui.heading("🚫 Ignore Patterns");

            // Remove empty directories option
            if ui.checkbox(&mut self.config.remove_empty_directories, "Remove empty directories").changed() {
                self.apply_filters();
            }

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
            ui.add_space(1.0);
            // Custom ignore pattern input
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_ignore_pattern)
                        .hint_text("Add pattern (wildcards: *, ?)")
                );
                if ui.button("Add").clicked() && !self.new_ignore_pattern.is_empty() {
                    self.config.ignore_patterns.insert(self.new_ignore_pattern.clone());
                    self.new_ignore_pattern.clear();
                    self.apply_filters();
                }
            });
            
            // List current ignore patterns
            ui.collapsing("Current ignore patterns", |ui| {
                // Make it scrollable and take available height (ensure minimum height)
                let available_height = (ui.available_height() - 20.0).max(50.0); // Minimum 50px
                egui::ScrollArea::vertical()
                    .max_height(available_height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let patterns: Vec<String> = self.config.ignore_patterns.iter().cloned().collect();
                        for pattern in patterns {
                            ui.horizontal(|ui| {
                                if ui.small_button("❌").clicked() {
                                    self.config.ignore_patterns.remove(&pattern);
                                    self.apply_filters();
                                }
                                ui.label(&pattern);
                            });
                        }
                    });
            });
        });
    }
    
    fn render_right_panel_fixed(&mut self, ui: &mut egui::Ui) {
        let available_height = ui.available_height();
        let min_file_list_height = 150.0;
        let min_preview_height = 100.0;
        
        // Ensure file_list_height is within bounds
        self.file_list_height = self.file_list_height
            .max(min_file_list_height)
            .min((available_height - min_preview_height).max(min_file_list_height));
        
        ui.vertical(|ui| {
            // File list section
            ui.allocate_ui_with_layout(
                egui::Vec2::new(ui.available_width(), self.file_list_height),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    ui.group(|ui| {
                        ui.set_height((self.file_list_height - 8.0).max(50.0));
                        self.render_file_list(ui);
                    });
                }
            );
            
            // Resizer
            let resizer_response = ui.allocate_response(
                egui::Vec2::new(ui.available_width(), 1.0),
                egui::Sense::drag()
            );

            if resizer_response.dragged() {
                self.file_list_height += resizer_response.drag_delta().y;
            }

            if resizer_response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }

            ui.painter().rect_filled(
                resizer_response.rect,
                egui::Rounding::ZERO,
                if resizer_response.hovered() {
                    egui::Color32::from_gray(100)
                } else {
                    egui::Color32::from_gray(80)
                },
            );
            
            // Preview section
            ui.group(|ui| {
                ui.set_height((ui.available_height() - 8.0).max(50.0));
                self.render_file_preview_with_highlighting(ui);
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
        
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("file_tree_scroll")
            .show(ui, |ui| {
                self.render_file_tree_recursive(ui);
            });
    }
    
    fn render_file_tree_recursive(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.x = 2.0;
        let mut root_items = Vec::new();
        let current_root = PathBuf::from(&self.current_path);
        
        for item in &self.filtered_files {
            if let Ok(relative) = item.path.strip_prefix(&current_root) {
                if relative.components().count() == 1 {
                    root_items.push(item.clone());
                }
            }
        }
        
        root_items.sort_by(|a, b| {
            match (a.is_directory, b.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.path.file_name().cmp(&b.path.file_name()),
            }
        });
        
        for item in root_items {
            self.render_tree_item(ui, &item, 0);
        }
    }
    
    fn render_tree_item(&mut self, ui: &mut egui::Ui, item: &FileItem, indent_level: usize) {
        let is_search_match = self.is_search_match(item);
        
        ui.horizontal(|ui| {
            ui.add_space(indent_level as f32 * 35.0);
            
            if item.is_directory {
                let is_expanded = self.expanded_dirs.contains(&item.path);
                
                let expand_response = ui.add(
                    egui::Button::new(if is_expanded { "🔽" } else { "▶" })
                        .small()
                        .frame(false)
                );
                
                if expand_response.clicked() {
                    if is_expanded {
                        self.expanded_dirs.remove(&item.path);
                    } else {
                        self.expanded_dirs.insert(item.path.clone());
                    }
                }
                
                let mut dir_selected = self.is_directory_selected(&item.path);
                let mut should_toggle = false;

                if ui.checkbox(&mut dir_selected.0, "").changed() {
                    should_toggle = true;
                }

                if should_toggle {
                    self.toggle_directory_selection(&item.path);
                }

                let dir_selected = self.is_directory_selected(&item.path).0;

                if dir_selected {
                    ui.label("📁");
                } else {
                    ui.colored_label(egui::Color32::from_gray(120), "📁");
                }

                let dir_name = item.path.file_name().unwrap_or_default().to_string_lossy();
                if is_search_match {
                    ui.colored_label(egui::Color32::YELLOW, format!("🔍 {}", dir_name));
                } else if dir_selected {
                    ui.colored_label(egui::Color32::WHITE, dir_name.as_ref());
                } else {
                    ui.colored_label(egui::Color32::from_gray(160), dir_name.as_ref());
                }

                let ignore_button = ui.add(
                    egui::Button::new("i")
                        .small()
                        .min_size(egui::Vec2::new(16.0, 16.0))
                        .fill(egui::Color32::from_gray(30))
                );

                if ignore_button.clicked() {
                    if let Some(dir_name) = item.path.file_name().and_then(|n| n.to_str()) {
                        self.config.ignore_patterns.insert(format!("{}/", dir_name));
                        self.apply_filters();
                    }
                }

                if ignore_button.hovered() {
                    egui::show_tooltip_at_pointer(
                        ui.ctx(), 
                        egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("ignore_dir_tooltip")), 
                        egui::Id::new("ignore_dir_tooltip"),
                        |ui: &mut egui::Ui| {
                            ui.label("Add directory to ignore patterns");
                        }
                    );
                }
            } else {
                let mut is_selected = self.selected_files.contains(&item.path);
                if ui.checkbox(&mut is_selected, "").changed() {
                    if is_selected {
                        self.selected_files.insert(item.path.clone());
                    } else {
                        self.selected_files.remove(&item.path);
                    }
                }
                
                let icon = if is_image_file(&item.path) {
                    "📷"
                } else if item.is_binary {
                    "🔧"
                } else {
                    "📄"
                };
                
                if is_selected {
                    ui.label(icon);
                } else {
                    ui.colored_label(egui::Color32::from_gray(120), icon);
                }
                
                let name = item.path.file_name().unwrap_or_default().to_string_lossy();
                let label_text = if is_search_match {
                    format!("🔍 {}", name)
                } else {
                    name.to_string()
                };
                
                let response = if is_search_match {
                    ui.selectable_label(
                        self.preview_file.as_ref() == Some(&item.path),
                        egui::RichText::new(label_text).color(egui::Color32::YELLOW)
                    )
                } else if is_selected {
                    ui.selectable_label(
                        self.preview_file.as_ref() == Some(&item.path),
                        egui::RichText::new(label_text).color(egui::Color32::WHITE)
                    )
                } else {
                    ui.selectable_label(
                        self.preview_file.as_ref() == Some(&item.path),
                        egui::RichText::new(label_text).color(egui::Color32::from_gray(160))
                    )
                };
                
                if response.clicked() {
                    self.load_file_preview(&item.path);
                }
                
                let ignore_button = ui.add(
                    egui::Button::new("i")
                        .small()
                        .min_size(egui::Vec2::new(16.0, 16.0))
                        .fill(egui::Color32::from_gray(30))
                );
                
                if ignore_button.clicked() {
                    if let Some(file_name) = item.path.file_name().and_then(|n| n.to_str()) {
                        self.config.ignore_patterns.insert(file_name.to_string());
                        self.apply_filters();
                    }
                }
                
                if ignore_button.hovered() {
                    egui::show_tooltip_at_pointer(
                        ui.ctx(), 
                        egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("ignore_tooltip")), 
                        egui::Id::new("ignore_tooltip"),
                        |ui: &mut egui::Ui| {
                            ui.label("Add to ignore patterns");
                        }
                    );
                }
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(20.0);
                    if is_selected {
                        ui.label(format_file_size(item.size));
                    } else {
                        ui.colored_label(egui::Color32::from_gray(120), format_file_size(item.size));
                    }
                });
            }
        });
        
        if item.is_directory && self.expanded_dirs.contains(&item.path) {
            let children = self.get_directory_children(&item.path);
            for child in children {
                self.render_tree_item(ui, &child, indent_level + 1);
            }
        }
    }
    
    fn is_search_match(&self, item: &FileItem) -> bool {
        let filename_match = if !self.search_query.is_empty() {
            let file_name = item.path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            
            if self.case_sensitive {
                file_name.contains(&self.search_query)
            } else {
                let query_lower = self.search_query.to_lowercase();
                file_name.to_lowercase().contains(&query_lower)
            }
        } else {
            false
        };
        
        let content_match = if !self.search_in_files_query.is_empty() && !item.is_directory {
            !self.search_query.is_empty() || self.is_searching_content
        } else {
            false
        };
        
        filename_match || content_match
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
            (false, false)
        } else if selected_count == children.len() {
            (true, false)
        } else {
            (true, true)
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
            for file_path in files_in_dir {
                self.selected_files.remove(&file_path);
            }
        } else {
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

    fn render_file_preview_with_highlighting(&mut self, ui: &mut egui::Ui) {
        let is_preview_active = self.generated_content.is_some() || self.preview_file.is_some();

        ui.horizontal(|ui| {
            let heading = if self.generated_content.is_some() {
                "Generated Preview"
            } else {
                "Preview"
            };
            ui.heading(heading);
            
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_preview_active {
                    if ui.button("❌ Clear Preview").on_hover_text("Clear the preview area").clicked() {
                        self.generated_content = None;
                        self.preview_file = None;
                        self.preview_content.clear();
                        self.highlighted_preview_content.clear();
                    }
                }
            });
        });
        ui.separator();

        if let Some(generated_content) = &self.generated_content {
            let line_count = generated_content.lines().count();
            let file_size = generated_content.len() as u64;

            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} lines", format_number_with_separators(line_count))).color(ui.visuals().text_color()));
                ui.label("•");
                ui.label(egui::RichText::new(format_file_size(file_size)).color(ui.visuals().text_color()));
                
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("📋 Copy to Clipboard").clicked() {
                        ui.output_mut(|o| o.copied_text = generated_content.clone());
                    }
                });
            });
            ui.add_space(5.0);
            
            let lines: Vec<&str> = generated_content.lines().collect();
            let num_rows = lines.len();
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);

            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .id_salt("virtual_preview_scroll")
                .show_rows(ui, row_height, num_rows, |ui, row_range| {
                    for i in row_range {
                        if let Some(line) = lines.get(i) {
                            ui.horizontal(|ui| {
                                let line_number_text = format!("{:<5}", i + 1);
                                // KORREKTUR: Nutze die semantisch korrekte Farbe für schwachen Text
                                let dim_color = ui.visuals().weak_text_color();
                                ui.monospace(egui::RichText::new(line_number_text).color(dim_color));
                                ui.monospace(*line);
                            });
                        }
                    }
                });

        } else if let Some(preview_file) = &self.preview_file {
            ui.label(format!("📄 {}", preview_file.file_name().unwrap_or_default().to_string_lossy()));
            ui.separator();
            
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(false)
                .id_salt("preview_scroll")
                .show(ui, |ui| {
                    if !self.search_in_files_query.is_empty() && !self.highlighted_preview_content.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            for segment in &self.highlighted_preview_content {
                                if segment.is_match { ui.colored_label(egui::Color32::YELLOW, &segment.text); } else { ui.label(&segment.text); }
                            }
                        });
                    } else {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.preview_content).code_editor().desired_width(f32::INFINITY).desired_rows(50).interactive(false)
                        );
                    }
                });
        } else {
            ui.centered_and_justified(|ui| {
                ui.label("Select a file or generate a preview.");
            });
        }
    }
    
    fn update_preview_highlighting(&mut self) {
        if self.search_in_files_query.is_empty() || self.preview_content.is_empty() {
            self.highlighted_preview_content.clear();
            return;
        }
        
        let search_term = if self.case_sensitive {
            self.search_in_files_query.clone()
        } else {
            self.search_in_files_query.to_lowercase()
        };
        
        let content = if self.case_sensitive {
            self.preview_content.clone()
        } else {
            self.preview_content.to_lowercase()
        };
        
        let mut segments = Vec::new();
        let mut last_end = 0;
        
        for match_start in content.match_indices(&search_term).map(|(i, _)| i) {
            if match_start > last_end {
                segments.push(PreviewSegment {
                    text: self.preview_content[last_end..match_start].to_string(),
                    is_match: false,
                });
            }
            
            let match_end = match_start + search_term.len();
            segments.push(PreviewSegment {
                text: self.preview_content[match_start..match_end].to_string(),
                is_match: true,
            });
            
            last_end = match_end;
        }
        
        if last_end < self.preview_content.len() {
            segments.push(PreviewSegment {
                text: self.preview_content[last_end..].to_string(),
                is_match: false,
            });
        }
        
        self.highlighted_preview_content = segments;
    }

    fn render_bottom_panel(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.heading("📤 Output Settings");
            ui.separator();
            
            ui.horizontal(|ui| {
                ui.label("Output Directory:");
                ui.add(egui::TextEdit::singleline(&mut self.output_path).desired_width(250.0));
                if ui.button("Browse").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.output_path = path.to_string_lossy().to_string();
                    }
                }
                ui.separator();
                ui.label("Filename:");
                ui.add(egui::TextEdit::singleline(&mut self.output_filename).desired_width(250.0));
            });
            
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.include_tree, "Include directory tree");
                if self.include_tree {
                    ui.checkbox(&mut self.tree_full_mode, "Full tree mode");
                }
            });

            if self.include_tree && self.tree_full_mode {
                ui.horizontal(|ui| {
                    ui.label("Tree ignore patterns:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_tree_pattern)
                            .hint_text("Add pattern...")
                            .desired_width(150.0)
                    );
                    if ui.button("Add").clicked() && !self.new_tree_pattern.is_empty() {
                        self.tree_ignore_patterns.insert(self.new_tree_pattern.clone());
                        self.new_tree_pattern.clear();
                    }
                });
        
                if !self.tree_ignore_patterns.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        let patterns: Vec<String> = self.tree_ignore_patterns.iter().cloned().collect();
                        for pattern in patterns {
                            ui.label(
                                egui::RichText::new(format!(" {} ", &pattern))
                                .background_color(ui.visuals().widgets.inactive.bg_fill)
                                .monospace()
                            );
                            if ui.small_button("❌").on_hover_text("Remove").clicked() {
                                self.tree_ignore_patterns.remove(&pattern);
                            }
                        }
                    });
                }
            }
            
            ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                // Nimm den restlichen Platz ein, um die Buttons vertikal zu zentrieren
                ui.add_space(ui.available_height() / 2.0 - ui.spacing().interact_size.y / 2.0);

                ui.horizontal(|ui| {
                    let button_width = 160.0;
                    
                    let can_generate = !self.selected_files.is_empty() && !self.is_scanning && !self.is_generating;
                    let generate_button = egui::Button::new("🚀 Generate Preview").min_size(egui::Vec2::new(button_width, 30.0));
                    if ui.add_enabled(can_generate, generate_button).clicked() {
                        self.generate_preview();
                    }

                    ui.add_space(20.0);

                    let can_save = self.generated_content.is_some() && !self.is_generating;
                    let save_button = egui::Button::new("💾 Save to File").min_size(egui::Vec2::new(button_width, 30.0));
                    let save_response = ui.add_enabled(can_save, save_button);

                    if save_response.clicked() {
                        self.save_generated_file();
                    }
                    if save_response.hovered() && !can_save && can_generate {
                         egui::show_tooltip_at_pointer(
                            ui.ctx(),
                            egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("save_tooltip_layer")),
                            egui::Id::new("save_tooltip"),
                            |ui| {
                                ui.label("Generate a preview first to enable saving.");
                            }
                        );
                    }
                });
            });
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

fn format_number_with_separators(number: usize) -> String {
    let number_str = number.to_string();
    let chars: Vec<char> = number_str.chars().collect();
    let mut result = String::new();
    
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && (chars.len() - i) % 3 == 0 {
            result.push('\'');
        }
        result.push(ch);
    }
    
    result
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
        self.selected_files.clear();
        
        self.expanded_dirs.clear();
        self.preview_file = None;
        self.preview_content.clear();
        self.highlighted_preview_content.clear();
        self.generated_content = None;
        
        self.search_query.clear();
        self.file_extension_filter.clear();
        self.search_in_files_query.clear();
        self.is_searching_content = false;
        
        self.large_files_count = 0;
        self.large_files_names.clear();
        self.show_large_files_warning = false;
        
        self.progress_receiver = None;
        self.file_receiver = None;
        self.content_search_receiver = None;
        self.generation_receiver = None;
        
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (file_sender, file_receiver) = mpsc::unbounded_channel::<(Vec<FileItem>, usize, Vec<String>)>();

        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        self.file_receiver = Some(Arc::new(tokio::sync::Mutex::new(file_receiver)));

        let path = PathBuf::from(&self.current_path);
        let ignore_patterns = self.config.ignore_patterns.clone();

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let scanner = DirectoryScanner::new(ignore_patterns);
                
                let progress_sender_clone = progress_sender.clone();
                
                match scanner.scan_directory(&path, progress_sender).await {
                    Ok((files, large_files_count, large_files_names)) => {
                        let _ = file_sender.send((files, large_files_count, large_files_names));
                        tracing::info!("Scan completed with {} large files skipped", large_files_count);
                    }
                    Err(e) => {
                        tracing::error!("Scan failed: {}", e);
                        let _ = progress_sender_clone.send(ScanProgress {
                            current_file: path.clone(),
                            processed: 0,
                            total: 0,
                            status: format!("Error: {}", e),
                            file_size: None,
                            line_count: None,
                        });
                    }
                }
            });
        });
    }

    pub fn update_progress(&mut self) {
        // --- Phase 1: Daten aus den Kanälen sammeln, ohne `self` zu verändern ---
        let mut progress_update = None;
        let mut scan_result = None;
        let mut content_search_results = None;
        let mut generation_result = None;

        // Fortschritts-Nachrichten
        if let Some(receiver) = &self.progress_receiver {
            if let Ok(mut rx) = receiver.try_lock() {
                let mut latest_progress = None;
                while let Ok(progress) = rx.try_recv() {
                    latest_progress = Some(progress);
                }
                progress_update = latest_progress;
            }
        }

        // Scan-Ergebnis (Dateiliste)
        if self.is_scanning {
            if let Some(file_receiver) = &self.file_receiver {
                if let Ok(mut rx) = file_receiver.try_lock() {
                    if let Ok(data) = rx.try_recv() {
                        scan_result = Some(data);
                    }
                }
            }
        }

        // Andere Ergebnisse
        if let Some(content_receiver) = &self.content_search_receiver {
            if let Ok(mut rx) = content_receiver.try_lock() {
                if let Ok(results) = rx.try_recv() {
                    content_search_results = Some(results);
                }
            }
        }

        if let Some(receiver) = &self.generation_receiver {
            if let Ok(mut rx) = receiver.try_lock() {
                if let Ok(result) = rx.try_recv() {
                    generation_result = Some(result);
                }
            }
        }
        
        // --- Phase 2: `self` mit den gesammelten Daten sicher verändern ---

        if let Some(progress) = progress_update {
            self.scan_progress = Some(progress);
        }

        if let Some((files, large_files_count, large_files_names)) = scan_result {
            self.file_tree = files;
            if large_files_count > 0 {
                self.large_files_count = large_files_count;
                self.large_files_names = large_files_names;
                self.show_large_files_warning = true;
            }
            self.is_scanning = false;
            self.apply_filters();
        }
        
        if let Some(result) = generation_result {
            self.is_generating = false;
            match result {
                Ok((content, _size, _lines)) => {
                    self.generated_content = Some(content);
                    self.preview_file = None;
                    self.preview_content.clear();
                    self.highlighted_preview_content.clear();
                    tracing::info!("Generated content is ready for preview.");
                }
                Err(e) => {
                    tracing::error!("Content generation failed: {}", e);
                }
            }
        }
        
        if let Some(search_results) = content_search_results {
            self.is_searching_content = false;
            let mut results_with_parents = search_results.clone();
            let root_path = PathBuf::from(&self.current_path);
            
            for item in &search_results {
                let mut current = item.path.parent();
                while let Some(parent) = current {
                    if parent >= root_path {
                        if let Some(dir_item) = self.file_tree.iter().find(|i| i.path == parent && i.is_directory) {
                            if !results_with_parents.iter().any(|r| r.path == parent) {
                                results_with_parents.push(dir_item.clone());
                            }
                        }
                    }
                    current = parent.parent();
                }
            }
            
            self.filtered_files = results_with_parents;
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
        
        let mut filtered = SearchEngine::filter_files(&self.file_tree, &filter);
        
        let mut additional_dirs = HashSet::new();
        let root_path = PathBuf::from(&self.current_path);
        
        for item in &filtered {
            if !item.is_directory {
                let mut current = item.path.parent();
                while let Some(parent) = current {
                    if parent >= root_path && !additional_dirs.contains(parent) {
                        additional_dirs.insert(parent.to_path_buf());
                    }
                    current = parent.parent();
                }
            }
        }
        
        for dir_path in additional_dirs {
            if let Some(dir_item) = self.file_tree.iter().find(|item| item.path == dir_path && item.is_directory) {
                if !filtered.iter().any(|item| item.path == dir_path) {
                    filtered.push(dir_item.clone());
                }
            }
        }
        
        if self.config.remove_empty_directories {
            filtered = self.remove_empty_directories(filtered);
        }
        
        self.filtered_files = filtered;

        // --- NEU: Bereinige die Auswahl ---
        // Erstelle ein Set der sichtbaren Dateien für schnellen Zugriff
        let visible_files: HashSet<_> = self.filtered_files.iter()
            .filter(|f| !f.is_directory)
            .map(|f| &f.path)
            .collect();
        
        // Behalte nur die ausgewählten Dateien, die auch sichtbar sind
        self.selected_files.retain(|path| visible_files.contains(path));
    }

    fn remove_empty_directories(&self, mut files: Vec<FileItem>) -> Vec<FileItem> {
        let mut has_changes = true;
        
        while has_changes {
            has_changes = false;
            let files_before_len = files.len();
            
            let mut dirs_to_remove = Vec::new();
            
            for item in &files {
                if item.is_directory {
                    let has_files = files.iter().any(|other| {
                        !other.is_directory && other.path.starts_with(&item.path) && other.path != item.path
                    });
                    
                    if !has_files {
                        dirs_to_remove.push(item.path.clone());
                    }
                }
            }
            
            files.retain(|item| {
                !dirs_to_remove.contains(&item.path)
            });
            
            if files.len() != files_before_len {
                has_changes = true;
            }
        }
        
        files
    }
    
    pub fn select_all_files(&mut self) {
        for file in &self.filtered_files {
            if !file.is_directory {
                self.selected_files.insert(file.path.clone());
            }
        }
    }
    
    pub fn load_file_preview(&mut self, file_path: &PathBuf) {
        self.generated_content = None;

        self.preview_file = Some(file_path.clone());
        
        match FileHandler::get_file_preview(file_path, 1000) {
            Ok(content) => {
                self.preview_content = content;
                self.update_preview_highlighting();
            }
            Err(e) => {
                self.preview_content = format!("Error loading preview: {}", e);
                self.highlighted_preview_content.clear();
            }
        }
    }
    
    pub fn save_config_dialog(&mut self) {
        if let Some(file) = rfd::FileDialog::new()
            .add_filter("JSON Config", &["json"])
            .set_file_name("context-file-concat-config.json")
            .save_file()
        {
            match crate::config::settings::export_config(&self.config, &file) {
                Ok(_) => {
                    tracing::info!("Config exported to {:?}", file);
                }
                Err(e) => {
                    tracing::error!("Failed to export config: {}", e);
                }
            }
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
                    self.apply_filters();
                    tracing::info!("Config loaded from {:?}", file);
                }
                Err(e) => {
                    tracing::error!("Failed to load config: {}", e);
                }
            }
        }
    }
    
    pub fn generate_preview(&mut self) {
        if self.selected_files.is_empty() || self.is_generating || self.is_scanning {
            return;
        }

        self.is_generating = true;
        self.scan_progress = None;
        self.generated_content = None;
        self.preview_file = None;

        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (result_sender, result_receiver) = mpsc::unbounded_channel();

        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        self.generation_receiver = Some(Arc::new(tokio::sync::Mutex::new(result_receiver)));

        let selected_files: Vec<PathBuf> = self.selected_files.iter().cloned().collect();
        let include_tree = self.include_tree;
        let tree_content = if include_tree {
            let root_path = PathBuf::from(&self.current_path);
            
            let items_for_tree: Vec<_> = if self.tree_full_mode {
                self.file_tree.clone()
            } else {
                self.file_tree.iter()
                    .filter(|item| {
                        if item.is_directory {
                            selected_files.iter().any(|f| f.starts_with(&item.path))
                        } else {
                            selected_files.contains(&item.path)
                        }
                    })
                    .cloned()
                    .collect()
            };
                
            let ignore_patterns = if self.tree_full_mode { self.tree_ignore_patterns.clone() } else { HashSet::new() };

            Some(TreeGenerator::generate_tree(&items_for_tree, &root_path, &ignore_patterns))
        } else {
            None
        };

        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let result = FileHandler::generate_concatenated_content(
                    &selected_files,
                    include_tree,
                    tree_content,
                    progress_sender,
                ).await;

                match result {
                    Ok((content, size, lines)) => {
                        let _ = result_sender.send(Ok((content, size, lines)));
                    }
                    Err(e) => {
                        let _ = result_sender.send(Err(e.to_string()));
                    }
                }
            });
        });
    }

    pub fn save_generated_file(&mut self) {
        if let Some(content) = &self.generated_content {
            let output_dir = PathBuf::from(&self.output_path);
            if !output_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&output_dir) {
                    tracing::error!("Failed to create output directory '{:?}': {}", output_dir, e);
                    return;
                }
            }
            let file_path = output_dir.join(&self.output_filename);

            match std::fs::write(&file_path, content) {
                Ok(_) => {
                    tracing::info!("Successfully saved generated file to: {}", file_path.display());
                    self.open_output_in_finder();
                }
                Err(e) => {
                    tracing::error!("Failed to save generated file: {}", e);
                }
            }
        }
    }
    
    pub fn start_content_search(&mut self) {
        if self.search_in_files_query.is_empty() || self.is_searching_content {
            return;
        }
        
        self.is_searching_content = true;
        
        let search_query = self.search_in_files_query.clone();
        let files_to_search: Vec<FileItem> = self.file_tree.iter()
            .filter(|item| !item.is_directory && !item.is_binary)
            .cloned()
            .collect();
        
        let (result_sender, result_receiver) = mpsc::unbounded_channel();
        self.content_search_receiver = Some(Arc::new(tokio::sync::Mutex::new(result_receiver)));
        
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut matching_files = Vec::new();
                
                for file_item in files_to_search {
                    if let Ok(content) = std::fs::read_to_string(&file_item.path) {
                        if content.to_lowercase().contains(&search_query.to_lowercase()) {
                            matching_files.push(file_item);
                        }
                    }
                    
                    if matching_files.len() % 50 == 0 {
                        tokio::task::yield_now().await;
                    }
                }
                
                let _ = result_sender.send(matching_files);
            });
        });
    }


    pub fn render_progress_overlay(&mut self, ctx: &egui::Context) {
        if let Some(progress) = &self.scan_progress.clone() {
            let is_complete = progress.processed >= progress.total && progress.total > 0;

            let (title, complete_title) = if self.is_generating {
                ("⏳ Generating Preview...", "✅ Preview Ready!")
            } else {
                ("⏳ Scanning...", "✅ Scan Complete!")
            };

            egui::Window::new(if is_complete { complete_title } else { title })
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        // KORREKTUR: Die redundante Überschrift wird entfernt. Der Fenstertitel reicht aus.
                        // ui.heading(if is_complete { complete_title } else { title });

                        // --- KORREKTUR FORTSCHRITTSBALKEN ---
                        let progress_fraction = if progress.total > 0 {
                            progress.processed as f32 / progress.total as f32
                        } else {
                            // Erzeuge einen unbestimmten, animierten Ladebalken
                            (ui.ctx().input(|i| i.time) * 2.0).sin() as f32 * 0.5 + 0.5
                        };
                        
                        let progress_text = if progress.total > 0 {
                            format!("{}/{}", progress.processed, progress.total)
                        } else {
                            format!("{} items found", progress.processed)
                        };

                        ui.add(egui::ProgressBar::new(progress_fraction).text(progress_text));
                        // --- ENDE KORREKTUR ---

                        // Dies zeigt den detaillierten Status wie "Loading Data..." oder "Scanning..."
                        ui.label(&progress.status);

                        if let Some(file_name) = progress.current_file.file_name() {
                            if !file_name.is_empty() {
                                ui.label(format!("File: {}", file_name.to_string_lossy()));
                            }
                        }

                        if is_complete {
                            if let Some(size) = progress.file_size {
                                ui.label(format!("Total Size: {}", format_file_size(size)));
                            }
                            if let Some(lines) = progress.line_count {
                                ui.label(format!("Total Lines: {}", format_number_with_separators(lines)));
                            }
                        }
                        
                        ui.separator();
                        
                        ui.horizontal(|ui| {
                            if is_complete {
                                if ui.button("✅ Close").clicked() {
                                    self.is_scanning = false;
                                    self.is_generating = false;
                                    self.scan_progress = None;
                                }
                            } else {
                                if ui.button("❌ Cancel").clicked() {
                                    self.is_scanning = false;
                                    self.is_generating = false;
                                    self.scan_progress = None;
                                }
                            }
                        });
                    });
                });
        }
    }
        
    pub fn render_large_files_warning(&mut self, ctx: &egui::Context) {
        egui::Window::new("⚠️ Large Files Detected")
            .collapsible(false)
            .resizable(true)
            .default_width(500.0)
            .default_height(300.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.heading("⚠️ Large Files Skipped");
                    
                    ui.add_space(10.0);
                    
                    ui.label(format!("{} files were skipped because they exceed the 20MB limit.", self.large_files_count));
                    ui.label("These files are not included in the scan for performance reasons.");
                    
                    ui.add_space(10.0);
                    
                    ui.label("Skipped files:");
                    egui::ScrollArea::vertical()
                        .max_height(150.0)
                        .show(ui, |ui| {
                            for file_name in &self.large_files_names {
                                ui.label(format!("• {}", file_name));
                            }
                        });
                    
                    ui.add_space(10.0);
                    
                    ui.horizontal(|ui| {
                        if ui.button("✅ OK").clicked() {
                            self.show_large_files_warning = false;
                            self.large_files_count = 0;
                            self.large_files_names.clear();
                        }
                        
                        if ui.button("📋 Show in Logs").clicked() {
                            tracing::warn!("=== {} Large Files (>20MB) Skipped ===", self.large_files_count);
                            for file_name in &self.large_files_names {
                                tracing::warn!("Skipped: {}", file_name);
                            }
                            tracing::warn!("=== End Large Files List ===");
                            
                            self.show_large_files_warning = false;
                            self.large_files_count = 0;
                            self.large_files_names.clear();
                        }
                    });
                });
            });
    }

    fn open_output_in_finder(&self) {
        let output_path = PathBuf::from(&self.output_path);
        
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg(&output_path)
                .spawn();
        }
        
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer")
                .arg(&output_path)
                .spawn();
        }
        
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(&output_path)
                .spawn();
        }
    }
}