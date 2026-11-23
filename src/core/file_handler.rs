//! Handles file content operations like reading, previewing, and concatenation.
//!
//! Content generation is split into three concerns:
//! - [`OutputDocument`] formats the output document (pure, no IO),
//! - `load_selected_entry` and its helpers read and decode files (blocking IO),
//! - [`FileHandler::generate_concatenated_content_simple`] orchestrates both, keeping
//!   the blocking work off the async runtime and honouring cancellation. What to
//!   generate is described by a [`GenerationRequest`]; a [`GenerationObserver`] can
//!   follow the progress.

#[cfg(test)]
use super::MAX_FILE_SIZE;
use super::{CoreError, FileItem, TreeGenerator};
use crate::utils::file_detection::{classify_by_extension, is_text_content, is_text_file};
use std::collections::HashSet;
use std::fs;
use std::io::{self, Read};
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
fn load_selected_entry(file_path: &Path, max_file_size: u64) -> Option<String> {
    if file_path.is_dir() {
        return None;
    }
    Some(read_file_body(file_path, max_file_size))
}

/// Reads the body of a file for the output document.
///
/// Never fails: a file that cannot be read (e.g. deleted since the scan) yields an
/// inline marker, so that a single bad file does not abort the whole generation.
fn read_file_body(file_path: &Path, max_file_size: u64) -> String {
    try_read_file_body(file_path, max_file_size).unwrap_or_else(|error| {
        tracing::warn!(
            "Skipping unreadable file {}: {}",
            file_path.display(),
            error
        );
        format!("[FILE UNREADABLE: {error} - CONTENT SKIPPED]")
    })
}

/// Reads a file once, with safeguards for large or binary files.
fn try_read_file_body(file_path: &Path, max_file_size: u64) -> io::Result<String> {
    // Skip files that exceed the size limit to prevent excessive memory usage.
    let size = fs::metadata(file_path)?.len();
    if size > max_file_size {
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
/// output generation, the file preview and the content search, so all of them always
/// see the same characters. The rule:
///
/// 1. Valid UTF-8 is used as is.
/// 2. Anything else is binary if the application-wide detection says so (a known
///    binary extension, NUL bytes, many control characters).
/// 3. The remaining files are text with bytes that are not UTF-8, and
///    [`guess_encoding`] decides what they are: UTF-8 with damaged places, which are
///    replaced by U+FFFD while everything around them stays intact, or a legacy
///    single-byte encoding, read as Windows-1252.
pub(crate) fn decode_text(file_path: &Path, bytes: Vec<u8>) -> Option<String> {
    let bytes = match String::from_utf8(bytes) {
        Ok(text) => return Some(text),
        Err(error) => error.into_bytes(),
    };
    if !is_text_despite_invalid_utf8(file_path, &bytes) {
        return None;
    }
    Some(match guess_encoding(&bytes) {
        TextEncoding::Utf8 => String::from_utf8_lossy(&bytes).into_owned(),
        TextEncoding::Windows1252 => decode_windows_1252(&bytes),
    })
}

/// What text that is not valid UTF-8 as a whole is taken to be.
#[derive(Debug, PartialEq, Eq)]
enum TextEncoding {
    /// UTF-8 with some invalid sequences.
    Utf8,
    /// A legacy single-byte encoding. Windows-1252 is the superset of ISO-8859-1
    /// (Latin-1) that such files are overwhelmingly written in; every byte maps to
    /// exactly one character, so nothing is lost or replaced.
    Windows1252,
}

/// Decides how to read text that is not valid UTF-8 as a whole.
///
/// The content is UTF-8 if it holds at least one valid multi-byte UTF-8 sequence
/// (a non-ASCII character) and at least as many of them as invalid sequences;
/// otherwise it is Windows-1252. ASCII says nothing, it is the same in both.
///
/// The reasoning: in a legacy encoding a non-ASCII character is a single byte that is
/// followed by ASCII nearly always, which is invalid UTF-8, so such a file has many
/// invalid sequences and (but for rare coincidences) no valid multi-byte one. A UTF-8
/// file with a few damaged places is the opposite. Reading that one as Windows-1252
/// would garble every non-ASCII character of it.
fn guess_encoding(bytes: &[u8]) -> TextEncoding {
    let mut valid_multi_byte_sequences = 0usize;
    let mut invalid_sequences = 0usize;
    for chunk in bytes.utf8_chunks() {
        valid_multi_byte_sequences += chunk.valid().chars().filter(|c| !c.is_ascii()).count();
        if !chunk.invalid().is_empty() {
            invalid_sequences += 1;
        }
    }
    if valid_multi_byte_sequences > 0 && valid_multi_byte_sequences >= invalid_sequences {
        TextEncoding::Utf8
    } else {
        TextEncoding::Windows1252
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

/// Decides whether bytes that are not valid UTF-8 are still text, reusing the
/// application-wide detection: a known binary extension or binary-looking content
/// (NUL bytes, many control characters) disqualifies the file.
fn is_text_despite_invalid_utf8(file_path: &Path, bytes: &[u8]) -> bool {
    classify_by_extension(file_path) != Some(false) && is_text_content(bytes)
}

/// Runs [`load_selected_entry`] on the blocking thread pool.
async fn load_selected_entry_off_runtime(
    file_path: PathBuf,
    max_file_size: u64,
) -> Result<Option<String>, CoreError> {
    Ok(tokio::task::spawn_blocking(move || load_selected_entry(&file_path, max_file_size)).await?)
}

/// The most a preview reads of a file, however long its lines are. A file whose
/// first lines are longer than this (minified code, a dump on one line) is cut here.
const PREVIEW_MAX_BYTES: usize = 4 * 1024 * 1024;

/// How much a preview reads at a time. It reads at most this much beyond the lines
/// it shows.
const PREVIEW_CHUNK_SIZE: usize = 16 * 1024;

/// Reads the beginning of `reader`: enough for `max_lines` lines, at most `max_bytes`
/// bytes. The second value tells whether something was left unread.
///
/// Reading stops as soon as more than `max_lines` line ends were seen, so the size of
/// what lies behind them does not matter.
///
/// A cut never ends inside a UTF-8 character: an incomplete sequence at the end of the
/// bytes read is dropped, so the cut alone cannot turn valid UTF-8 into something else.
fn read_preview_prefix(
    mut reader: impl Read,
    max_lines: usize,
    max_bytes: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    let mut line_ends = 0usize;
    let mut chunk = [0u8; PREVIEW_CHUNK_SIZE];

    let reached_end = loop {
        if line_ends > max_lines || bytes.len() > max_bytes {
            break false;
        }
        // One byte more than allowed tells a longer input from one of exactly
        // `max_bytes` bytes.
        let wanted = chunk.len().min(max_bytes + 1 - bytes.len());
        let read = match reader.read(&mut chunk[..wanted]) {
            Ok(0) => break true,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        line_ends += chunk[..read].iter().filter(|&&byte| byte == b'\n').count();
        bytes.extend_from_slice(&chunk[..read]);
    };

    if !reached_end {
        bytes.truncate(max_bytes);
        drop_incomplete_utf8_tail(&mut bytes);
    }
    Ok((bytes, !reached_end))
}

/// Removes a UTF-8 sequence the bytes end in the middle of. Sequences that are
/// invalid rather than incomplete stay.
fn drop_incomplete_utf8_tail(bytes: &mut Vec<u8>) {
    // An incomplete sequence is at most three bytes long, so only the end is checked.
    for start in bytes.len().saturating_sub(3)..bytes.len() {
        if let Err(error) = std::str::from_utf8(&bytes[start..]) {
            // `error_len() == None`: the input ended in the middle of a character.
            if error.error_len().is_none() && error.valid_up_to() == 0 {
                bytes.truncate(start);
                return;
            }
        }
    }
}

/// What a generation produces: which files, in which layout, under which limits.
pub struct GenerationRequest<'a> {
    /// The files whose content is embedded, in output order.
    pub selected_files: &'a [PathBuf],
    /// The project root; display paths and the tree are relative to it.
    pub root_path: &'a Path,
    pub include_tree: bool,
    pub items_for_tree: Vec<FileItem>,
    pub tree_ignore_patterns: HashSet<String>,
    pub use_relative_paths: bool,
    /// Files larger than this many bytes are represented by a marker instead of
    /// their content.
    pub max_file_size: u64,
}

/// Follows the progress of a generation. All notifications default to doing nothing,
/// and `()` is the observer that is not interested in any of them.
///
/// The application does not show the progress of a generation, so it passes `()`. The
/// trait is nevertheless the way to learn that a file has been appended: it is how
/// the tests cancel a generation at an exact point between two files instead of
/// racing against it, and where a progress display would hook in.
pub trait GenerationObserver: Sync {
    /// The block of `file_path` has been appended to the document.
    fn file_appended(&self, _file_path: &Path) {}
}

impl GenerationObserver for () {}

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
    pub async fn generate_concatenated_content_simple(
        request: GenerationRequest<'_>,
        cancel_flag: Arc<AtomicBool>,
        observer: &impl GenerationObserver,
    ) -> Result<String, CoreError> {
        let GenerationRequest {
            selected_files,
            root_path,
            include_tree,
            items_for_tree,
            tree_ignore_patterns,
            use_relative_paths,
            max_file_size,
        } = request;

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
            let Some(body) =
                load_selected_entry_off_runtime(file_path.clone(), max_file_size).await?
            else {
                continue;
            };

            let display_path = display_path(file_path, root_path, use_relative_paths)?;
            document.push_file(&display_path, &body);
            observer.file_appended(file_path);
        }
        Ok(document.finish())
    }

    /// Retrieves a truncated preview of a text file's content.
    ///
    /// Shows up to `max_lines` lines. Identifies directories and binary files. The text
    /// is decoded by the same rule as for the generated output (see [`decode_text`]),
    /// so a file in a legacy encoding looks the same in the preview and in the document.
    ///
    /// Only the beginning of the file is read: about as much as the lines shown take,
    /// and never more than [`PREVIEW_MAX_BYTES`], however large the file is.
    pub fn get_file_preview(file_path: &Path, max_lines: usize) -> Result<String, CoreError> {
        if file_path.is_dir() {
            return Ok("[DIRECTORY]".to_string());
        }
        let io_error = |e: io::Error| CoreError::Io(e.to_string(), file_path.to_path_buf());

        // Use a utility to quickly check if the file is likely text-based.
        if !is_text_file(file_path)
            .map_err(|e| CoreError::Io(e.to_string(), file_path.to_path_buf()))?
        {
            return Ok("[BINARY FILE]".to_string());
        }

        let file = fs::File::open(file_path).map_err(io_error)?;
        let (bytes, is_cut) =
            read_preview_prefix(file, max_lines, PREVIEW_MAX_BYTES).map_err(io_error)?;
        Ok(preview_of(file_path, bytes, is_cut, max_lines))
    }
}

/// Formats the preview of a file from the beginning of its content (pure, no IO).
/// `is_cut` tells that the file goes on after `bytes`.
fn preview_of(file_path: &Path, bytes: Vec<u8>, is_cut: bool, max_lines: usize) -> String {
    let Some(text) = decode_text(file_path, bytes) else {
        return "[BINARY FILE]".to_string();
    };

    let mut preview = String::new();
    let mut lines = text.lines();
    for line in lines.by_ref().take(max_lines) {
        preview.push_str(line);
        preview.push('\n');
    }
    if is_cut || lines.next().is_some() {
        preview.push_str("...\n[Preview truncated]");
    }
    preview
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

    /// Runs a generation with the default size limit and without an observer.
    async fn generate(
        selected_files: &[PathBuf],
        root_path: &Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<String, CoreError> {
        FileHandler::generate_concatenated_content_simple(
            request(
                selected_files,
                root_path,
                include_tree,
                items_for_tree,
                tree_ignore_patterns,
                use_relative_paths,
            ),
            cancel_flag,
            &(),
        )
        .await
    }

    /// A request with the default size limit.
    fn request<'a>(
        selected_files: &'a [PathBuf],
        root_path: &'a Path,
        include_tree: bool,
        items_for_tree: Vec<FileItem>,
        tree_ignore_patterns: HashSet<String>,
        use_relative_paths: bool,
    ) -> GenerationRequest<'a> {
        GenerationRequest {
            selected_files,
            root_path,
            include_tree,
            items_for_tree,
            tree_ignore_patterns,
            use_relative_paths,
            max_file_size: MAX_FILE_SIZE,
        }
    }

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

        let content = generate(
            &selected_files,
            &root,
            true,
            all_items,
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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
        let content = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            false,
            Arc::new(AtomicBool::new(false)),
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

        let content = generate(
            &selected_files,
            &root,
            true,
            all_items,
            tree_ignore_patterns,
            true,
            Arc::new(AtomicBool::new(false)),
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
        let content = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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
        generate(
            selected_files,
            root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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

        let content = generate(
            &selected_files,
            &root,
            true,   // include_tree
            vec![], // no items for tree either
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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

        let content = generate(
            &selected_files,
            &root,
            false, // include_tree
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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
        let content = generate(
            &selected_files,
            &root,
            true, // Wir wollen den Baum sehen, um das Rendering zu prüfen
            all_items,
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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
        let content = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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

        let content = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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

        let content = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(false)),
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

    /// Requests cancellation as soon as the first file has been appended.
    struct CancelAfterFirstFile {
        cancel_flag: Arc<AtomicBool>,
        appended: std::sync::Mutex<Vec<PathBuf>>,
    }

    impl GenerationObserver for CancelAfterFirstFile {
        fn file_appended(&self, file_path: &Path) {
            self.appended.lock().unwrap().push(file_path.to_path_buf());
            self.cancel_flag.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn generate_content_can_be_cancelled_deterministically() {
        let (_dir, root) = setup_test_environment();
        let selected_files = vec![root.join("src/main.rs"), root.join("README.md")];
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let observer = CancelAfterFirstFile {
            cancel_flag: cancel_flag.clone(),
            appended: Default::default(),
        };

        let result = FileHandler::generate_concatenated_content_simple(
            request(&selected_files, &root, false, vec![], HashSet::new(), true),
            cancel_flag,
            &observer,
        )
        .await;

        assert!(
            matches!(result, Err(CoreError::Cancelled)),
            "The generation should have been cancelled between the two files, got {result:?}"
        );
        assert_eq!(
            *observer.appended.lock().unwrap(),
            vec![root.join("src/main.rs")],
            "Only the first file may have been processed"
        );
    }

    #[tokio::test]
    async fn observer_is_told_about_every_appended_file_but_not_about_directories() {
        let (_dir, root) = setup_test_environment();
        let selected_files = vec![
            root.join("src/main.rs"),
            root.join("src"),
            root.join("README.md"),
        ];
        let observer = CancelAfterFirstFile {
            cancel_flag: Arc::new(AtomicBool::new(false)),
            appended: Default::default(),
        };

        FileHandler::generate_concatenated_content_simple(
            request(&selected_files, &root, false, vec![], HashSet::new(), true),
            Arc::new(AtomicBool::new(false)),
            &observer,
        )
        .await
        .unwrap();

        assert_eq!(
            *observer.appended.lock().unwrap(),
            vec![root.join("src/main.rs"), root.join("README.md")]
        );
    }

    #[tokio::test]
    async fn size_limit_of_the_request_decides_which_files_are_inlined() {
        let (_dir, root) = setup_test_environment();
        let small = root.join("small.txt");
        let large = root.join("large.txt");
        fs::write(&small, "12345").unwrap();
        fs::write(&large, "123456").unwrap();
        let selected_files = vec![small, large];
        let mut request = request(&selected_files, &root, false, vec![], HashSet::new(), true);
        request.max_file_size = 5;

        let content = FileHandler::generate_concatenated_content_simple(
            request,
            Arc::new(AtomicBool::new(false)),
            &(),
        )
        .await
        .unwrap();

        assert_eq!(file_block_body(&content, "small.txt"), "12345\n");
        assert_eq!(
            file_block_body(&content, "large.txt"),
            "[FILE TOO LARGE: 6 bytes - CONTENT SKIPPED]\n"
        );
    }

    #[tokio::test]
    async fn generate_content_cancelled_before_start_returns_cancelled() {
        let (_dir, root) = setup_test_environment();
        // Even an unreadable file must not mask a cancellation.
        let selected_files = vec![root.join("does_not_exist.txt")];

        let result = generate(
            &selected_files,
            &root,
            false,
            vec![],
            HashSet::new(),
            true,
            Arc::new(AtomicBool::new(true)),
        )
        .await;

        assert!(matches!(result, Err(CoreError::Cancelled)));
    }

    /// A line that is not valid UTF-8 is shown the way the generation embeds it:
    /// decoded as Windows-1252 instead of being replaced by an error marker.
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

        // The snapshot shows the second line with 0x99 decoded as Windows-1252.
        insta::assert_snapshot!(preview);
    }

    #[tokio::test]
    async fn preview_and_generation_show_the_same_text_for_a_latin1_file() {
        let (_dir, root) = setup_test_environment();
        let path = root.join("latin1.txt");
        fs::write(&path, b"Gr\xF6\xDFe: 5 \x80\r\nzweite Zeile\n").unwrap();

        let preview = FileHandler::get_file_preview(&path, 10).unwrap();
        let generated = generate_plain(std::slice::from_ref(&path), &root)
            .await
            .unwrap();

        assert_eq!(preview, "Größe: 5 €\nzweite Zeile\n");
        assert_eq!(
            file_block_body(&generated, "latin1.txt"),
            "Größe: 5 €\r\nzweite Zeile\n"
        );
    }

    #[test]
    fn preview_of_binary_looking_text_extension_is_reported_as_binary() {
        let (_dir, root) = setup_test_environment();
        let path = root.join("data.txt");
        fs::write(&path, b"\x00\x01\xFF\xFE\x00\x00").unwrap();

        assert_eq!(
            FileHandler::get_file_preview(&path, 10).unwrap(),
            "[BINARY FILE]"
        );
    }

    #[test]
    fn preview_marks_truncation_only_when_lines_are_left_out() {
        let (_dir, root) = setup_test_environment();
        let path = root.join("three.txt");
        fs::write(&path, "one\ntwo\nthree").unwrap();

        assert_eq!(
            FileHandler::get_file_preview(&path, 3).unwrap(),
            "one\ntwo\nthree\n"
        );
        assert_eq!(
            FileHandler::get_file_preview(&path, 2).unwrap(),
            "one\ntwo\n...\n[Preview truncated]"
        );
    }

    /// A reader that counts the bytes handed out, to observe how much is read.
    struct CountingReader<R> {
        inner: R,
        bytes_read: usize,
    }

    impl<R: Read> Read for CountingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let read = self.inner.read(buffer)?;
            self.bytes_read += read;
            Ok(read)
        }
    }

    /// An input that repeats `pattern` and never ends: reading it to the end is not
    /// an option.
    fn endless(pattern: &'static [u8]) -> CountingReader<impl Read> {
        CountingReader {
            inner: Repeating {
                pattern,
                position: 0,
            },
            bytes_read: 0,
        }
    }

    struct Repeating {
        pattern: &'static [u8],
        position: usize,
    }

    impl Read for Repeating {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            for byte in buffer.iter_mut() {
                *byte = self.pattern[self.position];
                self.position = (self.position + 1) % self.pattern.len();
            }
            Ok(buffer.len())
        }
    }

    #[test]
    fn preview_prefix_stops_reading_once_it_has_the_lines() {
        let mut reader = endless(b"0123456789abcde\n");

        let (bytes, is_cut) = read_preview_prefix(&mut reader, 1500, PREVIEW_MAX_BYTES).unwrap();

        assert!(is_cut);
        let line_ends = bytes.iter().filter(|&&byte| byte == b'\n').count();
        assert!(line_ends > 1500, "only {line_ends} lines were read");
        // 1501 lines of 16 bytes, rounded up to the chunks they were read in.
        assert_eq!(reader.bytes_read, bytes.len());
        assert!(
            reader.bytes_read <= 1501 * 16 + PREVIEW_CHUNK_SIZE,
            "{} bytes were read",
            reader.bytes_read
        );
    }

    #[test]
    fn preview_prefix_is_capped_for_input_without_line_ends() {
        let mut reader = endless(b"x");

        let (bytes, is_cut) = read_preview_prefix(&mut reader, 1500, 100_000).unwrap();

        assert!(is_cut);
        assert_eq!(bytes.len(), 100_000);
        // One byte beyond the cap is what tells that the input goes on.
        assert_eq!(reader.bytes_read, 100_001);
    }

    #[test]
    fn preview_prefix_of_short_input_is_complete() {
        let read = |input: &'static [u8], max_lines, max_bytes| {
            read_preview_prefix(input, max_lines, max_bytes).unwrap()
        };

        assert_eq!(read(b"", 10, 100), (b"".to_vec(), false));
        assert_eq!(read(b"one\ntwo", 10, 100), (b"one\ntwo".to_vec(), false));
        // Exactly as many lines, or bytes, as allowed: nothing is left out.
        assert_eq!(read(b"one\ntwo\n", 2, 100), (b"one\ntwo\n".to_vec(), false));
        assert_eq!(read(b"one\ntwo", 10, 7), (b"one\ntwo".to_vec(), false));
        assert_eq!(read(b"one\ntwo", 10, 6), (b"one\ntw".to_vec(), true));
    }

    #[test]
    fn preview_prefix_is_never_cut_inside_a_utf8_character() {
        let read =
            |input: &'static [u8], max_bytes| read_preview_prefix(input, 10, max_bytes).unwrap();
        let umlaut = "aö".as_bytes(); // 'ö' takes two bytes.
        let crab = "a🦀b".as_bytes(); // '🦀' takes four bytes.

        assert_eq!(read(umlaut, 3), (b"a\xC3\xB6".to_vec(), false));
        assert_eq!(read(umlaut, 2), (b"a".to_vec(), true));
        assert_eq!(read(umlaut, 1), (b"a".to_vec(), true));
        for max_bytes in 1..=4 {
            assert_eq!(read(crab, max_bytes), (b"a".to_vec(), true));
        }
        assert_eq!(read(crab, 5), ("a🦀".as_bytes().to_vec(), true));
    }

    #[test]
    fn preview_prefix_keeps_bytes_that_are_invalid_rather_than_incomplete() {
        assert_eq!(
            read_preview_prefix(&b"a\xF6bc"[..], 10, 3).unwrap(),
            (b"a\xF6b".to_vec(), true)
        );
        // A lone continuation byte at the end is invalid, not incomplete.
        assert_eq!(
            read_preview_prefix(&b"ab\x99c"[..], 10, 3).unwrap(),
            (b"ab\x99".to_vec(), true)
        );
    }

    /// The preview of a file that is larger than what a preview reads at most.
    #[test]
    fn preview_of_a_huge_file_shows_its_beginning() {
        let (_dir, root) = setup_test_environment();

        // Short lines: the preview has its lines long before the cap.
        let many_lines = root.join("many_lines.txt");
        let line = b"a line of text\n";
        fs::write(&many_lines, line.repeat(2 * PREVIEW_MAX_BYTES / line.len())).unwrap();
        let preview = FileHandler::get_file_preview(&many_lines, 3).unwrap();
        assert_eq!(
            preview,
            "a line of text\na line of text\na line of text\n...\n[Preview truncated]"
        );

        // One line longer than the cap: the preview ends at the cap.
        let one_line = root.join("one_line.txt");
        fs::write(&one_line, vec![b'x'; PREVIEW_MAX_BYTES + 10]).unwrap();
        let preview = FileHandler::get_file_preview(&one_line, 3).unwrap();
        let marker = "\n...\n[Preview truncated]";
        assert_eq!(preview.len(), PREVIEW_MAX_BYTES + marker.len());
        assert!(preview.ends_with(marker));
    }

    /// The regression this rule exists for: one stray byte must not make the whole
    /// file be read as Windows-1252, which would garble every umlaut of it.
    #[test]
    fn utf8_text_with_a_stray_byte_keeps_its_characters() {
        let mut bytes = "Größe: Können wir? Ja.\nzweite Zeile -> "
            .as_bytes()
            .to_vec();
        bytes.push(0x99);
        bytes.extend_from_slice(" so good. Ärger über Öl.\n".as_bytes());

        assert_eq!(
            decode_text(Path::new("mixed.txt"), bytes).unwrap(),
            "Größe: Können wir? Ja.\nzweite Zeile -> \u{FFFD} so good. Ärger über Öl.\n"
        );
    }

    #[test]
    fn encoding_of_text_that_is_not_valid_utf8_is_guessed_by_its_sequences() {
        use TextEncoding::{Utf8, Windows1252};
        let cases: [(&str, &[u8], TextEncoding); 7] = [
            ("pure Latin-1", b"Gr\xF6\xDFe: K\xF6nnen wir?", Windows1252),
            (
                "ASCII with one stray byte",
                b"not -> \x99 so good",
                Windows1252,
            ),
            (
                "Windows-1252 punctuation",
                b"\x93quoted\x94 \x85",
                Windows1252,
            ),
            (
                "UTF-8 with one stray byte",
                b"Gr\xC3\xB6\xC3\x9Fe \x99",
                Utf8,
            ),
            (
                "UTF-8 cut inside a character",
                b"\xC3\xA4\xC3\xB6 \xE2\x82",
                Utf8,
            ),
            ("as many valid as invalid", b"\xC3\xA4 \xFF", Utf8),
            (
                "Latin-1 with a coincidental UTF-8 pair",
                b"\xC3\xA9 K\xF6nnen Gr\xF6\xDFe",
                Windows1252,
            ),
        ];

        for (name, bytes, expected) in cases {
            assert_eq!(guess_encoding(bytes), expected, "{name}");
        }
    }

    #[tokio::test]
    async fn preview_and_generation_show_the_same_text_for_utf8_with_a_stray_byte() {
        let (_dir, root) = setup_test_environment();
        let path = root.join("mixed.txt");
        fs::write(&path, b"Gr\xC3\xB6\xC3\x9Fe\nkaputt: \x99\n\xC3\x84rger\n").unwrap();

        let preview = FileHandler::get_file_preview(&path, 10).unwrap();
        let generated = generate_plain(std::slice::from_ref(&path), &root)
            .await
            .unwrap();

        let expected = "Größe\nkaputt: \u{FFFD}\nÄrger\n";
        assert_eq!(preview, expected);
        assert_eq!(file_block_body(&generated, "mixed.txt"), expected);
    }
}
