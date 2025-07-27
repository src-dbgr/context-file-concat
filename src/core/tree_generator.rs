//! Generates an ASCII representation of a directory tree.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::{build_globset_from_patterns, FileItem};

/// A utility struct for generating an ASCII directory tree.
///
/// This struct is stateless and provides methods as associated functions.
pub struct TreeGenerator;

impl TreeGenerator {
    /// Generates a string representing the directory tree from a list of `FileItem`s.
    ///
    /// It filters the items based on tree-specific ignore patterns before rendering.
    pub fn generate_tree(
        files: &[FileItem],
        root_path: &Path,
        ignore_patterns: &HashSet<String>,
    ) -> String {
        // 1. Build a GlobSet from the tree-specific ignore patterns.
        let (ignore_set, _) = build_globset_from_patterns(ignore_patterns);

        // 2. Filter the provided files before building the tree.
        let filtered_files: Vec<&FileItem> = files
            .iter()
            .filter(|file| !ignore_set.is_match(&file.path))
            .collect();

        let mut tree_map = HashMap::new();

        // Build tree structure from the correctly filtered files.
        for file in filtered_files {
            let relative_path = file.path.strip_prefix(root_path).unwrap_or(&file.path);
            Self::insert_into_tree(&mut tree_map, relative_path, file.is_directory);
        }

        // Generate ASCII representation
        let mut result = String::new();
        result.push_str(&format!(
            "{}/\n",
            root_path.file_name().unwrap_or_default().to_string_lossy()
        ));

        Self::render_tree_recursive(&tree_map, &mut result, "", true);

        result
    }

    /// Inserts a path into the tree map structure.
    fn insert_into_tree(
        tree_map: &mut HashMap<PathBuf, TreeNode>,
        path: &Path,
        is_directory: bool,
    ) {
        let mut current_path = PathBuf::new();

        for component in path.components() {
            current_path.push(component);

            let is_final = current_path == path;
            let node_is_dir = if is_final { is_directory } else { true };

            tree_map
                .entry(current_path.clone())
                .or_insert_with(|| TreeNode {
                    name: component.as_os_str().to_string_lossy().to_string(),
                    is_directory: node_is_dir,
                    children: Vec::new(),
                });
        }

        // Build parent-child relationships
        let paths: Vec<PathBuf> = tree_map.keys().cloned().collect();
        for path in paths {
            if let Some(parent_path) = path.parent() {
                if parent_path != Path::new("") {
                    if let Some(parent_node) = tree_map.get_mut(parent_path) {
                        if !parent_node.children.contains(&path) {
                            parent_node.children.push(path.clone());
                        }
                    }
                }
            }
        }
    }

    /// Recursively renders the tree structure into a string.
    fn render_tree_recursive(
        tree_map: &HashMap<PathBuf, TreeNode>,
        result: &mut String,
        prefix: &str,
        is_root: bool,
    ) {
        let mut root_nodes: Vec<&PathBuf> = if is_root {
            tree_map
                .keys()
                .filter(|path| path.parent().is_none_or(|p| p == Path::new("")))
                .collect()
        } else {
            vec![]
        };

        root_nodes.sort_by(|a, b| {
            let a_node = &tree_map[*a];
            let b_node = &tree_map[*b];

            // Directories first, then files
            match (a_node.is_directory, b_node.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a_node.name.cmp(&b_node.name),
            }
        });

        for (i, path) in root_nodes.iter().enumerate() {
            let node = &tree_map[*path];
            let is_last = i == root_nodes.len() - 1;

            let connector = if is_last { "└── " } else { "├── " };
            let icon = if node.is_directory { "📁 " } else { "📄 " };

            result.push_str(&format!("{prefix}{connector}{icon}{}\n", node.name));

            // Recursively render children
            if !node.children.is_empty() {
                let new_prefix = if is_last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };

                Self::render_children(tree_map, &node.children, result, &new_prefix);
            }
        }
    }

    /// Renders the children of a tree node.
    fn render_children(
        tree_map: &HashMap<PathBuf, TreeNode>,
        children: &[PathBuf],
        result: &mut String,
        prefix: &str,
    ) {
        let mut sorted_children: Vec<&PathBuf> = children.iter().collect();
        sorted_children.sort_by(|a, b| {
            let a_node = &tree_map[*a];
            let b_node = &tree_map[*b];

            match (a_node.is_directory, b_node.is_directory) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a_node.name.cmp(&b_node.name),
            }
        });

        for (i, path) in sorted_children.iter().enumerate() {
            let node = &tree_map[*path];
            let is_last = i == sorted_children.len() - 1;

            let connector = if is_last { "└── " } else { "├── " };
            let icon = if node.is_directory { "📁 " } else { "📄 " };

            result.push_str(&format!("{prefix}{connector}{icon}{}\n", node.name));

            // Recursively render children
            if !node.children.is_empty() {
                let new_prefix = if is_last {
                    format!("{prefix}    ")
                } else {
                    format!("{prefix}│   ")
                };

                Self::render_children(tree_map, &node.children, result, &new_prefix);
            }
        }
    }
}

/// A transient node used for building the ASCII tree.
#[derive(Debug, Clone)]
struct TreeNode {
    name: String,
    is_directory: bool,
    children: Vec<PathBuf>,
}
