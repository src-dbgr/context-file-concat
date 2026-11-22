//! Handles file content operations like reading, previewing, and concatenation.
//!
//! Content generation is split into three concerns:
//! - [`OutputDocument`] formats the output document (pure, no IO),
//! - `load_selected_entry` and its helpers read and decode files (blocking IO),
//! - [`FileHandler::generate_concatenated_content_simple`] orchestrates both, keeping
//!   the blocking work off the async runtime and honouring cancellation.

use super::{CoreError, FileItem, TreeGenerator, MAX_FILE_SIZE};
use crate::utils::file_detection::{classify_by_extension, is_text_content, is_text_file};
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Placeholder emitted instead of the content of binary / undecodable files.
const BINARY_CONTENT_MARKER: &str = "[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]";

/// Builds the concatenated output document.
///
/// This type is concerned with formatting only; it performs no IO.
struct OutputDocument {
    buffer: String,
}

impl OutputDocument {
    /// Starts a document with the metadata header.
    fn new(generated_at: impl std::fmt::Display, total_files: usize) -> Self {
        Self {
            buffer: format!(
                "# CFC Output - Generated: {generated_at}\n# Total files: {total_files}\n\n"
            ),
        }
    }

    /// Appends the directory tree section.
    fn push_tree(&mut self, tree: &str) {
        self.buffer.push_str("# DIRECTORY TREE\n");
        self.buffer.push_str("=====================\n");
        self.buffer.push_str(tree);
        self.buffer.push_str("=====================\n\n");
    }

    /// Appends one file block consisting of its display path and body.
    fn push_file(&mut self, display_path: &str, body: &str) {
        self.buffer.push_str(display_path);
        self.buffer.push_str("\n===FILE-START===\n");
        self.buffer.push_str(body);
        // Ensure the content block ends with a newline for consistent formatting.
        if !body.ends_with('\n') {
            self.buffer.push('\n');
        }
        self.buffer.push_str("---FILE-END-----\n\n");
    }

    fn finish(self) -> String {
        self.buffer
    }
}

/// Returns the path under which a file is listed in the output document.
fn display_path(
    file_path: &Path,
    root_path: &Path,
    use_relative_paths: bool,
) -> Result<String, CoreError> {
    let shown = match root_path.parent() {
        Some(parent) if use_relative_paths => file_path.strip_prefix(parent)?,
        // Absolute paths were requested, or the root has no parent (e.g., "/").
        _ => file_path,
    };
    Ok(shown.display().to_string())
}

/// Loads the body for a selected path. Returns `None` for directories, which are
/// silently skipped by the generation.
///
/// Performs blocking IO; must not be called directly on the async runtime.
fn load_selected_entry(file_path: &Path) -> Option<String> {
    if file_path.is_dir() {
        return None;
    }
    Some(read_file_body(file_path))
}

/// Reads the body of a file for the output document.
///
/// Never fails: a file that cannot be read (e.g. deleted since the scan) yields an
/// inline marker, so that a single bad file does not abort the whole generation.
fn read_file_body(file_path: &Path) -> String {
    try_read_file_body(file_path).unwrap_or_else(|error| {
        tracing::warn!(
            "Skipping unreadable file {}: {}",
            file_path.display(),
            error
        );
        format!("[FILE UNREADABLE: {error} - CONTENT SKIPPED]")
    })
}

/// Reads a file once, with safeguards for large or binary files.
fn try_read_file_body(file_path: &Path) -> io::Result<String> {
    // Skip files that exceed the size limit to prevent excessive memory usage.
    let size = fs::metadata(file_path)?.len();
    if size > MAX_FILE_SIZE {
        return Ok(format!("[FILE TOO LARGE: {size} bytes - CONTENT SKIPPED]"));
    }
    Ok(decode_file_bytes(file_path, fs::read(file_path)?))
}

/// Turns raw file bytes into the text to embed (pure, no IO): the decoded text, or a
/// marker for binary content.
fn decode_file_bytes(file_path: &Path, bytes: Vec<u8>) -> String {
    decode_text(file_path, bytes).unwrap_or_else(|| BINARY_CONTENT_MARKER.to_string())
}

/// Decodes the raw bytes of a file into text (pure, no IO). Returns `None` for binary
/// content.
///
/// This is the one place that defines how the application reads text, shared by the
/// output generation and the content search, so both always see the same characters.
/// Valid UTF-8 is used as is. Anything else that still looks like text is taken to be
/// in a legacy single-byte encoding and decoded as Windows-1252, the superset of
/// ISO-8859-1 (Latin-1) that such files are overwhelmingly written in. Every byte maps
/// to exactly one character, so no content is lost or replaced.
pub(crate) fn decode_text(file_path: &Path, bytes: Vec<u8>) -> Option<String> {
    match String::from_utf8(bytes) {
        Ok(text) => Some(text),
        Err(error) if is_legacy_encoded_text(file_path, error.as_bytes()) => {
            Some(decode_windows_1252(error.as_bytes()))
        }
        Err(_) => None,
    }
}

/// The characters Windows-1252 assigns to the bytes `0x80..=0x9F`. All other bytes
/// denote the Unicode code point of the same value, as in ISO-8859-1. The five bytes
/// the encoding leaves undefined map to the C1 control character of the same value,
/// following the WHATWG Encoding Standard.
const WINDOWS_1252_HIGH_CONTROL_RANGE: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// Decodes bytes as Windows-1252. Total: every byte sequence has a decoding.
fn decode_windows_1252(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&byte| match byte {
            0x80..=0x9F => WINDOWS_1252_HIGH_CONTROL_RANGE[usize::from(byte - 0x80)],
            _ => char::from(byte),
        })
        .collect()
}

/// Decides whether non-UTF-8 bytes are still text (e.g. Latin-1), reusing the
/// application-wide detection: a known binary extension or binary-looking content
/// (NUL bytes, many control characters) disqualifies the file.
fn is_legacy_encoded_text(file_path: &Path, bytes: &[u8]) -> bool {
    classify_by_extension(file_path) != Some(false) && is_text_content(bytes)
}

/// Runs [`load_selected_entry`] on the blocking thread pool.
async fn load_selected_entry_off_runtime(file_path: PathBuf) -> Result<Option<String>, CoreError> {
    Ok(tokio::task::spawn_blocking(move || load_selected_entry(&file_path)).await?)
}

/// Test-only synchronisation point between two files.
///
/// Signals the test that a file has been fully processed, then holds the generation
/// until the test has requested cancellation. This makes the cancellation test
/// deterministic without relying on timing.
#[cfg(test)]
async fn pause_until_cancel_requested(
    notifier: tokio::sync::oneshot::Sender<()>,
    cancel_flag: &AtomicBool,
) {
    let _ = notifier.send(());
    while !cancel_flag.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
}

/// A utility struct for handling file-related operations.
///
/// This struct is stateless and provides methods as associated functions.
pub struct FileHandler;

impl FileHandler {
    /// Generates a single string by concatenating the content of selected files.
    ///
    /// It includes a header with metadata, an optional directory tree, and formatted
    /// content blocks for each selected file. This operation is cancellable between
    /// files. File IO runs on the blocking thread pool, not on the async runtime.
    ///
    /// Files that cannot be read get an inline marker instead of aborting the generation.
    // The production signature has seven parameters; only the test-only notifier below
    // pushes test builds over clippy's limit.
    #[cfg_attr(test, allow(clippy::too_many_arguments))]
    pub async fn generate_concatenated_content_simple(
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
        cancel_flag: Arc<AtomicBool>,
        // This parameter only exists during `cargo test` runs. It allows deterministic
        // testing of the cancellation logic without affecting the production build:
        // after the first file, the generation signals the test and waits for the
        // cancel flag (see `pause_until_cancel_requested`).
        #[cfg(test)] mut test_notifier: Option<tokio::sync::oneshot::Sender<()>>,
    ) -> Result<String, CoreError> {
        let mut document = OutputDocument::new(
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
            selected_files.len(),
        );

        if include_tree {
            let tree =
                TreeGenerator::generate_tree(&items_for_tree, root_path, &tree_ignore_patterns);
            document.push_tree(&tree);
        }

        for file_path in selected_files {
            // Check for cancellation at the start of each file processing step.
            // `SeqCst` provides the strongest memory ordering guarantee, ensuring that
            // changes to the flag from other threads are immediately visible.
            if cancel_flag.load(Ordering::SeqCst) {
                return Err(CoreError::Cancelled);
            }

            // Directories in the selection list are silently skipped.
            let Some(body) = load_selected_entry_off_runtime(file_path.clone()).await? else {
                continue;
            };

            let display_path = display_path(file_path, root_path, use_relative_paths)?;
            document.push_file(&display_path, &body);

            // Removed from release builds entirely, incurring zero overhead.
            #[cfg(test)]
            if let Some(notifier) = test_notifier.take() {
                pause_until_cancel_requested(notifier, &cancel_flag).await;
            }
        }
        Ok(document.finish())
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
            .map_err(|e| CoreError::Io(e.to_string(), file_path.to_path_buf()))?
        {
            return Ok("[BINARY FILE]".to_string());
        }

        let file = fs::File::open(file_path)
            .map_err(|e| CoreError::Io(e.to_string(), file_path.to_path_buf()))?;
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
            [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        )
        .unwrap();
        fs::write(root.join("assets/data.bin"), [0xFF, 0xFE, 0xFD]).unwrap();
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

    /// Runs a generation without tree and with relative paths for the given selection.
    async fn generate_plain(selected_files: &[PathBuf], root: &Path) -> Result<String, CoreError> {
        FileHandler::generate_concatenated_content_simple(
            selected_files,
            root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            None,
        )
        .await
    }

    /// Extracts the body of the file block whose display path ends with `name`.
    fn file_block_body<'a>(content: &'a str, name: &str) -> &'a str {
        let header = format!("{name}\n===FILE-START===\n");
        let start = content.find(&header).expect("file block not found") + header.len();
        let end = content[start..]
            .find("---FILE-END-----\n")
            .expect("file block not terminated");
        &content[start..start + end]
    }

    #[tokio::test]
    async fn generate_content_marks_unreadable_file_and_continues() {
        let (_dir, root) = setup_test_environment();

        let selected_files = vec![
            root.join("README.md"),                    // valid
            root.join("path/to/nonexistent/file.txt"), // vanished since the scan
            root.join("src/main.rs"),                  // must still be processed
        ];

        let content = generate_plain(&selected_files, &root)
            .await
            .expect("an unreadable file must not abort the generation");

        assert_eq!(
            file_block_body(&content, "README.md"),
            "This is the main readme.\n"
        );
        let marker = file_block_body(&content, "path/to/nonexistent/file.txt");
        assert!(
            marker.starts_with("[FILE UNREADABLE: ") && marker.ends_with(" - CONTENT SKIPPED]\n"),
            "unexpected marker: {marker:?}"
        );
        assert!(
            marker.len() > "[FILE UNREADABLE:  - CONTENT SKIPPED]\n".len(),
            "marker should state a reason: {marker:?}"
        );
        assert!(file_block_body(&content, "src/main.rs").starts_with("fn main() {"));
    }

    #[tokio::test]
    async fn latin1_text_file_is_included_with_its_characters_intact() {
        let (_dir, root) = setup_test_environment();
        let latin1_path = root.join("legacy.txt");
        // "Größe: Können wir? Ja." encoded as Latin-1: 0xF6, 0xDF are not valid UTF-8.
        fs::write(&latin1_path, b"Gr\xf6\xdfe: K\xf6nnen wir? Ja.\n").unwrap();

        let content = generate_plain(&[latin1_path], &root).await.unwrap();

        assert_eq!(
            file_block_body(&content, "legacy.txt"),
            "Größe: Können wir? Ja.\n"
        );
    }

    #[test]
    fn windows_1252_decoding_maps_every_byte_to_one_character() {
        let all_bytes: Vec<u8> = (0..=255).collect();

        let decoded = decode_windows_1252(&all_bytes);

        assert_eq!(decoded.chars().count(), 256);
        assert!(!decoded.contains('\u{FFFD}'));
        // ASCII and the Latin-1 range are identity mappings ...
        assert_eq!(decode_windows_1252(b"A\xe4\xf6\xfc\xdf\xff"), "Aäöüßÿ");
        // ... while 0x80..=0x9F holds the Windows-specific punctuation.
        assert_eq!(
            decode_windows_1252(b"\x80 \x93x\x94 \x85 \x99"),
            "€ “x” … ™"
        );
        assert_eq!(decode_windows_1252(b"\x81"), "\u{0081}");
    }

    #[tokio::test]
    async fn non_utf8_file_with_nul_bytes_is_skipped_as_binary() {
        let (_dir, root) = setup_test_environment();
        // A text extension must not override binary-looking content (e.g. UTF-16).
        let binary_path = root.join("utf16.txt");
        fs::write(&binary_path, b"\xff\xfeh\x00i\x00").unwrap();

        let content = generate_plain(&[binary_path], &root).await.unwrap();

        assert_eq!(
            file_block_body(&content, "utf16.txt"),
            "[BINARY OR NON-UTF8 FILE - CONTENT SKIPPED]\n"
        );
    }

    #[test]
    fn decode_file_bytes_classifies_content() {
        let decode = |name: &str, bytes: &[u8]| decode_file_bytes(Path::new(name), bytes.to_vec());

        assert_eq!(decode("a.txt", "héllo".as_bytes()), "héllo");
        assert_eq!(decode("no_extension", b"caf\xe9"), "café");
        // Known binary extensions stay binary even without NUL bytes.
        assert_eq!(decode("a.bin", b"caf\xe9"), BINARY_CONTENT_MARKER);
        assert_eq!(decode("a.txt", b"\x00\xff"), BINARY_CONTENT_MARKER);
    }

    #[test]
    fn output_document_formats_blocks_without_io() {
        let mut document = OutputDocument::new("NOW", 2);
        document.push_tree("tree\n");
        document.push_file("a.txt", "no newline");
        document.push_file("b.txt", "");

        assert_eq!(
            document.finish(),
            "# CFC Output - Generated: NOW\n# Total files: 2\n\n\
             # DIRECTORY TREE\n=====================\ntree\n=====================\n\n\
             a.txt\n===FILE-START===\nno newline\n---FILE-END-----\n\n\
             b.txt\n===FILE-START===\n\n---FILE-END-----\n\n"
        );
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

        const MAX_SIZE: u64 = MAX_FILE_SIZE;

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

        // Wait until the first file has been fully processed. The task now holds
        // before the second file until it observes the cancel flag.
        rx.await.expect("Task did not send start signal");

        // Set the flag over the original reference
        cancel_flag.store(true, Ordering::SeqCst);

        let result = generation_task.await.unwrap();

        assert!(
            matches!(result, Err(CoreError::Cancelled)),
            "The task should have been cancelled between the two files, got {result:?}"
        );
    }

    #[tokio::test]
    async fn generate_content_cancelled_before_start_returns_cancelled() {
        let (_dir, root) = setup_test_environment();
        // Even an unreadable file must not mask a cancellation.
        let selected_files = vec![root.join("does_not_exist.txt")];

        let result = FileHandler::generate_concatenated_content_simple(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(true)),
            #[cfg(test)]
            None,
        )
        .await;

        assert!(matches!(result, Err(CoreError::Cancelled)));
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
