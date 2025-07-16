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
        self.render_save_error_popup(ctx);
    }
    
    fn render_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label("📁 Root Directory:");
        
        ui.horizontal(|ui| {
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.current_path)
                    .desired_width(350.0)
                    .hint_text("Enter directory path or use Select Directory button")
            );
            
            if ui.button("Select Directory").clicked() {
                if let Some(path) = rfd::FileDialog::new().pick_folder() {
                    self.current_path = path.to_string_lossy().to_string();
                    self.start_directory_scan();
                }
            }
            
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                if !self.current_path.is_empty() {
                    self.start_directory_scan();
                }
            }
        });
        
        ui.add_space(1.0);
        if ui.button("Scan").clicked() && !self.current_path.is_empty() {
            self.start_directory_scan();
        }
        
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("📂 Import Config").clicked() {
                self.load_config_dialog();
            }
            if ui.button("💾 Export Config").clicked() {
                self.save_config_dialog();
            }
        });
        ui.add_space(1.0);
    }

    fn render_left_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(1.0);
        ui.vertical(|ui| {
            ui.heading("🔍 Search & Filter");
            ui.horizontal(|ui| { 
                if ui.add(egui::TextEdit::singleline(&mut self.search_query).hint_text("Search for filenames...")).changed() {
                    self.apply_filters(); 
                    // *** HINZUFÜGEN: Auto-expand bei Suche ***
                    if !self.search_query.is_empty() {
                        self.expand_all_directories();
                    }
                }
            });
            // Gestrichelte Linie einfügen
            draw_dashed_separator(ui,
                egui::Color32::from_gray(64), // Dunkelgrau
                1.0, // Strichstärke
                4.0, // Strichlänge
                4.0, // Lückenlänge
            );
            ui.horizontal(|ui| { 
                if ui.add(egui::TextEdit::singleline(&mut self.file_extension_filter).hint_text("Search for file extension e.g., .rs, .js, .py")).changed() { 
                    self.apply_filters(); 
                    // *** HINZUFÜGEN: Auto-expand bei Suche ***
                    if !self.file_extension_filter.is_empty() {
                        self.expand_all_directories();
                    }
                } 
            });
            // Gestrichelte Linie einfügen
            draw_dashed_separator(ui,
                egui::Color32::from_gray(64), // Dunkelgrau
                1.0, // Strichstärke
                4.0, // Strichlänge
                4.0, // Lückenlänge
            );
            ui.horizontal(|ui| {
                if ui.add(egui::TextEdit::singleline(&mut self.search_in_files_query).hint_text("Search text inside files...")).changed() {
                    if !self.search_in_files_query.is_empty() { 
                        self.start_content_search(); 
                        // *** HINZUFÜGEN: Auto-expand bei Suche ***
                        self.expand_all_directories();
                    } else { 
                        self.apply_filters(); 
                    }
                    self.update_preview_highlighting();
                }
            });
            if self.is_searching_content { ui.horizontal(|ui| { ui.spinner(); ui.label("Searching in files..."); }); }
            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.case_sensitive, "Case sensitive").changed() { 
                    self.apply_filters(); 
                    self.update_preview_highlighting();
                    // <- HINZUGEFÜGT: Neue Suche wenn Search in Files aktiv ist
                    if !self.search_in_files_query.is_empty() {
                        self.start_content_search();
                    }
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.heading("🚫 Ignore Patterns");
                if ui.add_enabled(!self.current_path.is_empty() && !self.is_scanning, egui::Button::new("🔄 Re-Scan Files")).clicked() { self.start_directory_scan(); }
            });                
            if ui.checkbox(&mut self.config.remove_empty_directories, "Remove empty dirs").changed() { self.apply_filters(); }
            ui.add_space(10.0);
            ui.label("Common Ignore Pattern:");
            ui.horizontal_wrapped(|ui| {
                // Gleiche Abstände wie bei den anderen Pattern-Listen
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                
                for pattern in ["node_modules", "target", ".git", "*.log", "*.lock", "__pycache__", "*.tmp", ".DS_Store", "Thumbs.db", "*.class", "package-lock.json", "*.psd", "*.jpg", "*.svg", "*.png", "*.webp", "*.avif", "*.gif", "*.tiff", "*.raw", "*.avif"] {
                    // NEUE BEDINGUNG: Zeige den Button nur an, wenn das Muster noch nicht in der Liste ist.
                    if !self.config.ignore_patterns.contains(pattern) {
                        // Gleiches Styling wie die anderen Pattern-Buttons, aber ohne ❌
                        let button = egui::Button::new(pattern)
                            .fill(egui::Color32::TRANSPARENT) // Kein Hintergrund
                            .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(100))) // Dünner grauer Rand
                            .min_size(egui::vec2(0.0, 20.0)); // Mindesthöhe für bessere Klickbarkeit
                        
                        if ui.add(button)
                            .on_hover_text(format!("Click to add '{}' to ignore patterns", pattern))
                            .clicked() { 
                            self.config.ignore_patterns.insert(pattern.to_string());
                            self.apply_filters();
                        }
                    }
                }
            });
            ui.add_space(10.0);
            ui.vertical(|ui| {
                let text_edit_response = ui.add(
                    egui::TextEdit::singleline(&mut self.new_ignore_pattern).hint_text("Add Ignore Pattern...")
                );
                let submitted = text_edit_response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.add_space(1.0);
                ui.horizontal(|ui| {
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
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.ignore_pattern_filter).hint_text("Filter currently assigned ignore patterns..."));
            });
            ui.collapsing("Current ignore patterns", |ui| {
                egui::ScrollArea::vertical().max_height((ui.available_height() - 20.0).max(50.0)).auto_shrink([false, false]).show(ui, |ui| {
                    let mut patterns: Vec<String> = self.config.ignore_patterns.iter().cloned().collect();
                    patterns.sort_unstable();
                    let filter_text = self.ignore_pattern_filter.to_lowercase();
                    
                    // Horizontal wrapped Layout für automatischen Zeilenumbruch
                    ui.horizontal_wrapped(|ui| {
                        // Schöne Abstände zwischen den Elementen
                        ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                        
                        let mut pattern_to_remove: Option<String> = None;
                        
                        for pattern in &patterns {
                            if !filter_text.is_empty() && !pattern.to_lowercase().contains(&filter_text) { continue; }
                            
                            // Jedes Pattern als ein Button mit X-Symbol LINKS - bricht automatisch um
                            let button_text = format!("❌ {}", pattern);
                            let button = egui::Button::new(button_text)
                                .fill(egui::Color32::TRANSPARENT) // Kein Hintergrund
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(100))) // Dünner grauer Rand
                                .min_size(egui::vec2(0.0, 20.0)); // Mindesthöhe für bessere Klickbarkeit
                            
                            if ui.add(button)
                                .on_hover_text("Click to remove this pattern")
                                .clicked() {
                                pattern_to_remove = Some(pattern.clone());
                            }
                        }
                        
                        // Pattern entfernen nach der Schleife
                        if let Some(pattern) = pattern_to_remove {
                            self.config.ignore_patterns.remove(&pattern);
                            self.apply_filters();
                        }
                    });
                });
            });
        });
        /// Zeichnet eine horizontale, gestrichelte Linie über die gesamte Breite des aktuellen Bereichs.
        fn draw_dashed_separator(ui: &mut egui::Ui, color: egui::Color32, stroke_width: f32, dash_length: f32, gap_length: f32) {
            let (response, painter) = ui.allocate_painter(
                egui::vec2(ui.available_width(), 2.0),
                egui::Sense::hover(),
            );
            let rect = response.rect;
            let y = rect.center().y;
            let stroke = egui::Stroke::new(stroke_width, color);
            let mut x = rect.left();
            while x < rect.right() {
                painter.line_segment(
                    [egui::Pos2::new(x, y), egui::Pos2::new(x + dash_length, y)],
                    stroke,
                );
                x += dash_length + gap_length;
            }
        }

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
        ui.horizontal(|ui| {
            ui.heading("Files");
            ui.separator();
            if ui.button("Select All").clicked() { self.select_all_files(); }
            if ui.button("Deselect All").clicked() { self.selected_files.clear(); }
            if ui.button("Expand All").clicked() { self.expand_all_directories(); }
            if ui.button("Collapse All").clicked() { self.expanded_dirs.clear(); }
            ui.separator();
            
            // *** GEÄNDERT: Zeige auch versteckte Selections an ***
            let total_files_in_tree = self.file_tree.iter().filter(|item| !item.is_directory).count();
            let total_selected = self.selected_files.len();
            let visible_selected = self.filtered_files.iter()
                .filter(|item| !item.is_directory && self.selected_files.contains(&item.path))
                .count();
            
            ui.label(format!("{} files found", self.filtered_files.len()));
            
            if total_selected > visible_selected {
                ui.label("•");
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("{} selected ({} hidden by filter)", total_selected, total_selected - visible_selected)
                );
            } else {
                ui.label("•");
                ui.label(format!("{} selected", total_selected));
            }
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

                let ignore_button_response = ui.add(egui::Button::new("i").small().min_size(egui::Vec2::new(16.0, 16.0)).fill(egui ::Color32::from_gray(35)));
                if ignore_button_response.clicked() {
                    let root_path = PathBuf::from(&self.current_path);
                    // Erzeuge einen pfad-spezifischen Ignore-Pattern.
                    if let Ok(relative_path) = item.path.strip_prefix(&root_path) {
                        // Pattern mit Suffix '/', um das gesamte Verzeichnis zu ignorieren.
                        let pattern = format!("{}/", relative_path.to_string_lossy());
                        self.config.ignore_patterns.insert(pattern);
                        self.apply_filters(); // Filter sofort anwenden
                    }
                }
                ignore_button_response.on_hover_text("Add this specific directory to ignore patterns");

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
                
                let ignore_button_response = ui.add(egui::Button::new("i").small().min_size(egui::Vec2::new(16.0, 16.0)).fill(egui ::Color32::from_gray(35)));
                if ignore_button_response.clicked() {
                    let root_path = PathBuf::from(&self.current_path);
                    // Erzeuge einen pfad-spezifischen Ignore-Pattern.
                    if let Ok(relative_path) = item.path.strip_prefix(&root_path) {
                        // Pattern ist der exakte relative Pfad der Datei.
                        let pattern = relative_path.to_string_lossy().to_string();
                        self.config.ignore_patterns.insert(pattern);
                        self.apply_filters(); // Filter sofort anwenden
                    }
                }
                ignore_button_response.on_hover_text("Add this specific file to ignore patterns");

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
            let matching_line_numbers: Vec<usize> = if !self.search_in_files_query.is_empty() {
            let search_term = if self.case_sensitive { 
                self.search_in_files_query.clone() 
            } else { 
                self.search_in_files_query.to_lowercase() 
            };
            
            lines.iter().enumerate()
                .filter_map(|(i, line)| {
                    let line_for_search = if self.case_sensitive { 
                        line.to_string() 
                    } else { 
                        line.to_lowercase() 
                    };
                    if line_for_search.contains(&search_term) {
                        Some(i)
                    } else {
                        None
                    }
                })
                .collect()
        } else {
            Vec::new()
        };
            let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
            egui::ScrollArea::vertical().auto_shrink([false, false]).id_salt("virtual_single_file_preview_scroll").show_rows(ui, row_height, num_rows, |ui, row_range| {
                let is_highlighting_active = !self.search_in_files_query.is_empty() && !self.highlighted_preview_lines.is_empty();
                for i in row_range {
                    let line_number_text = format!("{:<5}", i + 1);
                    
                    // <- HINZUGEFÜGT: Spezielle Farbe für Zeilen mit Matches
                    let line_number_color = if matching_line_numbers.contains(&i) {
                        egui::Color32::YELLOW // Gelb für Zeilen mit Matches
                    } else {
                        ui.visuals().weak_text_color() // Normal grau
                    };
                    
                    ui.horizontal(|ui| {
                        ui.monospace(egui::RichText::new(line_number_text).color(line_number_color));
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
                let generate_button = if can_generate {
                    egui::Button::new(egui::RichText::new("🚀 Generate Preview").color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(112, 157, 108)) // #709d6c
                        .min_size(egui::vec2(button_width, button_height))
                } else {
                    egui::Button::new("🚀 Generate Preview")
                        .fill(egui::Color32::from_gray(60)) // Grau wenn disabled
                        .min_size(egui::vec2(button_width, button_height))
                };
                
                let generate_response = ui.add_enabled(can_generate, generate_button);
                
                // Explizites Hover-Styling für besseren Border
                if can_generate && generate_response.hovered() {
                    let hover_stroke = egui::Stroke::new(0.0, egui::Color32::from_rgb(255, 255, 255));
                    ui.painter().rect_stroke(generate_response.rect, 3.0, hover_stroke);
                }
                
                if generate_response.clicked() {
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
            
            ui.add_space(1.0); // Ein wenig Abstand unter den Buttons
            ui.separator(); // Eine Trennlinie
            
            // ======================= 2. EINSTELLUNGEN (UNTEN, EINKLAPPBAR) =======================
            
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
                        ui.add_space(5.0);
                        
                        // LÖSUNG: Jedes Pattern als ein einzelner klickbarer Button für echten Zeilenumbruch
                        ui.horizontal_wrapped(|ui| {
                            // Schöne Abstände zwischen den Elementen
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                            
                            let mut patterns: Vec<String> = self.tree_ignore_patterns.iter().cloned().collect();
                            patterns.sort_unstable();
                            
                            let mut pattern_to_remove: Option<String> = None;

                            for pattern in &patterns {
                                // Jedes Pattern als ein Button mit X-Symbol LINKS - bricht automatisch um
                                let button_text = format!("❌ {}", pattern);
                                let button = egui::Button::new(button_text)
                                    .fill(egui::Color32::TRANSPARENT) // Kein Hintergrund
                                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(100))) // Dünner grauer Rand
                                    .min_size(egui::vec2(0.0, 20.0)); // Mindesthöhe für bessere Klickbarkeit
                                
                                if ui.add(button)
                                    .on_hover_text("Click to remove this pattern")
                                    .clicked() {
                                    pattern_to_remove = Some(pattern.clone());
                                }
                            }
                            
                            if let Some(pattern) = pattern_to_remove {
                                self.tree_ignore_patterns.remove(&pattern);
                            }
                        });
                    }
                }
            });
            ui.add_space(2.0);
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
        // Zuerst alle automatisch hinzugefügten Ignore-Patterns der letzten Runde entfernen,
        // um einen sauberen Zustand für die neue Filterung zu schaffen.
        self.config.ignore_patterns.retain(|p| !self.auto_removed_dir_patterns.contains(p));
        self.auto_removed_dir_patterns.clear();

        let filter = SearchFilter {
            query: self.search_query.clone(),
            extension: self.file_extension_filter.clone(),
            case_sensitive: self.case_sensitive,
            ignore_patterns: self.config.ignore_patterns.clone(),
        };
        
        let mut filtered = SearchEngine::filter_files(&self.file_tree, &filter);

        // Füge Elternverzeichnisse für gefilterte Dateien hinzu, um die Baumstruktur zu erhalten.
        let root_path = PathBuf::from(&self.current_path);
        let required_dirs: HashSet<PathBuf> = filtered.par_iter().flat_map(|item| {
            let mut parents = Vec::new();
            if !item.is_directory {
                let mut current = item.path.parent();
                while let Some(parent) = current {
                    if parent.starts_with(&root_path) && parent != &root_path {
                        parents.push(parent.to_path_buf());
                    } else {
                        break;
                    }
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

        // NEUE LOGIK: Wenn "Remove empty dirs" aktiv ist.
        if self.config.remove_empty_directories {
            let (pruned_files, removed_dirs) = SearchEngine::remove_empty_directories(filtered);
            
            for dir_path in removed_dirs {
                if let Ok(relative_path) = dir_path.strip_prefix(&root_path) {
                    // Füge den relativen Pfad als Ignore-Pattern hinzu.
                    // Wichtig: mit Suffix '/', damit es als Verzeichnis behandelt wird.
                    let pattern = format!("{}/", relative_path.to_string_lossy());
                    self.config.ignore_patterns.insert(pattern.clone());
                    self.auto_removed_dir_patterns.insert(pattern);
                }
            }
            filtered = pruned_files;
        }

        self.filtered_files = filtered;
        
        // *** GEÄNDERT: Nur noch Konsistenz-Prüfung gegen file_tree, nicht gegen filtered_files ***
        // selected_files sollte nur bereinigt werden, wenn Files tatsächlich aus file_tree entfernt wurden
        let valid_paths_in_tree: HashSet<PathBuf> = self.file_tree.par_iter().map(|item| item.path.clone()).collect();
        
        // 1. Bereinige selected_files nur gegen file_tree (nicht gegen filtered_files!)
        self.selected_files.retain(|path| valid_paths_in_tree.contains(path));
        
        // 2. Bereinige expanded_dirs nur gegen file_tree
        self.expanded_dirs.retain(|path| valid_paths_in_tree.contains(path));
        
        // 3. Bereinige preview_file nur wenn es nicht mehr in file_tree existiert
        if let Some(preview_path) = &self.preview_file {
            if !valid_paths_in_tree.contains(preview_path) {
                self.preview_file = None;
                self.preview_content.clear();
                self.highlighted_preview_lines.clear();
            }
        }
        
        tracing::info!("apply_filters: {} files filtered, {} selected total (may include hidden)", 
            self.filtered_files.len(), self.selected_files.len());
    }
    

    pub fn select_all_files(&mut self) {
        // *** GEÄNDERT: Wähle nur sichtbare (gefilterte) Files aus ***
        // Das ist konsistenter mit der UI-Erwartung - "Select All" wählt nur sichtbare Items aus
        for file in &self.filtered_files {
            if !file.is_directory { 
                self.selected_files.insert(file.path.clone()); 
            }
        }
        tracing::info!("select_all_files: {} visible files selected", 
            self.filtered_files.iter().filter(|item| !item.is_directory).count());
    }
    
    pub fn load_file_preview(&mut self, file_path: &PathBuf) {
        self.generated_content = None;
        self.preview_file = Some(file_path.clone());
        match FileHandler::get_file_preview(file_path, 1500) {
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
        
        // *** KRITISCH: Letzte Konsistenz-Prüfung vor der Generierung ***
        let selected_files_ordered = self.get_selected_files_in_tree_order();
        
        if selected_files_ordered.is_empty() {
            tracing::warn!("generate_preview: No valid selected files found after consistency check");
            return;
        }
        
        tracing::info!("generate_preview: Processing {} files in tree order", selected_files_ordered.len());

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
                    &selected_files_ordered, // *** HIER die sortierte Liste verwenden ***
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
        if let Some(content) = self.generated_content.clone() {
            self.update_default_filename_if_needed();

            // --- VERSUCH 1: Primärer Pfad ---
            let primary_path_str = self.output_path.clone();
            if self.try_write_to_dir(&content, &primary_path_str).is_ok() {
                return; // Erfolg!
            }

            // --- VERSUCH 2: Fallback-Pfad (Desktop oder Home) ---
            tracing::warn!("Schreiben in den primären Pfad '{}' fehlgeschlagen. Versuche Fallback.", primary_path_str);
            
            let fallback_path = dirs::desktop_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default());
            
            if let Some(fallback_str) = fallback_path.to_str() {
                if self.try_write_to_dir(&content, fallback_str).is_ok() {
                    self.output_path = fallback_str.to_string();
                    return; // Erfolg!
                }
            }

            // --- LETZTER SCHRITT: Fehler-Pop-up anzeigen (mit mehr Details) ---
            tracing::error!("Schreiben in den Fallback-Pfad fehlgeschlagen. Zeige Fehler-Pop-up.");
            
            // ÄNDERUNG HIER: Die Fehlermeldung wird jetzt dynamisch mit den Pfaden erstellt.
            let error_message = format!(
                "Failed to save the file.\n\nCould not write to the primary directory:\n'{}'\n\nAlso failed to write to the fallback directory:\n'{}'\n\nPlease check your write permissions.",
                primary_path_str,
                fallback_path.display()
            );
            self.save_error_message = Some(error_message);
        }
    }

    fn try_write_to_dir(&mut self, content: &str, output_dir_str: &str) -> Result<(), std::io::Error> {
        if output_dir_str.is_empty() {
            return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Output directory is not specified"));
        }
        
        let output_dir = PathBuf::from(output_dir_str);
        std::fs::create_dir_all(&output_dir)?;

        let mut final_path = output_dir.join(&self.output_filename);
        let mut counter = 1;

        while final_path.exists() {
            let stem = Path::new(&self.output_filename).file_stem().and_then(|s| s.to_str()).unwrap_or("");
            let extension = Path::new(&self.output_filename).extension().and_then(|s| s.to_str()).unwrap_or("");
            let new_filename = if extension.is_empty() {
                format!("{}_({})", stem, counter)
            } else {
                format!("{}_({}).{}", stem, counter, extension)
            };
            final_path = output_dir.join(new_filename);
            counter += 1;
        }

        std::fs::write(&final_path, content)?;

        tracing::info!("Successfully saved file to: {}", final_path.display());
        self.open_output_in_finder();

        Ok(())
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

    fn render_save_error_popup(&mut self, ctx: &egui::Context) {
        if let Some(error_message) = &self.save_error_message.clone() {
            let mut is_open = true;
            egui::Window::new("Save Error")
                .open(&mut is_open)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("💥").size(30.0));
                        ui.add_space(10.0);
                        ui.label(error_message);
                        ui.add_space(10.0);
                        if ui.button("Close").clicked() {
                            self.save_error_message = None;
                        }
                    });
                });

            if !is_open {
                self.save_error_message = None;
            }
        }
    }

    pub fn start_content_search(&mut self) {
        if self.search_in_files_query.is_empty() || self.is_searching_content { return; }
        self.is_searching_content = true;
        let search_query = self.search_in_files_query.clone();
        let case_sensitive = self.case_sensitive; // <- HINZUGEFÜGT
        let files_to_search: Vec<FileItem> = self.file_tree.iter().filter(|item| !item.is_directory && !item.is_binary).cloned().collect();
        let (result_sender, result_receiver) = mpsc::unbounded_channel();
        self.content_search_receiver = Some(Arc::new(tokio::sync::Mutex::new(result_receiver)));
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async move {
                let mut matching_files = Vec::new();
                for file_item in files_to_search {
                    if let Ok(content) = std::fs::read_to_string(&file_item.path) {
                        // <- GEÄNDERT: Case Sensitivity berücksichtigen
                        let matches = if case_sensitive {
                            content.contains(&search_query)
                        } else {
                            content.to_lowercase().contains(&search_query.to_lowercase())
                        };
                        
                        if matches {
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
                    ui.heading("Large Files Skipped");
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

impl ContextFileConcatApp {
    /// Sammelt ALLE ausgewählten Dateien in der Tree-Reihenfolge
    /// UNABHÄNGIG davon, ob ihre Eltern-Verzeichnisse im UI expanded sind
    /// UNABHÄNGIG vom aktuellen visuellen Filter
    fn get_selected_files_in_tree_order(&self) -> Vec<PathBuf> {
        let current_root = PathBuf::from(&self.current_path);
        
        // 1. Sammle alle ausgewählten Dateien (nicht Verzeichnisse) aus file_tree, NICHT aus filtered_files
        let mut selected_file_items: Vec<&FileItem> = self.file_tree  // <-- GEÄNDERT: von filtered_files zu file_tree
            .iter()
            .filter(|item| !item.is_directory && self.selected_files.contains(&item.path))
            .collect();
        
        // 2. Sortiere sie in der korrekten Tree-Reihenfolge basierend auf ihrem Pfad
        selected_file_items.sort_by(|a, b| {
            self.compare_paths_for_tree_order(&a.path, &b.path, &current_root)
        });
        
        // 3. Extrahiere die Pfade
        let ordered_files: Vec<PathBuf> = selected_file_items
            .into_iter()
            .map(|item| item.path.clone())
            .collect();
        
        // 4. Doppelte Validierung für 100% Konsistenz gegen file_tree (nicht filtered_files)
        let tree_paths: std::collections::HashSet<PathBuf> = self.file_tree  // <-- GEÄNDERT: von filtered_files zu file_tree
            .iter()
            .map(|item| item.path.clone())
            .collect();
        
        let final_files: Vec<PathBuf> = ordered_files
            .into_iter()
            .filter(|path| {
                self.selected_files.contains(path) && tree_paths.contains(path)
            })
            .collect();
        
        tracing::info!("get_selected_files_in_tree_order: {} files in tree order (independent of UI filter)", final_files.len());
        final_files
    }
    
    /// Vergleicht zwei Pfade für die korrekte Tree-Reihenfolge
    /// Implementiert die gleiche Logik wie das UI-Tree: Verzeichnisse zuerst, dann alphabetisch
    fn compare_paths_for_tree_order(&self, path_a: &PathBuf, path_b: &PathBuf, root: &PathBuf) -> std::cmp::Ordering {
        // Berechne relative Pfade
        let rel_a = path_a.strip_prefix(root).unwrap_or(path_a);
        let rel_b = path_b.strip_prefix(root).unwrap_or(path_b);
        
        // Vergleiche die Pfad-Komponenten Ebene für Ebene
        let components_a: Vec<_> = rel_a.components().collect();
        let components_b: Vec<_> = rel_b.components().collect();
        
        // Vergleiche gemeinsame Pfad-Ebenen
        let min_len = components_a.len().min(components_b.len());
        
        for i in 0..min_len {
            let comp_a = &components_a[i];
            let comp_b = &components_b[i];
            
            if comp_a != comp_b {
                // Wenn wir auf der letzten Ebene sind, vergleiche direkt
                if i == min_len - 1 {
                    // Beide sind auf der gleichen Ebene - alphabetisch sortieren
                    return comp_a.as_os_str().cmp(comp_b.as_os_str());
                } else {
                    // Wir sind in einem Zwischenpfad - prüfe, ob eines ein Verzeichnis ist
                    let is_dir_a = i < components_a.len() - 1; // Hat weitere Komponenten = ist Verzeichnis
                    let is_dir_b = i < components_b.len() - 1; // Hat weitere Komponenten = ist Verzeichnis
                    
                    match (is_dir_a, is_dir_b) {
                        (true, false) => return std::cmp::Ordering::Less,    // Verzeichnis vor Datei
                        (false, true) => return std::cmp::Ordering::Greater, // Datei nach Verzeichnis
                        _ => return comp_a.as_os_str().cmp(comp_b.as_os_str()), // Beide gleich -> alphabetisch
                    }
                }
            }
        }
        
        // Einer ist ein Präfix des anderen - der kürzere (Verzeichnis) kommt zuerst
        components_a.len().cmp(&components_b.len())
    }

    /// Debug-Methode um die Sortierung zu validieren
    fn debug_file_order(&self, files: &[PathBuf]) {
        tracing::info!("=== FILE ORDER DEBUG ===");
        for (i, file) in files.iter().enumerate() {
            let relative = file.strip_prefix(&self.current_path).unwrap_or(file);
            tracing::info!("{:3}: {}", i + 1, relative.display());
        }
        tracing::info!("=== END FILE ORDER DEBUG ===");
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