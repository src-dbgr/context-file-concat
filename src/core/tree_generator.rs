//! Generates an ASCII representation of a directory tree.

use super::FileItem;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

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
        // 1. Build a Matcher from the tree-specific ignore patterns.
        let mut ignore_builder = ignore::gitignore::GitignoreBuilder::new(root_path);
        for pattern in ignore_patterns {
            ignore_builder.add_line(None, pattern).ok();
        }
        let matcher = match ignore_builder.build() {
            Ok(m) => m,
            Err(_) => return String::from("Error building tree ignore patterns."),
        };

        // 2. Filter the provided files to get the final list of items to render.
        let filtered_files: Vec<&FileItem> = files
            .iter()
            .filter(|file| !matcher.matched(&file.path, file.is_directory).is_ignore())
            .collect();

        // 3. Create a map from parent directory paths to their children.
        let mut children_map: HashMap<PathBuf, Vec<&FileItem>> = HashMap::new();
        for item in &filtered_files {
            if let Some(parent) = item.path.parent() {
                children_map
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push(item);
            }
        }

        // 4. Generate the ASCII representation.
        let mut result = String::new();
        result.push_str(&format!(
            "{}/\n",
            root_path.file_name().unwrap_or_default().to_string_lossy()
        ));

        // Start the recursive rendering from the root path.
        Self::render_level(&mut result, root_path, &children_map, "");

        result
    }

    /// Recursively renders one level of the directory tree.
    fn render_level(
        result: &mut String,
        parent_path: &Path,
        children_map: &HashMap<PathBuf, Vec<&FileItem>>,
        prefix: &str,
    ) {
        if let Some(children) = children_map.get(parent_path) {
            let mut sorted_children = children.clone();
            // Sort entries: directories first, then alphabetically by name.
            sorted_children.sort_by(|a, b| {
                a.is_directory
                    .cmp(&b.is_directory)
                    .reverse()
                    .then_with(|| a.path.cmp(&b.path))
            });

            let last_index = sorted_children.len().saturating_sub(1);
            for (i, item) in sorted_children.iter().enumerate() {
                let is_last = i == last_index;
                let connector = if is_last { "└── " } else { "├── " };
                let icon = if item.is_directory { "📁 " } else { "📄 " };

                let file_name = item.path.file_name().unwrap_or_default().to_string_lossy();
                result.push_str(&format!("{prefix}{connector}{icon}{file_name}\n"));

                if item.is_directory {
                    let new_prefix = if is_last {
                        format!("{prefix}    ")
                    } else {
                        format!("{prefix}│   ")
                    };
                    Self::render_level(result, &item.path, children_map, &new_prefix);
                }
            }
        }
    }
}

// The tests you already added. No changes needed here.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FileItem;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    /// Helper function to create test items.
    fn create_item(path: &str, is_dir: bool) -> FileItem {
        FileItem {
            path: PathBuf::from(path),
            is_directory: is_dir,
            is_binary: false,
            size: if is_dir { 0 } else { 123 },
            depth: path.split('/').count(),
            parent: Path::new(path).parent().map(|p| p.to_path_buf()),
        }
    }

    #[test]
    fn test_basic_tree_generation() {
        let root_path = Path::new("/project");
        let files = vec![
            create_item("/project/src", true),
            create_item("/project/src/main.rs", false),
            create_item("/project/README.md", false),
        ];
        let ignore_patterns = HashSet::new();

        let tree_output = TreeGenerator::generate_tree(&files, root_path, &ignore_patterns);

        // This is the Insta snapshot assert!
        insta::assert_snapshot!(tree_output);
    }

    #[test]
    fn test_tree_with_ignored_files() {
        let root_path = Path::new("/project");
        let files = vec![
            create_item("/project/src", true),
            create_item("/project/src/main.rs", false),
            create_item("/project/target", true), // This should be ignored
            create_item("/project/target/debug", true),
            create_item("/project/README.md", false),
        ];
        let mut ignore_patterns = HashSet::new();
        ignore_patterns.insert("target/".to_string());

        let tree_output = TreeGenerator::generate_tree(&files, root_path, &ignore_patterns);

        insta::assert_snapshot!(tree_output);
    }

    // Add this test to the tests module in src/core/tree_generator.rs

    #[test]
    fn test_realistic_project_structure_with_ignores() {
        let root_path = Path::new("/real-world-project");
        let files = vec![
            // --- Source files (should be kept) ---
            create_item("/real-world-project/src", true),
            create_item("/real-world-project/src/index.js", false),
            create_item("/real-world-project/src/components", true),
            create_item("/real-world-project/src/components/Button.jsx", false),
            // --- Config files (should be kept) ---
            create_item("/real-world-project/.gitignore", false),
            create_item("/real-world-project/package.json", false),
            // --- Ignored top-level directory ---
            create_item("/real-world-project/node_modules", true),
            create_item("/real-world-project/node_modules/react", true),
            create_item("/real-world-project/node_modules/react/index.js", false),
            // --- Ignored build output directory ---
            create_item("/real-world-project/dist", true),
            create_item("/real-world-project/dist/bundle.js", false),
            // --- Ignored binary and image files by extension ---
            create_item("/real-world-project/assets", true),
            create_item("/real-world-project/assets/logo.png", false), // Should be ignored
            create_item("/real-world-project/vendor", true),
            create_item("/real-world-project/vendor/legacy.dll", false), // Should be ignored
        ];

        let mut ignore_patterns = HashSet::new();
        ignore_patterns.insert("node_modules/".to_string());
        ignore_patterns.insert("dist/".to_string());
        ignore_patterns.insert("*.png".to_string());
        ignore_patterns.insert("*.dll".to_string());

        let tree_output = TreeGenerator::generate_tree(&files, root_path, &ignore_patterns);

        insta::assert_snapshot!(tree_output);
    }
}
