use eframe::egui;
use std::path::{Path, PathBuf};
use std::collections::HashSet;
use tokio::sync::mpsc;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use rayon::prelude::*;
use regex::Regex;

use super::{ContextFileConcatApp, PreviewSegment};
// BEREINIGTE IMPORTS: Path und build_globset_from_patterns sind nicht mehr nötig
use crate::core::{DirectoryScanner, FileHandler, SearchEngine, SearchFilter, FileItem, ScanProgress};
use crate::utils::file_detection::is_image_file;

impl ContextFileConcatApp {
    pub fn render_main_ui(&mut self, _ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar")
            .default_height(60.0)
            .show(ctx, |ui| {
                self.render_toolbar(ui);
            });
        
        // ÄNDERUNG HIER: Die Zeile .min_height(150.0) wurde entfernt.
        egui::TopBottomPanel::bottom("output_panel")
            .show(ctx, |ui| {
                // Diese Methode bleibt unverändert.
                self.render_bottom_panel(ui);
            });
        
        egui::SidePanel::left("left_panel")
            .default_width(300.0)
            .min_width(250.0)
            .show(ctx, |ui| {
                self.render_left_panel(ui);
            });
            
        egui::CentralPanel::default().show(ctx, |ui| {
            self.render_right_panel_fixed(ui);
        });
        
        if self.is_scanning || self.is_generating {
            self.render_progress_overlay(ctx);
        }

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
        let response = ui.add(
            egui::TextEdit::singleline(&mut self.current_path)
                .desired_width(400.0)
                .hint_text("Enter directory path or use Select Directory button")
        );
        if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            if !self.current_path.is_empty() {
                self.start_directory_scan();
            }
        }
        
        ui.add_space(1.0);
        if ui.button("Scan").clicked() && !self.current_path.is_empty() {
            self.start_directory_scan();
        }
        
        ui.separator();
        
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
        ui.add_space(1.0);
        ui.vertical(|ui| {
            ui.heading("🔍 Search & Filter");
            ui.horizontal(|ui| { ui.label("Search:"); if ui.add(egui::TextEdit::singleline(&mut self.search_query).hint_text("Search in filenames...")).changed() { self.apply_filters(); } });
            ui.horizontal(|ui| { ui.label("Extension:"); if ui.add(egui::TextEdit::singleline(&mut self.file_extension_filter).hint_text("e.g., .rs, .js, .py")).changed() { self.apply_filters(); } });
            ui.separator();
            ui.heading("🔍 Search in Files");
            ui.horizontal(|ui| {
                ui.label("Content:");
                if ui.add(egui::TextEdit::singleline(&mut self.search_in_files_query).hint_text("Search text inside files...")).changed() {
                    if !self.search_in_files_query.is_empty() { self.start_content_search(); } else { self.apply_filters(); }
                    self.update_preview_highlighting();
                }
            });
            if self.is_searching_content { ui.horizontal(|ui| { ui.spinner(); ui.label("Searching in files..."); }); }
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.case_sensitive, "Case sensitive").changed() { self.apply_filters(); self.update_preview_highlighting(); }
                if ui.checkbox(&mut self.show_binary_files, "Show binary files").changed() { self.apply_filters(); }
            });
            ui.separator();
            ui.heading("🚫 Ignore Patterns");
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.config.remove_empty_directories, "Remove empty dirs").changed() { self.apply_filters(); }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add_enabled(!self.current_path.is_empty() && !self.is_scanning, egui::Button::new("🔄 Re-Scan Changes")).clicked() { self.start_directory_scan(); }
                });
            });
            ui.label("Common Ignore Pattern:");
            ui.horizontal_wrapped(|ui| {
                for pattern in ["node_modules/", "target/", ".git/", "*.log", "*.lock", "*.tmp", ".DS_Store", "Thumbs.db", "*.class", "package-lock.json", ".psd", "*.jpg", "*.svg", "*.png", "*.webp", "*.avif", "*.gif", "*.tiff", "*.raw", "*.avif"] {
                    if ui.small_button(pattern).clicked() { 
                        self.config.ignore_patterns.insert(pattern.to_string());
                        self.apply_filters();
                    }
                }
            });

            ui.horizontal(|ui| {
                let text_edit_response = ui.add(
                    egui::TextEdit::singleline(&mut self.new_ignore_pattern).hint_text("Add Ignore Pattern...")
                );
                let submitted = text_edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Add").clicked() || submitted {
                    if !self.new_ignore_pattern.is_empty() {
                        self.config.ignore_patterns.insert(self.new_ignore_pattern.clone());
                        self.new_ignore_pattern.clear();
                        self.apply_filters();
                        text_edit_response.request_focus();
                    }
                }
                if ui.button("Delete All").on_hover_text("Remove all ignore patterns").clicked() {
                    self.config.ignore_patterns.clear();
                    self.apply_filters();
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Filter list:");
                ui.add(egui::TextEdit::singleline(&mut self.ignore_pattern_filter).hint_text("Filter displayed patterns..."));
            });
            ui.collapsing("Current ignore patterns", |ui| {
                egui::ScrollArea::vertical().max_height((ui.available_height() - 20.0).max(50.0)).auto_shrink([false, false]).show(ui, |ui| {
                    let mut patterns: Vec<String> = self.config.ignore_patterns.iter().cloned().collect();
                    patterns.sort_unstable();
                    let filter_text = self.ignore_pattern_filter.to_lowercase();
                    for pattern in &patterns {
                        if !filter_text.is_empty() && !pattern.to_lowercase().contains(&filter_text) { continue; }
                        ui.horizontal(|ui| {
                            if ui.small_button("❌").on_hover_text("Remove pattern").clicked() { 
                                self.config.ignore_patterns.remove(pattern); 
                                self.apply_filters();
                            }
                            ui.label(pattern);
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
        self.file_list_height = self.file_list_height
            .max(min_file_list_height)
            .min((available_height - min_preview_height).max(min_file_list_height));
        ui.vertical(|ui| {
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
            let resizer_height = 8.0; 
            let resizer_response = ui.allocate_response(
                egui::Vec2::new(ui.available_width(), resizer_height),
                egui::Sense::drag(),
            );
            if resizer_response.dragged() {
                self.file_list_height += resizer_response.drag_delta().y;
            }
            if resizer_response.hovered() || resizer_response.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            }
            let stroke = if resizer_response.hovered() || resizer_response.dragged() {
                egui::Stroke::new(2.0, egui::Color32::from_rgb(200, 200, 200))
            } else {
                ui.visuals().widgets.noninteractive.bg_stroke
            };
            ui.painter().line_segment(
                [resizer_response.rect.left_center(), resizer_response.rect.right_center()],
                stroke,
            );
            ui.group(|ui| {
                ui.set_height((ui.available_height() - 1.0).max(50.0));
                self.render_file_preview_with_highlighting(ui);
            });
        });
    }
    
    fn render_file_list(&mut self, ui: &mut egui::Ui) {
        ui.heading("📄 Files");
        ui.horizontal(|ui| {
            if ui.button("Select All").clicked() { self.select_all_files(); }
            if ui.button("Deselect All").clicked() { self.selected_files.clear(); }
            if ui.button("Expand All").clicked() { self.expand_all_directories(); }
            if ui.button("Collapse All").clicked() { self.expanded_dirs.clear(); }
            ui.separator();
            ui.label(format!("{} files found, {} selected", self.filtered_files.len(), self.selected_files.len()));
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
                if ui.add(egui::Button::new(if is_expanded { "🔽" } else { "▶" }).small().frame(false)).clicked() {
                    if is_expanded { self.expanded_dirs.remove(&item.path); } else { self.expanded_dirs.insert(item.path.clone()); }
                }
                
                let mut dir_selected = self.is_directory_selected(&item.path);
                if ui.checkbox(&mut dir_selected.0, "").changed() {
                    self.toggle_directory_selection(&item.path);
                }
                
                let dir_selected = self.is_directory_selected(&item.path).0;
                if dir_selected { ui.label("📁"); } else { ui.colored_label(egui::Color32::from_gray(120), "📁"); }
                
                let dir_name = item.path.file_name().unwrap_or_default().to_string_lossy();
                if is_search_match {
                    ui.colored_label(egui::Color32::YELLOW, format!("🔍 {}", dir_name));
                } else if dir_selected {
                    ui.colored_label(egui::Color32::WHITE, dir_name.as_ref());
                } else {
                    ui.colored_label(egui::Color32::from_gray(160), dir_name.as_ref());
                }

                let ignore_button_response = ui.add(egui::Button::new("i").small().min_size(egui::Vec2::new(16.0, 16.0)).fill(egui::Color32::from_gray(30)));
                if ignore_button_response.clicked() {
                    if let Some(dir_name) = item.path.file_name().and_then(|n| n.to_str()) {
                        self.config.ignore_patterns.insert(format!("{}/", dir_name));
                        self.apply_filters(); // Löst sofortiges Filtern aus, KEINEN Re-Scan
                    }
                }
                ignore_button_response.on_hover_text("Add directory to ignore patterns");

            } else { // Wenn es eine Datei ist
                let mut is_selected = self.selected_files.contains(&item.path);
                if ui.checkbox(&mut is_selected, "").changed() {
                    if is_selected { self.selected_files.insert(item.path.clone()); } else { self.selected_files.remove(&item.path); }
                }
                
                let icon = if is_image_file(&item.path) { "📷" } else if item.is_binary { "🔧" } else { "📄" };
                if is_selected { ui.label(icon); } else { ui.colored_label(egui::Color32::from_gray(120), icon); }
                
                let name = item.path.file_name().unwrap_or_default().to_string_lossy();
                let label_text = if is_search_match { format!("🔍 {}", name) } else { name.to_string() };
                
                let response = if is_search_match {
                    ui.selectable_label(self.preview_file.as_ref() == Some(&item.path), egui::RichText::new(label_text).color(egui::Color32::YELLOW))
                } else if is_selected {
                    ui.selectable_label(self.preview_file.as_ref() == Some(&item.path), egui::RichText::new(label_text).color(egui::Color32::WHITE))
                } else {
                    ui.selectable_label(self.preview_file.as_ref() == Some(&item.path), egui::RichText::new(label_text).color(egui::Color32::from_gray(160)))
                };
                if response.clicked() { self.load_file_preview(&item.path); }
                
                let ignore_button_response = ui.add(egui::Button::new("i").small().min_size(egui::Vec2::new(16.0, 16.0)).fill(egui::Color32::from_gray(30)));
                if ignore_button_response.clicked() {
                    if let Some(file_name) = item.path.file_name().and_then(|n| n.to_str()) {
                        self.config.ignore_patterns.insert(file_name.to_string());
                        self.apply_filters(); // Löst sofortiges Filtern aus, KEINEN Re-Scan
                    }
                }
                ignore_button_response.on_hover_text("Add file to ignore patterns");

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(20.0);
                    if is_selected { ui.label(format_file_size(item.size)); } else { ui.colored_label(egui::Color32::from_gray(120), format_file_size(item.size)); }
                });
            }
        });
        
        if item.is_directory && self.expanded_dirs.contains(&item.path) {
            let children = self.get_directory_children(&item.path);
            for child in children { self.render_tree_item(ui, &child, indent_level + 1); }
        }
    }
    
    fn is_search_match(&self, item: &FileItem) -> bool {
        let filename_match = if !self.search_query.is_empty() {
            let file_name = item.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
            if self.case_sensitive { file_name.contains(&self.search_query) } else { file_name.to_lowercase().contains(&self.search_query.to_lowercase()) }
        } else { false };
        let content_match = if !self.search_in_files_query.is_empty() && !item.is_directory { !self.search_query.is_empty() || self.is_searching_content } else { false };
        filename_match || content_match
    }
    
    fn get_directory_children(&self, dir_path: &PathBuf) -> Vec<FileItem> {
        let mut children = Vec::new();
        for item in &self.filtered_files {
            if let Some(parent) = item.path.parent() {
                if parent == dir_path { children.push(item.clone()); }
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
        if children.is_empty() { return (false, false); }
        let selected_count = children.iter().filter(|path| self.selected_files.contains(*path)).count();
        if selected_count == 0 { (false, false) } else if selected_count == children.len() { (true, false) } else { (true, true) }
    }
    
    fn get_all_files_in_directory(&self, dir_path: &PathBuf) -> Vec<PathBuf> {
        self.filtered_files
            .iter()
            .filter(|item| !item.is_directory && item.path.starts_with(dir_path))
            .map(|item| item.path.clone())
            .collect()
    }
    
    fn toggle_directory_selection(&mut self, dir_path: &PathBuf) {
        let files_in_dir = self.get_all_files_in_directory(dir_path);
        let (is_selected, _) = self.is_directory_selected(dir_path);
        if is_selected {
            for file_path in files_in_dir { self.selected_files.remove(&file_path); }
        } else {
            for file_path in files_in_dir { self.selected_files.insert(file_path); }
        }
    }
    
    fn expand_all_directories(&mut self) {
        for item in &self.filtered_files {
            if item.is_directory { self.expanded_dirs.insert(item.path.clone()); }
        }
    }

    fn render_file_preview_with_highlighting(&mut self, ui: &mut egui::Ui) {
        let is_preview_active = self.generated_content.is_some() || self.preview_file.is_some();
        ui.horizontal(|ui| {
            let heading = if self.generated_content.is_some() { "Generated Preview" } else { "Preview" };
            ui.heading(heading);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if is_preview_active {
                    if ui.button("❌ Clear Preview").on_hover_text("Clear the preview area").clicked() {
                        self.generated_content = None;
                        self.preview_file = None;
                        self.preview_content.clear();
                        self.highlighted_preview_lines.clear();
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
                    if ui.button("📋 Copy to Clipboard").clicked() { ui.output_mut(|o| o.copied_text = generated_content.clone()); }
                });
            });
            ui.add_space(5.0);
            let lines: Vec<&str> = generated_content.lines().collect();
            let num_rows = lines.len();
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("virtual_preview_scroll").show_rows(ui, row_height, num_rows, |ui, row_range| {
                for i in row_range {
                    if let Some(line) = lines.get(i) {
                        ui.horizontal(|ui| {
                            let line_number_text = format!("{:<5}", i + 1);
                            let dim_color = ui.visuals().weak_text_color();
                            ui.monospace(egui::RichText::new(line_number_text).color(dim_color));
                            ui.monospace(*line);
                        });
                    }
                }
            });
        } else if let Some(preview_file) = &self.preview_file {
            let line_count = self.preview_content.lines().count();
            let file_size = self.preview_content.len() as u64;
            ui.horizontal(|ui| {
                ui.label(format!("📄 {}", preview_file.file_name().unwrap_or_default().to_string_lossy()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(format_file_size(file_size)).color(ui.visuals().text_color()));
                    ui.label("•");
                    ui.label(egui::RichText::new(format!("{} lines", format_number_with_separators(line_count))).color(ui.visuals().text_color()));
                });
            });
            ui.separator();
            let lines: Vec<&str> = self.preview_content.lines().collect();
            let num_rows = self.preview_content.lines().count();
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("virtual_single_file_preview_scroll").show_rows(ui, row_height, num_rows, |ui, row_range| {
                let is_highlighting_active = !self.search_in_files_query.is_empty() && !self.highlighted_preview_lines.is_empty();
                for i in row_range {
                    let line_number_text = format!("{:<5}", i + 1);
                    let dim_color = ui.visuals().weak_text_color();
                    ui.horizontal(|ui| {
                        ui.monospace(egui::RichText::new(line_number_text).color(dim_color));
                        if is_highlighting_active {
                            if let Some(segments) = self.highlighted_preview_lines.get(i) {
                                for segment in segments {
                                    if segment.is_match {
                                        let highlight_color = egui::Color32::from_rgb(90, 80, 0);
                                        ui.monospace(egui::RichText::new(&segment.text).background_color(highlight_color).color(egui::Color32::YELLOW));
                                    } else {
                                        ui.monospace(&segment.text);
                                    }
                                }
                            } else if let Some(line) = lines.get(i) {
                                ui.monospace(*line);
                            }
                        } else if let Some(line) = lines.get(i) {
                            ui.monospace(*line);
                        }
                    });
                }
            });
        } else {
            ui.centered_and_justified(|ui| { ui.label("Select a file or generate a preview."); });
        }
    }
    
    fn update_preview_highlighting(&mut self) {
        self.highlighted_preview_lines.clear();
        if self.search_in_files_query.is_empty() || self.preview_content.is_empty() { return; }
        let search_term = if self.case_sensitive { self.search_in_files_query.clone() } else { self.search_in_files_query.to_lowercase() };
        for line in self.preview_content.lines() {
            let mut line_segments = Vec::new();
            let mut last_end = 0;
            let line_for_searching = if self.case_sensitive { line.to_string() } else { line.to_lowercase() };
            if !search_term.is_empty() {
                for (match_start, matched_str) in line_for_searching.match_indices(&search_term) {
                    if match_start > last_end {
                        line_segments.push(PreviewSegment { text: line[last_end..match_start].to_string(), is_match: false });
                    }
                    let match_end = match_start + matched_str.len();
                    line_segments.push(PreviewSegment { text: line[match_start..match_end].to_string(), is_match: true });
                    last_end = match_end;
                }
            }
            if last_end < line.len() {
                line_segments.push(PreviewSegment { text: line[last_end..].to_string(), is_match: false });
            }
            if line.is_empty() {
                 line_segments.push(PreviewSegment { text: String::new(), is_match: false });
            }
            self.highlighted_preview_lines.push(line_segments);
        }
    }    

    fn render_bottom_panel(&mut self, ui: &mut egui::Ui) {
        // Die vertikale Hauptstruktur für den gesamten Footer
        ui.vertical(|ui| {
            // ======================= 1. BUTTONS (OBEN) =======================
            ui.add_space(5.0); // Ein wenig Abstand oben
            
            // Die horizontale Reihe für die zentrierten Buttons
            ui.horizontal(|ui| {
                // Definiere die Breiten für die Berechnung der Zentrierung.
                let button_width = 160.0;
                let button_height = 30.0;
                let space_between = 20.0;
                let total_widgets_width = button_width * 2.0 + space_between;
                
                let available_width = ui.available_width();
                
                // Füge links einen Leerraum ein, der die Buttons in die Mitte schiebt.
                if available_width > total_widgets_width {
                    ui.add_space((available_width - total_widgets_width) / 2.0);
                }

                // Button 1: Generate Preview
                let can_generate = !self.selected_files.is_empty() && !self.is_scanning && !self.is_generating;
                if ui.add_enabled(can_generate, egui::Button::new("🚀 Generate Preview").min_size(egui::vec2(button_width, button_height))).clicked() {
                    self.generate_preview();
                }
                
                ui.add_space(space_between);

                // Button 2: Save to File
                let can_save = self.generated_content.is_some() && !self.is_generating;
                let save_response = ui.add_enabled(can_save, egui::Button::new("💾 Save to File").min_size(egui::vec2(button_width, button_height)));
                if save_response.clicked() {
                    self.save_generated_file();
                }

                // Tooltip-Logik
                if save_response.hovered() && !can_save && can_generate {
                    egui::show_tooltip_at_pointer(ui.ctx(), egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("save_tooltip_layer")), egui::Id::new("save_tooltip"), |ui| {
                        ui.label("Generate a preview first to enable saving.");
                    });
                }
            });
            
            ui.add_space(5.0); // Ein wenig Abstand unter den Buttons
            ui.separator(); // Eine Trennlinie
            
            // ======================= 2. EINSTELLUNGEN (UNTEN, EINKLAPPBAR) =======================
            
            // ANPASSUNG: ui.heading wird durch ui.collapsing ersetzt.
            // Alle Einstellungen werden innerhalb dieses Blocks platziert.
            ui.collapsing("📤 Output Settings", |ui| {
                ui.add_space(5.0); // Ein kleiner Abstand innerhalb der Sektion

                ui.horizontal(|ui| {
                    ui.label("Output Directory:");
                    ui.add(egui::TextEdit::singleline(&mut self.output_path).desired_width(250.0));
                    if ui.button("Browse").clicked() { if let Some(path) = rfd::FileDialog::new().pick_folder() { self.output_path = path.to_string_lossy().to_string(); } }
                });
                ui.horizontal(|ui| {
                    ui.label("Filename:");
                    ui.add(egui::TextEdit::singleline(&mut self.output_filename).desired_width(250.0));
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.include_tree, "Include directory tree");
                    ui.add_space(20.0);
                    ui.checkbox(&mut self.use_relative_paths, "relative file path");
                });
                if self.include_tree {
                    ui.horizontal(|ui| {
                        ui.label("Tree ignore patterns:");
                        let text_edit_response = ui.add(egui::TextEdit::singleline(&mut self.new_tree_pattern).hint_text("Add pattern...").desired_width(150.0));
                        let submitted = text_edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        
                        if ui.button("Add").clicked() || submitted {
                            if !self.new_tree_pattern.is_empty() {
                                self.tree_ignore_patterns.insert(self.new_tree_pattern.clone());
                                self.new_tree_pattern.clear();
                                text_edit_response.request_focus();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Copy Current Ignores").on_hover_text("Copies all patterns from the main list on the left").clicked() { self.tree_ignore_patterns = self.config.ignore_patterns.clone(); }
                        if ui.button("Clear Tree Ignores").on_hover_text("Removes all tree-specific ignore patterns").clicked() { self.tree_ignore_patterns.clear(); }
                    });
                    if !self.tree_ignore_patterns.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            let mut patterns: Vec<String> = self.tree_ignore_patterns.iter().cloned().collect();
                            patterns.sort_unstable();
                            for pattern in patterns {
                                ui.label(egui::RichText::new(format!(" {} ", &pattern)).background_color(ui.visuals().widgets.inactive.bg_fill).monospace());
                                if ui.small_button("❌").on_hover_text("Remove").clicked() { self.tree_ignore_patterns.remove(&pattern); }
                            }
                        });
                    }
                }
            });
            ui.add_space(2.0); // Ein wenig Abstand zum bottom
        });
    }
    
    fn update_output_path_from_root(&mut self) {
        let root = std::path::PathBuf::from(&self.current_path);
        // Setzt den Output-Pfad auf <Root-Verzeichnis>/cfc_output/
        self.output_path = root.join("cfc_output").to_string_lossy().to_string();
    }
}

impl ContextFileConcatApp {

    pub fn start_directory_scan(&mut self) {
        if self.current_path.is_empty() { return; }
        self.update_output_path_from_root();
        if self.is_scanning {
            if let Some(flag) = &self.cancel_flag {
                flag.store(true, Ordering::Relaxed);
                tracing::info!("Requested cancellation of previous scan task.");
            }
        }
        self.is_scanning = true;
        self.scan_progress = Some(ScanProgress {
            current_file: PathBuf::from(&self.current_path),
            processed: 0, total: 0,
            status: "Preparing to scan...".to_string(),
            file_size: None, line_count: None,
        });
        self.selected_files.clear();
        // self.expanded_dirs.clear(); // ENTFERNT
        self.preview_file = None;
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.cancel_flag = Some(cancel_flag.clone());
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (file_sender, file_receiver) = mpsc::unbounded_channel();
        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        self.file_receiver = Some(Arc::new(tokio::sync::Mutex::new(file_receiver)));
        let path = PathBuf::from(&self.current_path);
        let ignore_patterns = self.config.ignore_patterns.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let scanner = DirectoryScanner::new(ignore_patterns);
                match scanner.scan_directory(&path, progress_sender, cancel_flag).await {
                    Ok((all_files, large_files_count, large_files_names)) => {
                        tracing::info!("Scan complete. Found {} items. Sending to UI thread.", all_files.len());
                        let _ = file_sender.send((all_files, large_files_count, large_files_names));
                    }
                    Err(e) => { tracing::warn!("Scan process ended: {}", e); }
                }
            });
        });
    }

    pub fn update_progress(&mut self) {
        let mut progress_update = None;
        let mut scan_result = None;
        let mut content_search_results = None;
        let mut generation_result = None;
        if let Some(receiver) = &self.progress_receiver {
            if let Ok(mut rx) = receiver.try_lock() {
                let mut latest_progress = None;
                while let Ok(progress) = rx.try_recv() { latest_progress = Some(progress); }
                progress_update = latest_progress;
            }
        }
        if self.is_scanning {
            if let Some(file_receiver) = &self.file_receiver {
                if let Ok(mut rx) = file_receiver.try_lock() {
                    if let Ok(data) = rx.try_recv() { scan_result = Some(data); }
                }
            }
        }
        if let Some(content_receiver) = &self.content_search_receiver {
            if let Ok(mut rx) = content_receiver.try_lock() {
                if let Ok(results) = rx.try_recv() { content_search_results = Some(results); }
            }
        }
        if let Some(receiver) = &self.generation_receiver {
            if let Ok(mut rx) = receiver.try_lock() {
                if let Ok(result) = rx.try_recv() { generation_result = Some(result); }
            }
        }
        if let Some(progress) = progress_update { self.scan_progress = Some(progress); }
        if let Some((all_files, large_files_count, large_files_names)) = scan_result {
            tracing::info!("UI thread received {} items from scanner.", all_files.len());
            self.file_tree = all_files;
            self.apply_filters();
            if large_files_count > 0 {
                self.large_files_count = large_files_count;
                self.large_files_names = large_files_names;
                self.show_large_files_warning = true;
            }
            self.is_scanning = false;
            tracing::info!("UI state updated. Filtered list contains {} items.", self.filtered_files.len());
        }
        if let Some(result) = generation_result {
            self.is_generating = false;
            match result {
                Ok((content, _size, _lines)) => {
                    self.generated_content = Some(content);
                    self.preview_file = None;
                    self.preview_content.clear();
                    self.highlighted_preview_lines.clear();
                    tracing::info!("Generated content is ready for preview.");
                }
                Err(e) => { tracing::error!("Content generation failed: {}", e); }
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
            ignore_patterns: self.config.ignore_patterns.clone(), // WIEDER HINZUGEFÜGT
        };
        let mut filtered = SearchEngine::filter_files(&self.file_tree, &filter);
        let root_path = PathBuf::from(&self.current_path);
        let required_dirs: HashSet<PathBuf> = filtered.par_iter().flat_map(|item| {
            let mut parents = Vec::new();
            if !item.is_directory {
                let mut current = item.path.parent();
                while let Some(parent) = current {
                    if parent.starts_with(&root_path) && parent != &root_path { parents.push(parent.to_path_buf()); } else { break; }
                    current = parent.parent();
                }
            }
            parents
        }).collect();
        let existing_paths: HashSet<PathBuf> = filtered.par_iter().map(|item| item.path.clone()).collect();
        filtered.extend(
            self.file_tree.par_iter()
                .filter(|item| item.is_directory && required_dirs.contains(&item.path) && !existing_paths.contains(&item.path))
                .cloned().collect::<Vec<FileItem>>()
        );
        if self.config.remove_empty_directories { filtered = SearchEngine::remove_empty_directories(filtered); }
        self.filtered_files = filtered;
        let valid_paths: HashSet<PathBuf> = self.filtered_files.par_iter().map(|item| item.path.clone()).collect();
        self.selected_files.retain(|path| valid_paths.contains(path));
    }
    
    pub fn select_all_files(&mut self) {
        for file in &self.filtered_files {
            if !file.is_directory { self.selected_files.insert(file.path.clone()); }
        }
    }
    
    pub fn load_file_preview(&mut self, file_path: &PathBuf) {
        self.generated_content = None;
        self.preview_file = Some(file_path.clone());
        match FileHandler::get_file_preview(file_path, 1000) {
            Ok(content) => { self.preview_content = content; self.update_preview_highlighting(); }
            Err(e) => { self.preview_content = format!("Error loading preview: {}", e); self.highlighted_preview_lines.clear(); }
        }
    }
    
    pub fn save_config_dialog(&mut self) {
        if let Some(file) = rfd::FileDialog::new().add_filter("JSON Config", &["json"]).set_file_name("context-file-concat-config.json").save_file() {
            match crate::config::settings::export_config(&self.config, &file) {
                Ok(_) => { tracing::info!("Config exported to {:?}", file); }
                Err(e) => { tracing::error!("Failed to export config: {}", e); }
            }
        }
    }
    
    pub fn load_config_dialog(&mut self) {
        if let Some(file) = rfd::FileDialog::new().add_filter("JSON Config", &["json"]).pick_file() {
            match crate::config::settings::import_config(&file) {
                Ok(config) => {
                    tracing::info!("Config loaded from {:?}, triggering automatic rescan.", file);
                    self.config = config;
                    self.start_directory_scan();
                }
                Err(e) => { tracing::error!("Failed to load config: {}", e); }
            }
        }
    }
    
    pub fn generate_preview(&mut self) {
        if self.selected_files.is_empty() || self.is_generating || self.is_scanning { return; }
        if let Some(flag) = &self.generation_cancel_flag {
            flag.store(true, Ordering::Relaxed);
            tracing::info!("Requested cancellation of previous generation task.");
        }
        self.is_generating = true;
        self.scan_progress = None;
        self.generated_content = None;
        self.preview_file = None;
        let (progress_sender, progress_receiver) = mpsc::unbounded_channel();
        let (result_sender, result_receiver) = mpsc::unbounded_channel();
        self.progress_receiver = Some(Arc::new(tokio::sync::Mutex::new(progress_receiver)));
        self.generation_receiver = Some(Arc::new(tokio::sync::Mutex::new(result_receiver)));
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.generation_cancel_flag = Some(cancel_flag.clone());
        let selected_files: Vec<PathBuf> = self.selected_files.iter().cloned().collect();
        let root_path = PathBuf::from(&self.current_path);
        let include_tree = self.include_tree;
        let use_relative_paths = self.use_relative_paths;
        
        let ignore_set = crate::core::build_globset_from_patterns(&self.config.ignore_patterns);
        let safe_items: Vec<FileItem> = self.file_tree.par_iter().filter(|item| !ignore_set.is_match(&item.path)).cloned().collect();
        
        let items_for_tree: Vec<FileItem> = if self.include_tree {
            safe_items
        } else {
            Vec::new()
        };

        let tree_ignore_patterns = self.tree_ignore_patterns.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let result = FileHandler::generate_concatenated_content(
                    &selected_files,
                    &root_path,
                    use_relative_paths,
                    progress_sender,
                    cancel_flag,
                    include_tree,
                    items_for_tree,
                    tree_ignore_patterns,
                ).await;
                match result {
                    Ok((content, size, lines)) => { let _ = result_sender.send(Ok((content, size, lines))); }
                    Err(e) => { let _ = result_sender.send(Err(e.to_string())); }
                }
            });
        });
    }

    pub fn save_generated_file(&mut self) {
        // ÄNDERUNG HIER: .clone() löst den immutable borrow sofort auf.
        if let Some(content) = self.generated_content.clone() {
            // Dieser Aufruf ist jetzt sicher, da `self` nicht mehr ausgeliehen ist.
            self.update_default_filename_if_needed();

            let output_dir = PathBuf::from(&self.output_path);

            if !output_dir.exists() {
                if let Err(e) = std::fs::create_dir_all(&output_dir) {
                    tracing::error!("Failed to create output directory '{:?}': {}", output_dir, e);
                    return;
                }
            }

            let mut final_path = output_dir.join(&self.output_filename);
            let mut counter = 1;

            while final_path.exists() {
                let stem = Path::new(&self.output_filename)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let extension = Path::new(&self.output_filename)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");

                let new_filename = if extension.is_empty() {
                    format!("{}_({})", stem, counter)
                } else {
                    format!("{}_({}).{}", stem, counter, extension)
                };
                
                final_path = output_dir.join(new_filename);
                counter += 1;
            }
            
            // `content` ist hier die geklonte Variable
            match std::fs::write(&final_path, &content) {
                Ok(_) => {
                    tracing::info!("Successfully saved generated file to: {}", final_path.display());
                    self.open_output_in_finder();
                }
                Err(e) => {
                    tracing::error!("Failed to save generated file: {}", e);
                }
            }
        }
    }

    fn update_default_filename_if_needed(&mut self) {
        if let Ok(default_pattern) = Regex::new(r"^cfc_output_\d{8}_\d{6}\.txt$") {
            // ÄNDERUNG HIER:
            // Prüft jetzt, ob das Feld leer ist ODER dem Standard-Muster entspricht.
            if self.output_filename.is_empty() || default_pattern.is_match(&self.output_filename) {
                let message = if self.output_filename.is_empty() {
                    "Dateiname ist leer. Generiere neuen Namen."
                } else {
                    "Standard-Dateiname erkannt. Zeitstempel wird aktualisiert."
                };
                tracing::info!("{}", message);
                
                self.output_filename = format!(
                    "cfc_output_{}.txt",
                    chrono::Local::now().format("%d%m%Y_%H%M%S")
                );
            } else {
                tracing::info!("Benutzerdefinierter Dateiname erkannt: '{}' wird beibehalten.", self.output_filename);
            }
        }
    }

    pub fn start_content_search(&mut self) {
        if self.search_in_files_query.is_empty() || self.is_searching_content { return; }
        self.is_searching_content = true;
        let search_query = self.search_in_files_query.clone();
        let files_to_search: Vec<FileItem> = self.file_tree.iter().filter(|item| !item.is_directory && !item.is_binary).cloned().collect();
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
                    if matching_files.len() % 50 == 0 { tokio::task::yield_now().await; }
                }
                let _ = result_sender.send(matching_files);
            });
        });
    }

    pub fn render_progress_overlay(&mut self, ctx: &egui::Context) {
        if let Some(progress) = &self.scan_progress.clone() {
            if !self.is_scanning { return; }
            let is_complete = progress.processed >= progress.total && progress.total > 0;
            let title = if self.is_generating { "⏳ Generating Preview..." } else { "⏳ Scanning..." };
            let complete_title = if self.is_generating { "✅ Preview Ready!" } else { "✅ Scan Complete!" };
            egui::Window::new(if is_complete { complete_title } else { title })
                .collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_min_width(300.0);
                      ui.vertical_centered(|ui| {
                        ui.add_space(5.0);
                        if progress.total > 0 && !is_complete {
                            let progress_fraction = progress.processed as f32 / progress.total as f32;
                            let percentage = progress_fraction * 100.0;
                            let progress_text = format!("{:.0}% ({} / {})", percentage, format_number_with_separators(progress.processed), format_number_with_separators(progress.total));
                            ui.add(egui::ProgressBar::new(progress_fraction).text(progress_text));
                        } else if !is_complete {
                            ui.horizontal(|ui| { ui.spinner(); ui.label(&progress.status); });
                        }
                        let show_current_file = !is_complete && progress.status != "Counting files..." && progress.status != "Preparing to scan...";
                        if show_current_file {
                            if let Some(file_name) = progress.current_file.file_name() {
                                if !file_name.is_empty() {
                                    let name = file_name.to_string_lossy();
                                    let truncated_name = if name.len() > 40 { format!("...{}", &name[name.len()-37..]) } else { name.to_string() };
                                    ui.label(format!("File: {}", truncated_name));
                                }
                            }
                        } else if is_complete { ui.label(&progress.status); }
                        if is_complete {
                            if let Some(size) = progress.file_size { ui.label(format!("Total Size: {}", format_file_size(size))); }
                            if let Some(lines) = progress.line_count { ui.label(format!("Total Lines: {}", format_number_with_separators(lines))); }
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(5.0);
                        if is_complete {
                            if ui.button("✅ Close").clicked() {
                                self.is_scanning = false;
                                self.scan_progress = None;
                            }
                        } else {
                            if ui.button("❌ Cancel").clicked() {
                                if let Some(flag) = &self.cancel_flag { flag.store(true, Ordering::Relaxed); }
                                self.is_scanning = false;
                                self.scan_progress = None;
                            }
                        }
                        ui.add_space(5.0);
                    });
                });
        }
    }
        
    pub fn render_large_files_warning(&mut self, ctx: &egui::Context) {
        egui::Window::new("💥 Large Files Detected")
            .collapsible(false).resizable(true).default_width(500.0).default_height(300.0)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    ui.heading("💥 Large Files Skipped");
                    ui.add_space(10.0);
                    ui.label(format!("{} files were skipped because they exceed the 20MB limit.", self.large_files_count));
                    ui.label("These files are not included in the scan for performance reasons.");
                    ui.add_space(10.0);
                    ui.label("Skipped files:");
                    egui::ScrollArea::vertical().max_height(150.0).show(ui, |ui| {
                        for file_name in &self.large_files_names { ui.label(format!("\n{}", file_name)); }
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
                            for file_name in &self.large_files_names { tracing::warn!("Skipped: {}", file_name); }
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
        #[cfg(target_os = "macos")] { let _ = std::process::Command::new("open").arg(&output_path).spawn(); }
        #[cfg(target_os = "windows")] { let _ = std::process::Command::new("explorer").arg(&output_path).spawn(); }
        #[cfg(target_os = "linux")] { let _ = std::process::Command::new("xdg-open").arg(&output_path).spawn(); }
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