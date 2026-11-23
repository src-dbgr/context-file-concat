//! Provides logic for filtering and searching lists of `FileItem`s.

use super::file_handler::decode_text;
use super::{FileItem, MAX_FILE_SIZE};
use rayon::prelude::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The single definition of a filter text that is *not set*: it is empty or consists
/// of whitespace only.
///
/// This holds for the filename query, the extension filter and the content query
/// alike. A blank text is ignored entirely: it does not filter, it does not start a
/// content search and it does not count as an active filter. A text that is not blank
/// is used exactly as typed, i.e. it is *not* trimmed for matching.
pub fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// The extension filter value that stands for "files without an extension".
const NO_EXTENSION: &str = "no extension";

/// The single definition of what it means for a path to match the filename query and
/// the extension filter.
///
/// Everything that needs this decision (filtering the file list, highlighting matches
/// in the tree, expanding the directories of matches) asks a `FileNameMatcher`, so the
/// answers cannot drift apart. The matcher is normalised once on construction and can
/// then be applied to any number of paths.
///
/// # Rules
///
/// **Filename query**
/// - A [blank](is_blank) query (empty or whitespace only) is *inactive*.
/// - An active query is used as typed: it is not trimmed, so surrounding blanks have
///   to occur in the file name as well.
/// - The query must be contained in the *file name* (the last path component), never in
///   the directories leading to it.
/// - File names that are not valid Unicode are compared in their lossy form (invalid
///   sequences become `U+FFFD`), i.e. exactly as they are displayed.
/// - Case-insensitive matching compares the Unicode lowercase forms of both sides.
///
/// **Extension filter**
/// - A [blank](is_blank) filter (empty or whitespace only) is *inactive*.
/// - An active filter is used as typed (not trimmed).
/// - One leading dot of the filter is ignored (`.rs` equals `rs`).
/// - A path *with* an extension (as defined by [`Path::extension`]) matches if the
///   extension equals the filter, ignoring ASCII case. Only the last extension counts
///   (`archive.tar.gz` has the extension `gz`).
/// - A path *without* an extension matches only the filter `no extension` (in any ASCII
///   case). This includes dotfiles such as `.gitignore`, whose name is not an extension.
/// - An extension that is not valid Unicode cannot be named by a filter; such a path is
///   treated like one without an extension.
///
/// The two criteria can be combined in two ways, depending on what the caller needs:
/// [`matches_all`](Self::matches_all) to narrow a list down and
/// [`matches_any`](Self::matches_any) to point out matches.
#[derive(Debug, Clone)]
pub struct FileNameMatcher {
    name: Option<NameQuery>,
    extension: Option<ExtensionFilter>,
}

/// An active filename query.
#[derive(Debug, Clone)]
enum NameQuery {
    CaseSensitive(String),
    /// Holds the lowercased query.
    CaseInsensitive(String),
}

impl NameQuery {
    fn matches(&self, path: &Path) -> bool {
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        match self {
            Self::CaseSensitive(query) => file_name.contains(query.as_str()),
            Self::CaseInsensitive(query) => file_name.to_lowercase().contains(query.as_str()),
        }
    }
}

/// An active extension filter.
#[derive(Debug, Clone)]
struct ExtensionFilter {
    /// The extension to look for, without the leading dot.
    extension: String,
    /// Whether paths without an extension match.
    matches_missing_extension: bool,
}

impl ExtensionFilter {
    fn matches(&self, path: &Path) -> bool {
        match path.extension().and_then(|extension| extension.to_str()) {
            Some(extension) => extension.eq_ignore_ascii_case(&self.extension),
            None => self.matches_missing_extension,
        }
    }
}

impl FileNameMatcher {
    /// Creates a matcher for the given filename query and extension filter.
    pub fn new(query: &str, extension_filter: &str, case_sensitive: bool) -> Self {
        let name = (!is_blank(query)).then(|| {
            if case_sensitive {
                NameQuery::CaseSensitive(query.to_string())
            } else {
                NameQuery::CaseInsensitive(query.to_lowercase())
            }
        });
        let extension = (!is_blank(extension_filter)).then(|| ExtensionFilter {
            extension: extension_filter
                .strip_prefix('.')
                .unwrap_or(extension_filter)
                .to_string(),
            matches_missing_extension: extension_filter.eq_ignore_ascii_case(NO_EXTENSION),
        });
        Self { name, extension }
    }

    /// `true` if a filename query or an extension filter is set.
    pub fn is_active(&self) -> bool {
        self.name.is_some() || self.extension.is_some()
    }

    /// `true` if the path satisfies *every* active criterion.
    ///
    /// This is the rule for narrowing a list down. Without an active criterion every
    /// path matches.
    pub fn matches_all(&self, path: &Path) -> bool {
        let name_ok = match &self.name {
            Some(name) => name.matches(path),
            None => true,
        };
        let extension_ok = match &self.extension {
            Some(extension) => extension.matches(path),
            None => true,
        };
        name_ok && extension_ok
    }

    /// `true` if the path satisfies *at least one* active criterion.
    ///
    /// This is the rule for pointing out matches. Without an active criterion no path
    /// matches.
    pub fn matches_any(&self, path: &Path) -> bool {
        self.name.as_ref().is_some_and(|name| name.matches(path))
            || self
                .extension
                .as_ref()
                .is_some_and(|extension| extension.matches(path))
    }
}

/// A utility struct for searching and filtering file lists.
///
/// This struct is stateless and provides methods as associated functions.
pub struct SearchEngine;

impl SearchEngine {
    /// Recursively removes directories from a list that do not contain any files.
    ///
    /// This implementation is optimized to run in near-linear time O(N).
    /// It now takes a reference to `all_project_files` to correctly determine
    /// which directories are genuinely empty, regardless of any active filters.
    pub fn remove_empty_directories(
        files_to_filter: Vec<FileItem>,
        all_project_files: &[FileItem],
        dirs_to_preserve: &HashSet<PathBuf>,
    ) -> (Vec<FileItem>, HashSet<PathBuf>) {
        if files_to_filter.is_empty() {
            return (files_to_filter, HashSet::new());
        }

        // 1. Build a set of all non-directory paths from the complete file list.
        // This is the crucial reference to determine which directories are essential.
        let all_file_paths: HashSet<_> = all_project_files
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();

        // 2. Build a set of all directories that are ancestors of at least one file.
        let mut essential_dirs = HashSet::new();
        for file_path in &all_file_paths {
            let mut current = file_path.parent();
            while let Some(parent) = current {
                // No need to check if parent is in a directory list; any parent of a file is essential.
                if essential_dirs.insert(parent.to_path_buf()) {
                    current = parent.parent();
                } else {
                    // Stop if we've already processed this parent path.
                    break;
                }
            }
        }

        // 3. Filter the original list: keep all files and all essential/preserved directories.
        let mut removed_dirs_set = HashSet::new();
        let final_list: Vec<FileItem> = files_to_filter
            .into_iter()
            .filter(|item| {
                if item.is_directory {
                    // Keep if it's essential (contains files somewhere in its subtree) OR
                    // if it's explicitly preserved (e.g., it's not fully loaded yet).
                    if essential_dirs.contains(&item.path) || dirs_to_preserve.contains(&item.path)
                    {
                        true // Keep this directory
                    } else {
                        removed_dirs_set.insert(item.path.clone());
                        false // Remove this directory
                    }
                } else {
                    true // Always keep files
                }
            })
            .collect();

        (final_list, removed_dirs_set)
    }
}

/// Describes one content search: what to look for and under which conditions.
///
/// This is the complete input of a search besides the files themselves, so it can be
/// handed through layers as one value.
#[derive(Debug, Clone)]
pub struct ContentSearchRequest {
    /// The text to look for. Used as it is, i.e. not trimmed. Whether a search takes
    /// place at all ([`is_blank`]) is up to the caller.
    pub query: String,
    /// `true` if the case of the query has to match.
    pub case_sensitive: bool,
    /// Files larger than this many bytes are not searched.
    pub max_file_size: u64,
    /// Raised by whoever started the search once nobody waits for its result any more.
    pub cancel_flag: Arc<AtomicBool>,
}

#[cfg(test)]
impl ContentSearchRequest {
    /// A request with the size limit of the core that is never cancelled.
    pub(crate) fn new(query: impl Into<String>, case_sensitive: bool) -> Self {
        Self {
            query: query.into(),
            case_sensitive,
            max_file_size: MAX_FILE_SIZE,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Searches the *contents* of files for a query string.
///
/// The query is normalised once on construction, so the same instance can be applied
/// to any number of files. All methods perform blocking file I/O; async callers must
/// run them on a blocking thread.
///
/// A search can be told to stop through a [cancellation flag](Self::with_cancel_flag).
pub struct ContentSearch {
    /// The query, already lowercased if the search is case-insensitive.
    needle: String,
    case_sensitive: bool,
    max_file_size: u64,
    /// Raised by whoever started the search once nobody waits for its result any more.
    cancel_flag: Arc<AtomicBool>,
}

impl From<ContentSearchRequest> for ContentSearch {
    fn from(request: ContentSearchRequest) -> Self {
        Self::new(&request.query, request.case_sensitive)
            .with_max_file_size(request.max_file_size)
            .with_cancel_flag(request.cancel_flag)
    }
}

impl ContentSearch {
    /// Creates a search for `query`, skipping files larger than the scanner's size limit.
    pub fn new(query: &str, case_sensitive: bool) -> Self {
        let needle = if case_sensitive {
            query.to_string()
        } else {
            query.to_lowercase()
        };
        Self {
            needle,
            case_sensitive,
            max_file_size: MAX_FILE_SIZE,
            cancel_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Makes the search stop as soon as `cancel_flag` is raised.
    ///
    /// The flag is checked before every file, so a cancelled search finishes the files
    /// it is reading at that moment and does not open any further one.
    pub fn with_cancel_flag(mut self, cancel_flag: Arc<AtomicBool>) -> Self {
        self.cancel_flag = cancel_flag;
        self
    }

    /// Replaces the size limit: files larger than `max_file_size` bytes are not
    /// searched. The limit may be stricter or more generous than the default.
    pub fn with_max_file_size(mut self, max_file_size: u64) -> Self {
        self.max_file_size = max_file_size;
        self
    }

    /// Returns the paths of all `files` whose content contains the query.
    ///
    /// Directories, binary files, files above the size limit and files that cannot be
    /// read are skipped silently. Text is decoded exactly as for the generated output,
    /// so files in a legacy encoding (e.g. Latin-1) are searched as well.
    ///
    /// If the search is cancelled, the result is incomplete (it holds the matches found
    /// up to then) and must be discarded by the caller.
    pub fn matching_paths(&self, files: &[FileItem]) -> HashSet<PathBuf> {
        self.matching_paths_by(files, |item| self.matches_file(item))
    }

    /// Collects the paths of the files accepted by `matches_file`, which is no longer
    /// asked once the search has been cancelled.
    fn matching_paths_by(
        &self,
        files: &[FileItem],
        matches_file: impl Fn(&FileItem) -> bool + Sync,
    ) -> HashSet<PathBuf> {
        files
            .par_iter()
            .filter(|item| !self.is_cancelled() && matches_file(item))
            .map(|item| item.path.clone())
            .collect()
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::Relaxed)
    }

    fn matches_file(&self, item: &FileItem) -> bool {
        if item.is_directory || item.is_binary || !self.is_within_size_limit(&item.path) {
            return false;
        }
        std::fs::read(&item.path)
            .ok()
            .and_then(|bytes| decode_text(&item.path, bytes))
            .is_some_and(|content| self.matches_content(&content))
    }

    /// Checks the size on disk rather than `FileItem::size`, which may be outdated.
    fn is_within_size_limit(&self, path: &Path) -> bool {
        std::fs::metadata(path)
            .map(|metadata| metadata.len() <= self.max_file_size)
            .unwrap_or(false)
    }

    fn matches_content(&self, content: &str) -> bool {
        if self.case_sensitive {
            content.contains(&self.needle)
        } else {
            content.to_lowercase().contains(&self.needle)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Writes `content` to `name` inside `dir` and returns a matching text `FileItem`.
    fn write_text_file(dir: &Path, name: &str, content: &str) -> FileItem {
        let path = dir.join(name);
        std::fs::write(&path, content).unwrap();
        FileItem {
            path,
            size: content.len() as u64,
            ..Default::default()
        }
    }

    #[test]
    fn test_content_search_case_sensitive_requires_exact_case() {
        let dir = tempdir().unwrap();
        let exact = write_text_file(dir.path(), "exact.txt", "find the MagicWord here");
        let other_case = write_text_file(dir.path(), "other.txt", "find the magicword here");
        let files = vec![exact.clone(), other_case];

        let result = ContentSearch::new("MagicWord", true).matching_paths(&files);

        assert_eq!(result, HashSet::from([exact.path]));
    }

    #[test]
    fn test_content_search_case_insensitive_ignores_case_of_query_and_content() {
        let dir = tempdir().unwrap();
        let upper = write_text_file(dir.path(), "upper.txt", "find the MAGICWORD here");
        let mixed = write_text_file(dir.path(), "mixed.txt", "find the MagicWord here");
        let miss = write_text_file(dir.path(), "miss.txt", "nothing to see");
        let files = vec![upper.clone(), mixed.clone(), miss];

        let result = ContentSearch::new("mAgIcWoRd", false).matching_paths(&files);

        assert_eq!(result, HashSet::from([upper.path, mixed.path]));
    }

    #[test]
    fn test_content_search_skips_files_above_size_limit() {
        let dir = tempdir().unwrap();
        let at_limit = write_text_file(dir.path(), "at_limit.txt", "needle 890");
        let too_large = write_text_file(dir.path(), "too_large.txt", "needle 8901");
        let files = vec![at_limit.clone(), too_large];

        let result = ContentSearch::new("needle", true)
            .with_max_file_size(10)
            .matching_paths(&files);

        assert_eq!(result, HashSet::from([at_limit.path]));
    }

    #[test]
    fn test_content_search_uses_scanner_size_limit_by_default() {
        let dir = tempdir().unwrap();
        let mut large = write_text_file(dir.path(), "large.txt", "needle");
        // Extend the file sparsely beyond the limit; it still starts with the query.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&large.path)
            .unwrap();
        file.set_len(MAX_FILE_SIZE + 1).unwrap();
        // A stale size from an earlier scan must not bypass the limit.
        large.size = 6;

        let result = ContentSearch::new("needle", true).matching_paths(&[large]);

        assert!(result.is_empty());
    }

    /// The default is not a ceiling: a more generous limit is honoured.
    #[test]
    fn test_content_search_honours_a_limit_above_the_default() {
        let dir = tempdir().unwrap();
        let size = MAX_FILE_SIZE + 1;
        let mut content = "needle".to_string();
        content.push_str(&" ".repeat(size as usize - content.len()));
        let large = write_text_file(dir.path(), "large.txt", &content);

        let with_default_limit =
            ContentSearch::new("needle", true).matching_paths(std::slice::from_ref(&large));
        let with_raised_limit = ContentSearch::new("needle", true)
            .with_max_file_size(size)
            .matching_paths(std::slice::from_ref(&large));

        assert!(with_default_limit.is_empty());
        assert_eq!(with_raised_limit, HashSet::from([large.path]));
    }

    #[test]
    fn test_content_search_request_defaults_to_the_limit_of_the_core() {
        let request = ContentSearchRequest::new("needle", true);

        assert_eq!(request.max_file_size, MAX_FILE_SIZE);
        assert!(!request.cancel_flag.load(Ordering::SeqCst));
    }

    /// A search built from a request takes over every part of it.
    #[test]
    fn test_content_search_from_request_applies_query_case_limit_and_flag() {
        let dir = tempdir().unwrap();
        let at_limit = write_text_file(dir.path(), "at_limit.txt", "NEEDLE 890");
        let too_large = write_text_file(dir.path(), "too_large.txt", "NEEDLE 8901");
        let other_case = write_text_file(dir.path(), "other_case.txt", "needle");
        let files = vec![at_limit.clone(), too_large, other_case.clone()];
        let request = |case_sensitive| ContentSearchRequest {
            max_file_size: 10,
            ..ContentSearchRequest::new("NEEDLE", case_sensitive)
        };

        let case_sensitive = ContentSearch::from(request(true)).matching_paths(&files);
        let case_insensitive = ContentSearch::from(request(false)).matching_paths(&files);
        let cancelled = request(false);
        cancelled.cancel_flag.store(true, Ordering::SeqCst);
        let cancelled = ContentSearch::from(cancelled).matching_paths(&files);

        assert_eq!(case_sensitive, HashSet::from([at_limit.path.clone()]));
        assert_eq!(
            case_insensitive,
            HashSet::from([at_limit.path, other_case.path])
        );
        assert!(cancelled.is_empty());
    }

    #[test]
    fn test_content_search_skips_binaries_and_directories() {
        let dir = tempdir().unwrap();
        let text = write_text_file(dir.path(), "text.txt", "needle");
        let binary = FileItem {
            is_binary: true,
            ..write_text_file(dir.path(), "data.bin", "needle")
        };
        let directory_path = dir.path().join("needle_dir");
        std::fs::create_dir(&directory_path).unwrap();
        let directory = FileItem {
            path: directory_path,
            is_directory: true,
            ..Default::default()
        };
        let files = vec![text.clone(), binary, directory];

        let result = ContentSearch::new("needle", false).matching_paths(&files);

        assert_eq!(result, HashSet::from([text.path]));
    }

    #[test]
    fn test_content_search_ignores_unreadable_files() {
        let dir = tempdir().unwrap();
        let missing = FileItem {
            path: dir.path().join("deleted_since_scan.txt"),
            ..Default::default()
        };
        // Not marked as binary by the scanner (e.g. changed since), but not text either.
        let undecodable_path = dir.path().join("utf16.txt");
        std::fs::write(&undecodable_path, b"\xff\xfen\x00").unwrap();
        let undecodable = FileItem {
            path: undecodable_path,
            ..Default::default()
        };

        let result = ContentSearch::new("n", false).matching_paths(&[missing, undecodable]);

        assert!(result.is_empty());
    }

    /// One stray byte does not make a UTF-8 file a legacy-encoded one: its umlauts
    /// are still found.
    #[test]
    fn test_content_search_finds_umlauts_in_utf8_file_with_a_stray_byte() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mixed.txt");
        std::fs::write(&path, b"Die Gr\xC3\xB6\xC3\x9Fe z\xC3\xA4hlt. \x99\n").unwrap();
        let files = vec![FileItem {
            path: path.clone(),
            ..Default::default()
        }];

        assert_eq!(
            ContentSearch::new("Größe zählt", true).matching_paths(&files),
            HashSet::from([path])
        );
    }

    #[test]
    fn test_content_search_finds_text_in_latin1_file() {
        let dir = tempdir().unwrap();
        let latin1_path = dir.path().join("legacy.txt");
        // "Die Größe zählt." encoded as Latin-1, which is not valid UTF-8.
        std::fs::write(&latin1_path, b"Die Gr\xf6\xdfe z\xe4hlt.\n").unwrap();
        let utf8_path = dir.path().join("modern.txt");
        std::fs::write(&utf8_path, "Die Größe zählt.\n").unwrap();
        let other_path = dir.path().join("other.txt");
        std::fs::write(&other_path, b"Gr\xfc\xdfe\n").unwrap();
        let files: Vec<FileItem> = [&latin1_path, &utf8_path, &other_path]
            .into_iter()
            .map(|path| FileItem {
                path: path.clone(),
                ..Default::default()
            })
            .collect();

        let expected = HashSet::from([latin1_path, utf8_path]);
        assert_eq!(
            ContentSearch::new("Größe", true).matching_paths(&files),
            expected
        );
        assert_eq!(
            ContentSearch::new("GRöße", false).matching_paths(&files),
            expected
        );
    }

    #[test]
    fn test_content_search_that_is_cancelled_up_front_reads_nothing() {
        let dir = tempdir().unwrap();
        let files = vec![
            write_text_file(dir.path(), "a.txt", "needle"),
            write_text_file(dir.path(), "b.txt", "needle"),
        ];
        let cancel_flag = Arc::new(AtomicBool::new(true));

        let result = ContentSearch::new("needle", true)
            .with_cancel_flag(cancel_flag)
            .matching_paths(&files);

        assert!(result.is_empty());
    }

    #[test]
    fn test_content_search_with_a_flag_that_is_never_raised_finds_everything() {
        let dir = tempdir().unwrap();
        let files = vec![
            write_text_file(dir.path(), "a.txt", "needle"),
            write_text_file(dir.path(), "b.txt", "nothing"),
        ];
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let result = ContentSearch::new("needle", true)
            .with_cancel_flag(cancel_flag)
            .matching_paths(&files);

        assert_eq!(result, HashSet::from([files[0].path.clone()]));
    }

    #[test]
    fn test_cancelled_content_search_stops_between_files() {
        use std::sync::atomic::AtomicUsize;

        let files: Vec<FileItem> = (0..200)
            .map(|i| FileItem {
                path: PathBuf::from(format!("/virtual/{i}.txt")),
                ..Default::default()
            })
            .collect();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let search = ContentSearch::new("needle", true).with_cancel_flag(cancel_flag.clone());
        let files_looked_at = AtomicUsize::new(0);

        // One worker thread makes "between files" deterministic: the search is
        // superseded while it looks at the third file.
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let result = pool.install(|| {
            search.matching_paths_by(&files, |_| {
                if files_looked_at.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
                    cancel_flag.store(true, Ordering::SeqCst);
                }
                true
            })
        });

        assert_eq!(
            files_looked_at.load(Ordering::SeqCst),
            3,
            "No file may be looked at after the cancellation"
        );
        assert_eq!(result.len(), 3, "The file being read is still finished");
    }

    #[test]
    fn test_is_blank_is_true_for_empty_and_whitespace_only_texts() {
        for blank in ["", " ", "   ", "\t", " \n\t "] {
            assert!(is_blank(blank), "{blank:?}");
        }
        for set in ["a", " a", "a ", " a b "] {
            assert!(!is_blank(set), "{set:?}");
        }
    }

    // --- FileNameMatcher -----------------------------------------------------------

    /// How a query/filter combination is expected to treat one path.
    struct Case {
        query: &'static str,
        extension: &'static str,
        case_sensitive: bool,
        path: &'static str,
        /// Expected result of `matches_all` (filtering).
        all: bool,
        /// Expected result of `matches_any` (highlighting).
        any: bool,
    }

    const fn case(
        query: &'static str,
        extension: &'static str,
        case_sensitive: bool,
        path: &'static str,
        all: bool,
        any: bool,
    ) -> Case {
        Case {
            query,
            extension,
            case_sensitive,
            path,
            all,
            any,
        }
    }

    #[rustfmt::skip]
    const CASES: &[Case] = &[
        // Nothing active: filtering lets everything through, nothing is pointed out.
        case("", "", false, "src/main.rs", true, false),
        case("", "", true, "Makefile", true, false),

        // Filename query: contained in the file name.
        case("main", "", false, "src/main.rs", true, true),
        case("ain.r", "", false, "src/main.rs", true, true),
        case("lib", "", false, "src/main.rs", false, false),
        // ... but never in the directories leading to it.
        case("src", "", false, "src/main.rs", false, false),
        case("src", "", false, "src", true, true),

        // Case sensitivity of the query.
        case("README", "", true, "docs/README.md", true, true),
        case("readme", "", true, "docs/README.md", false, false),
        case("readme", "", false, "docs/README.md", true, true),
        case("README", "", false, "docs/readme.md", true, true),
        // Case-insensitive means Unicode lowercase, not just ASCII.
        case("ÄRGER", "", false, "ärger.txt", true, true),
        case("ÄRGER", "", true, "ärger.txt", false, false),

        // A blank query is inactive: it narrows nothing down and points nothing out.
        case(" ", "", false, "my file.txt", true, false),
        case(" \t ", "", false, "myfile.txt", true, false),
        // An active query is not trimmed.
        case("y f", "", false, "my file.txt", true, true),
        case("main ", "", false, "main.rs", false, false),
        case(" file", "", false, "my file.txt", true, true),
        case(" file", "", false, "file.txt", false, false),
        // A blank extension filter is inactive as well.
        case("", " ", false, "src/main.rs", true, false),
        case("main", " ", false, "src/main.rs", true, true),
        case("main", " ", false, "src/lib.rs", false, false),

        // Extension filter: equal to the extension, ignoring ASCII case.
        case("", "rs", false, "src/main.rs", true, true),
        case("", "RS", false, "src/main.rs", true, true),
        case("", "rs", false, "src/MAIN.RS", true, true),
        // The case-sensitivity setting only concerns the query.
        case("", "RS", true, "src/main.rs", true, true),
        case("", "rs", false, "src/main.rss", false, false),
        case("", "r", false, "src/main.rs", false, false),
        // One leading dot of the filter is ignored.
        case("", ".rs", false, "src/main.rs", true, true),
        case("", "..rs", false, "src/main.rs", false, false),
        // Only the last extension counts.
        case("", "gz", false, "archive.tar.gz", true, true),
        case("", "tar", false, "archive.tar.gz", false, false),
        case("", "tar.gz", false, "archive.tar.gz", false, false),

        // Files without an extension.
        case("", "no extension", false, "Makefile", true, true),
        case("", "No Extension", false, "Makefile", true, true),
        case("", "NO EXTENSION", true, "Makefile", true, true),
        case("", "no extension", false, ".gitignore", true, true),
        case("", "no extension", false, "src/main.rs", false, false),
        case("", "no extension", false, "archive.tar.gz", false, false),
        case("", "rs", false, "Makefile", false, false),
        case("", "gitignore", false, ".gitignore", false, false),
        // The special value is not trimmed and takes no leading dot.
        case("", " no extension", false, "Makefile", false, false),
        case("", ".no extension", false, "Makefile", false, false),

        // Both active: filtering needs both, highlighting needs one.
        case("main", "rs", false, "src/main.rs", true, true),
        case("main", "rs", false, "src/main.py", false, true),
        case("main", "rs", false, "src/lib.rs", false, true),
        case("main", "rs", false, "docs/README.md", false, false),
        case("make", "no extension", false, "Makefile", true, true),
        case("make", "no extension", true, "Makefile", false, true),
    ];

    #[test]
    fn test_file_name_matcher_rules() {
        for case in CASES {
            let matcher = FileNameMatcher::new(case.query, case.extension, case.case_sensitive);
            let path = Path::new(case.path);
            let context = format!(
                "query {:?}, extension {:?}, case_sensitive {}, path {:?}",
                case.query, case.extension, case.case_sensitive, case.path
            );

            assert_eq!(
                matcher.matches_all(path),
                case.all,
                "matches_all: {context}"
            );
            assert_eq!(
                matcher.matches_any(path),
                case.any,
                "matches_any: {context}"
            );
            assert_eq!(
                matcher.is_active(),
                !is_blank(case.query) || !is_blank(case.extension),
                "is_active: {context}"
            );
        }
    }

    #[test]
    #[cfg(unix)] // Builds file names from raw bytes (OsStrExt::from_bytes).
    fn test_file_name_matcher_compares_non_unicode_names_in_their_lossy_form() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        // "caf\xe9.txt": Latin-1 bytes, not valid UTF-8. Displayed as "caf\u{fffd}.txt".
        let latin1_name = Path::new(OsStr::from_bytes(b"dir/caf\xe9.txt"));
        // "data.\xff": the extension itself is not valid UTF-8.
        let broken_extension = Path::new(OsStr::from_bytes(b"dir/data.\xff"));

        let by_name = |query: &str| FileNameMatcher::new(query, "", false);
        assert!(by_name("caf").matches_all(latin1_name));
        assert!(by_name("CAF").matches_any(latin1_name));
        assert!(by_name("\u{fffd}.txt").matches_all(latin1_name));
        assert!(!by_name("café").matches_all(latin1_name));

        let by_extension = |filter: &str| FileNameMatcher::new("", filter, false);
        assert!(by_extension("txt").matches_all(latin1_name));
        // An extension that is not Unicode cannot be named; it counts as missing.
        assert!(by_extension("no extension").matches_all(broken_extension));
        assert!(!by_extension("\u{fffd}").matches_all(broken_extension));
    }
}
