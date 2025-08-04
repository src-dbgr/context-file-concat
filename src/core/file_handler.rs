//! Handles file content operations like reading, previewing, and concatenation.

use super::{CoreError, FileItem, TreeGenerator};
use crate::utils::file_detection::is_text_file;
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A utility struct for handling file-related operations.
///
/// This struct is stateless and provides methods as associated functions.
pub struct FileHandler;

impl FileHandler {
    /// Generates a single string by concatenating the content of selected files.
    ///
    /// It includes a header with metadata, an optional directory tree, and formatted
    /// content blocks for each selected file. This operation is cancellable.
    pub async fn generate_concatenated_content_simple(
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
        cancel_flag: Arc<AtomicBool>,
        // This parameter only exists during `cargo test` runs. It allows deterministic
        // testing of the cancellation logic without affecting the production build.
        #[cfg(test)] mut test_notifier: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<String, CoreError> {
        let mut content = String::new();
        content.push_str(&format!(
            "# CFC Output - Generated: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        ));
        content.push_str(&format!("# Total files: {}\n\n", selected_files.len()));

        if include_tree {
            let tree =
                TreeGenerator::generate_tree(&items_for_tree, root_path, &tree_ignore_patterns);
            content.push_str("# DIRECTORY TREE\n");
            content.push_str("=====================\n");
            content.push_str(&tree);
            content.push_str("=====================\n\n");
        }

        for file_path in selected_files {
            // In test builds, this block allows a test to synchronize with the function,
            // proving that cancellation works deterministically. It is completely removed
            // from release builds, incurring zero overhead.
            #[cfg(test)]
            if let Some(notifier) = test_notifier.take() {
                let _ = notifier.send(());
                // This pause is crucial for the test environment. It yields control back
                // to the Tokio scheduler, allowing the test runner to set the cancel flag
                // before this task continues.
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }

            // Check for cancellation at the start of each file processing step.
            // `SeqCst` provides the strongest memory ordering guarantee, ensuring that
            // changes to the flag from other threads are immediately visible.
            if cancel_flag.load(Ordering::SeqCst) {
                return Err(CoreError::Cancelled);
            }

            // Directories in the selection list are silently skipped.
            if file_path.is_dir() {
                continue;
            }

            let display_path = if use_relative_paths {
                if let Some(parent) = root_path.parent() {
                    file_path.strip_prefix(parent)?.display().to_string()
                } else {
                    // Fallback for root paths that have no parent (e.g., "/")
                    file_path.display().to_string()
                }
            } else {
                file_path.display().to_string()
            };

            content.push_str(&format!("{display_path}\n"));
            content.push_str("===FILE-START===\n");

            let file_content = Self::read_file_content(file_path)?;
            content.push_str(&file_content);

            // Ensure the content block ends with a newline for consistent formatting.
            if !file_content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str("---FILE-END-----\n\n");
        }
        Ok(content)
    }

    /// Reads the content of a file, with safeguards for large or binary files.
    fn read_file_content(file_path: &Path) -> Result<String, CoreError> {
        let metadata =
            fs::metadata(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;

        // Skip files that exceed the size limit to prevent excessive memory usage.
        if metadata.len() > 20 * 1024 * 1024 {
            return Ok(format!(
                "[FILE TOO LARGE: {} bytes - CONTENT SKIPPED]",
                metadata.len()
            ));
        }

        // Attempt to read the file as a UTF-8 string.
        match fs::read_to_string(file_path) {
            Ok(content) => Ok(content),
            // If reading as a string fails, it's likely binary or has an incompatible encoding.
            Err(_) => {
                let bytes =
                    fs::read(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;

                // Use a lossy conversion to create a string preview. If the conversion
                // introduces Unicode replacement characters, we classify it as binary.
                match String::from_utf8_lossy(&bytes) {
                    content if content.contains('\u{FFFD}') => {
                        Ok("[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]".to_string())
                    }
                    // Otherwise, the content might be valid in a different encoding but still mostly readable.
                    content => Ok(content.to_string()),
                }
            }
        }
    }

    /// Retrieves a truncated preview of a text file's content.
    ///
    /// Reads up to a specified maximum number of lines. Identifies directories and binary files.
    pub fn get_file_preview(file_path: &Path, max_lines: usize) -> Result<String, CoreError> {
        if file_path.is_dir() {
            return Ok("[DIRECTORY]".to_string());
        }

        // Use a utility to quickly check if the file is likely text-based.
        if !is_text_file(file_path)
            .map_err(|e| CoreError::Io(std::io::Error::other(e), file_path.to_path_buf()))?
        {
            return Ok("[BINARY FILE]".to_string());
        }

        let file =
            fs::File::open(file_path).map_err(|e| CoreError::Io(e, file_path.to_path_buf()))?;
        let reader = BufReader::new(file);
        let mut preview = String::new();

        for (i, line) in reader.lines().enumerate() {
            if i >= max_lines {
                preview.push_str("...\n[Preview truncated]");
                break;
            }
            match line {
                Ok(line_content) => {
                    preview.push_str(&line_content);
                    preview.push('\n');
                }
                // If a line cannot be read (e.g., due to invalid UTF-8 mid-file),
                // insert an error message.
                Err(_) => {
                    preview.push_str("[ERROR READING LINE]\n");
                }
            }
        }
        Ok(preview)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::FileItem;
    use std::collections::HashSet;
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tempfile::TempDir;

    // Die Helferfunktionen bleiben unverändert
    fn setup_test_environment() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("src/module")).unwrap();
        fs::create_dir_all(root.join("assets")).unwrap();
        fs::create_dir_all(root.join("docs")).unwrap();
        fs::write(root.join("README.md"), "This is the main readme.").unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() {\n    println!(\"Hello, world!\");\n}",
        )
        .unwrap();
        fs::write(
            root.join("src/lib.rs"),
            "pub fn add(left: usize, right: usize) -> usize {\n    left + right\n}",
        )
        .unwrap();
        fs::write(root.join("src/module/component.rs"), "// A UI component").unwrap();
        let mut long_file_content = String::new();
        for i in 1..=20 {
            long_file_content.push_str(&format!("Line {}\n", i));
        }
        fs::write(root.join("docs/large_file.txt"), long_file_content).unwrap();
        fs::write(
            root.join("assets/logo.png"),
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .unwrap();
        fs::write(root.join("assets/data.bin"), &[0xFF, 0xFE, 0xFD]).unwrap();
        let large_file_path = root.join("huge_file.log");
        let mut large_file = File::create(&large_file_path).unwrap();
        let chunk = vec![0u8; 1024];
        for _ in 0..(21 * 1024) {
            large_file.write_all(&chunk).unwrap();
        }
        (dir, root)
    }

    fn create_file_items(root: &Path, paths: &[&str]) -> Vec<FileItem> {
        paths
            .iter()
            .map(|p| {
                let full_path = root.join(p);
                let metadata = fs::metadata(&full_path).unwrap();
                let is_binary = if metadata.is_dir() {
                    false
                } else {
                    !crate::utils::file_detection::is_text_file(&full_path).unwrap_or(false)
                };
                FileItem {
                    path: full_path.clone(),
                    is_directory: metadata.is_dir(),
                    is_binary,
                    size: metadata.len(),
                    depth: p.split('/').count(),
                    parent: full_path.parent().map(|p| p.to_path_buf()),
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn concatenated_content_relative_with_tree() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        let selected_files = vec![root.join("src/main.rs"), root.join("README.md")];
        let all_items =
            create_file_items(&root, &["src", "src/main.rs", "src/lib.rs", "README.md"]);

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            true,
            all_items,
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_absolute_no_tree() {
        let (_dir, root) = setup_test_environment();
        let selected_files = vec![root.join("src/main.rs"), root.join("README.md")];
        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            false,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (root.to_str().unwrap(), "[ROOT]"),
        ]);
        settings.bind(|| {
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_with_tree_ignores() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();
        let selected_files = vec![
            root.join("src/main.rs"),
            root.join("README.md"),
            root.join("assets/logo.png"),
        ];
        let all_items = create_file_items(
            &root,
            &[
                "src",
                "src/main.rs",
                "README.md",
                "assets",
                "assets/logo.png",
            ],
        );
        let mut tree_ignore_patterns = HashSet::new();
        tree_ignore_patterns.insert("assets/".to_string());

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            true,
            all_items,
            tree_ignore_patterns,
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn content_reading_edge_cases() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();
        let selected_files = vec![root.join("huge_file.log"), root.join("assets/data.bin")];
        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            insta::assert_snapshot!(content);
        });
    }

    #[test]
    fn get_file_preview_all_cases() {
        let (_dir, root) = setup_test_environment();
        let dir_path = root.join("src");
        let preview = FileHandler::get_file_preview(&dir_path, 10).unwrap();
        assert_eq!(preview, "[DIRECTORY]");

        let binary_path = root.join("assets/logo.png");
        let preview = FileHandler::get_file_preview(&binary_path, 10).unwrap();
        assert_eq!(preview, "[BINARY FILE]");

        let long_file_path = root.join("docs/large_file.txt");
        let preview = FileHandler::get_file_preview(&long_file_path, 5).unwrap();
        assert!(preview.starts_with("Line 1\nLine 2\nLine 3\nLine 4\nLine 5\n"));
        assert!(preview.ends_with("...\n[Preview truncated]"));

        let short_file_path = root.join("src/main.rs");
        let preview = FileHandler::get_file_preview(&short_file_path, 10).unwrap();
        let expected_content = "fn main() {\n    println!(\"Hello, world!\");\n}\n";
        assert_eq!(preview, expected_content);
    }

    #[tokio::test]
    async fn generate_content_should_fail_on_nonexistent_file() {
        // --- Setup ---
        let (_dir, root) = setup_test_environment();

        let selected_files = vec![
            root.join("README.md"),                    // This one is valid
            root.join("path/to/nonexistent/file.txt"), // This one is not
        ];

        // --- Execute ---
        let result = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await;

        // --- Assert ---
        // We expect the function to fail gracefully.
        assert!(
            result.is_err(),
            "Function should return an Err for non-existent files"
        );

        // Bonus: We can even check for the specific error type to make the test more precise.
        // The error should be an I/O error from trying to read the non-existent file.
        if let Err(e) = result {
            match e {
                CoreError::Io(_, path) => {
                    assert!(path.ends_with("file.txt"));
                }
                _ => panic!("Expected CoreError::Io, but got a different error type."),
            }
        }
    }

    #[tokio::test]
    async fn concatenated_content_handles_empty_selection() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        // Test with an empty vector of selected files
        let selected_files: Vec<PathBuf> = vec![];

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            true,   // include_tree
            vec![], // no items for tree either
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            // The snapshot should just contain the header and an empty tree
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_handles_file_without_final_newline() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        // Create a file specifically without a trailing newline
        let no_newline_path = root.join("no_newline.txt");
        fs::write(&no_newline_path, "Line 1\nLine 2").unwrap();

        // Create an empty file
        let empty_file_path = root.join("empty.txt");
        fs::write(&empty_file_path, "").unwrap();

        let selected_files = vec![no_newline_path, empty_file_path];

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false, // include_tree
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            // The snapshot will verify that a newline was added to the first file's content
            // and that the empty file was handled gracefully.
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_handles_special_path_characters() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        // --- Setup für diesen spezifischen Test ---
        // Erstelle einen Ordner mit Leerzeichen im Namen
        let complex_dir = root.join("Mein Ordner");
        fs::create_dir(&complex_dir).unwrap();

        // Erstelle eine Datei mit Umlaut im Namen in diesem Ordner
        let special_char_path = complex_dir.join("Lösung.rs");
        fs::write(&special_char_path, "pub const ANSWER: i32 = 42;").unwrap();

        // --- Test-Inputs ---
        let selected_files = vec![special_char_path.clone()];
        let all_items = vec![
            create_file_items(&root, &["Mein Ordner"])[0].clone(),
            create_file_items(&complex_dir, &["Lösung.rs"])[0].clone(),
        ];

        // --- Execute ---
        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            true, // Wir wollen den Baum sehen, um das Rendering zu prüfen
            all_items,
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        // --- Assert ---
        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            // Der Snapshot wird beweisen, ob die Sonderzeichen korrekt dargestellt werden.
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn content_reading_handles_exact_size_boundary() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        const MAX_SIZE: u64 = 20 * 1024 * 1024;

        // --- Setup für diesen spezifischen Test ---
        // Datei 1: Exakt die maximal erlaubte Grösse
        let exact_size_path = root.join("exact_20mb_file.dat");
        let file_ok = File::create(&exact_size_path).unwrap();
        file_ok.set_len(MAX_SIZE).unwrap(); // Effizientes Erstellen einer grossen Datei

        // Datei 2: Ein Byte zu gross
        let too_large_path = root.join("too_large_file.dat");
        let file_too_large = File::create(&too_large_path).unwrap();
        file_too_large.set_len(MAX_SIZE + 1).unwrap();

        // --- Test-Inputs ---
        let selected_files = vec![exact_size_path.clone(), too_large_path.clone()];

        // --- Execute ---
        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        // --- Assert ---
        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            // Dieser Snapshot wird das Verhalten an der Grenze exakt dokumentieren.
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_skips_directories_in_selection() {
        let (_dir, root) = setup_test_environment();
        let project_name = root.file_name().unwrap().to_str().unwrap();

        // Select a valid file AND a directory
        let selected_files = vec![root.join("src/main.rs"), root.join("src")];

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        // The snapshot should only contain the content of main.rs.
        // The directory should be silently skipped by the `continue` statement.
        let mut settings = insta::Settings::clone_current();
        settings.set_filters(vec![
            (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            (project_name, "[PROJECT_NAME]"),
        ]);
        settings.bind(|| {
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn concatenated_content_handles_root_path_without_parent() {
        // This test covers the `else` branch of `root_path.parent()`
        let (_dir, mut root) = setup_test_environment();
        let readme_path = root.join("README.md");

        // Simulate the root path being "/"
        root = PathBuf::from("/");
        let selected_files = vec![readme_path.clone()];

        let content = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
        .unwrap();

        // In this case, `use_relative_paths` falls back to the full, absolute path.
        insta::with_settings!({
            filters => vec![
                (readme_path.to_str().unwrap(), "[README_PATH]"),
                (r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}", "[DATETIME]"),
            ]
        }, {
            insta::assert_snapshot!(content);
        });
    }

    #[tokio::test]
    async fn generate_content_can_be_cancelled_deterministically() {
        let (_dir, root) = setup_test_environment();
        let selected_files = vec![root.join("src/main.rs"), root.join("README.md")];
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let (tx, rx) = tokio::sync::oneshot::channel();

        // Klone/kopiere alle Daten, die der Task besitzen muss
        let task_cancel_flag = cancel_flag.clone();
        let task_selected_files = selected_files.clone();
        let task_root = root.clone();

        let generation_task = tokio::spawn(async move {
            FileHandler::generate_concatenated_content_simple(
                &task_selected_files, // Verwende die owned Daten
                &task_root,
                false,
                vec![],
                HashSet::new(),
                true,
                task_cancel_flag,
                #[cfg(test)]
                Some(tx),
            )
            .await
        });

        // Wait for signal
        rx.await.expect("Task did not send start signal");

        // Set the flag over the original reference
        cancel_flag.store(true, Ordering::SeqCst);

        let result = generation_task.await.unwrap();

        assert!(
            result.is_err(),
            "The task should have returned an error upon cancellation"
        );
        matches!(result.unwrap_err(), CoreError::Cancelled);
    }

    #[test]
    fn get_preview_handles_corrupted_line() {
        let (_dir, root) = setup_test_environment();

        // Create a file that starts with valid text but contains an invalid UTF-8 sequence later
        let corrupted_file_path = root.join("corrupted.txt");
        // 0x99 is an invalid start byte for a UTF-8 sequence
        let content: Vec<u8> =
            b"This is a valid line.\nAnd this one is not -> \x99 so good.".to_vec();
        fs::write(&corrupted_file_path, content).unwrap();

        let preview = FileHandler::get_file_preview(&corrupted_file_path, 10).unwrap();

        // The snapshot will show that the second line failed to read.
        insta::assert_snapshot!(preview);
    }
}
