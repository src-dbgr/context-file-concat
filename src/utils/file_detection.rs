use anyhow::Result;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::OnceLock;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt",
    "md",
    "markdown",
    "rst",
    "asciidoc",
    "adoc",
    "rs",
    "py",
    "js",
    "ts",
    "jsx",
    "tsx",
    "java",
    "c",
    "cpp",
    "cxx",
    "cc",
    "h",
    "hpp",
    "hxx",
    "go",
    "rb",
    "php",
    "swift",
    "kt",
    "kts",
    "scala",
    "clj",
    "cljs",
    "hs",
    "ml",
    "fs",
    "fsx",
    "html",
    "htm",
    "xml",
    "xhtml",
    "css",
    "scss",
    "sass",
    "less",
    "svg",
    "vue",
    "svelte",
    "json",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "config",
    "properties",
    "sql",
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "bat",
    "cmd",
    "dockerfile",
    "makefile",
    "cmake",
    "gradle",
    "maven",
    "pom",
    "build",
    "tex",
    "bib",
    "r",
    "m",
    "pl",
    "lua",
    "vim",
    "el",
    "lisp",
    "dart",
    "elm",
    "ex",
    "exs",
    "erl",
    "hrl",
    "nim",
    "crystal",
    "cr",
    "zig",
    "odin",
    "v",
    "log",
    "trace",
    "out",
    "err",
    "diff",
    "patch",
    "gitignore",
    "gitattributes",
    "editorconfig",
    "env",
    "example",
    "sample",
    "template",
    "spec",
    "test",
    "readme",
    "license",
    "changelog",
    "todo",
    "notes",
    "doc",
    "docs",
    "man",
    "help",
    "faq",
    "lock",
    "sum",
    "mod",
    "work",
    "pest",
    "ron",
    "map",
    "d.ts",
    "mjs",
    "cjs",
    "coffee",
    "litcoffee",
    "ls",
    "flow",
    "pegjs",
    "graphql",
    "gql",
    "prisma",
    "proto",
    "thrift",
    "avsc",
    "jsonl",
    "ndjson",
    "csv",
    "tsv",
    "psv",
    "ssv",
    "tab",
    "data",
    "idx",
    "org",
    "cls",
    "sty",
    "bst",
    "aux",
    "fdb_latexmk",
    "fls",
    "R",
    "Rmd",
    "Rnw",
    "jl",
    "ipynb",
    "pyx",
    "pxd",
    "pxi",
    "pyi",
    "gnumakefile",
    "containerfile",
    "vagrantfile",
    "rakefile",
    "gemfile",
    "guardfile",
    "procfile",
    "capfile",
    "berksfile",
    "jenkinsfile",
    "dangerfile",
    "fastfile",
    "appfile",
    "deliverfile",
    "snapfile",
    "ignore",
    "keep",
    "gitkeep",
    "npmignore",
    "dockerignore",
    "eslintrc",
    "babelrc",
    "browserslistrc",
    "nvmrc",
    "rvmrc",
    "rbenv-version",
    "ruby-version",
    "node-version",
    "wasm",
    "wat",
    "wit",
    "component",
    "lalrpop",
    "y",
    "l",
    "lex",
    "yacc",
    "capnp",
    "fbs",
    "schema",
    "avdl",
    "gn",
    "gni",
    "bp",
    "BUILD",
    "WORKSPACE",
    "bzl",
    "nix",
    "drv",
    "store-path",
    "dhall",
    "purescript",
    "purs",
    "roc",
    "gleam",
    "grain",
    "hx",
    "hxml",
    "moon",
    "just",
    "justfile",
    "task",
    "taskfile",
    "clang-format",
    "rustfmt",
    "modulemap",
    "def",
    "exports",
    "version",
    "in",
    "am",
    "ac",
    "m4",
    "ctest",
    "service",
    "socket",
    "timer",
    "mount",
    "desktop",
    "appdata",
    "metainfo",
];

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "ico", "webp", "tiff", "tif", "raw", "cr2", "nef", "orf",
    "dng", "heic", "heif", "avif", "jfif",
];

const BINARY_EXTENSIONS: &[&str] = &[
    "exe",
    "dll",
    "so",
    "dylib",
    "app",
    "deb",
    "rpm",
    "msi",
    "zip",
    "tar",
    "gz",
    "bz2",
    "7z",
    "rar",
    "jar",
    "war",
    "mp3",
    "mp4",
    "avi",
    "mkv",
    "mov",
    "wmv",
    "flv",
    "webm",
    "pdf",
    "docx",
    "xlsx",
    "pptx",
    "bin",
    "dat",
    "db",
    "sqlite",
    "sqlite3",
    "rlib",
    "rmeta",
    "d",
    "pdb",
    "ilk",
    "exp",
    "lib",
    "a",
    "obj",
    "o",
    "class",
    "pyc",
    "pyo",
    "__pycache__",
    "cache",
    "tmp",
    "temp",
    "swap",
    "bak",
    "backup",
    "fingerprint",
    "deps",
    "incremental",
    "crate",
    "gem",
    "whl",
    "egg",
    "snap",
    "flatpak",
];

static TEXT_EXT_SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();

static BINARY_EXT_SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();

static IMAGE_EXT_SET: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();

fn get_text_ext_set() -> &'static std::collections::HashSet<&'static str> {
    TEXT_EXT_SET.get_or_init(|| TEXT_EXTENSIONS.iter().copied().collect())
}

fn get_binary_ext_set() -> &'static std::collections::HashSet<&'static str> {
    BINARY_EXT_SET.get_or_init(|| BINARY_EXTENSIONS.iter().copied().collect())
}

fn get_image_ext_set() -> &'static std::collections::HashSet<&'static str> {
    IMAGE_EXT_SET.get_or_init(|| IMAGE_EXTENSIONS.iter().copied().collect())
}

const MAX_CONTENT_CHECK_SIZE: u64 = 20 * 1024 * 1024;

const CONTENT_CHECK_BUFFER_SIZE: usize = 1024;

/// Determines if a file is likely to be a text file.
pub fn is_text_file(path: &Path) -> Result<bool> {
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        if get_text_ext_set().contains(ext_lower.as_str()) {
            return Ok(true);
        }
        if get_binary_ext_set().contains(ext_lower.as_str()) {
            return Ok(false);
        }
        if get_image_ext_set().contains(ext_lower.as_str()) {
            return Ok(false);
        }
    }
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if metadata.len() > MAX_CONTENT_CHECK_SIZE {
                return Ok(false);
            }
            if metadata.len() == 0 {
                return Ok(true);
            }
        }
        Err(_) => return Ok(false),
    }
    check_file_content_optimized(path)
}

/// Determines if a file is an image file.
#[allow(dead_code)]
pub fn is_image_file(path: &Path) -> bool {
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        get_image_ext_set().contains(ext_lower.as_str()) || ext_lower == "svg"
    } else {
        false
    }
}

/// Checks the initial bytes of a file to guess if it's text.
fn check_file_content_optimized(path: &Path) -> Result<bool> {
    let mut buffer = [0u8; CONTENT_CHECK_BUFFER_SIZE];
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(CONTENT_CHECK_BUFFER_SIZE, file);
    let bytes_read = reader.read(&mut buffer)?;
    if bytes_read == 0 {
        return Ok(true);
    }
    if buffer[..bytes_read].contains(&0) {
        return Ok(false);
    }
    match std::str::from_utf8(&buffer[..bytes_read]) {
        Ok(_) => Ok(true),
        Err(_) => {
            // VET: Switched to a more robust heuristic. Instead of checking for printable
            // ASCII, we check for the absence of control characters (excluding whitespace).
            // This correctly handles legacy 8-bit encodings like Latin-1.
            let control_char_count = buffer[..bytes_read]
                .iter()
                .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
                .count();

            // If less than 5% of the bytes are weird control characters, we assume it's text.
            let ratio = control_char_count as f32 / bytes_read as f32;
            Ok(ratio < 0.05)
        }
    }
}

/// Classifies a batch of files as text or image.
#[allow(dead_code)]
pub fn batch_classify_files(paths: &[&Path]) -> Vec<(bool, bool)> {
    paths
        .iter()
        .map(|path| {
            let is_text = is_text_file(path).unwrap_or(false);
            let is_image = if !is_text { is_image_file(path) } else { false };
            (is_text, is_image)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Helper to create a test file with specific content in a temporary directory.
    fn create_test_file(dir: &TempDir, name: &str, content: &[u8]) -> std::path::PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, content).expect("Failed to write test file");
        path
    }

    #[test]
    fn test_is_text_by_known_text_extension() {
        let dir = TempDir::new().unwrap();
        let text_files = [
            "document.txt",
            "code.rs",
            "script.py",
            "config.YAML",
            "Makefile",
        ];
        for name in text_files {
            let path = create_test_file(&dir, name, b"some content");
            assert!(
                is_text_file(&path).unwrap(),
                "Expected '{}' to be a text file",
                name
            );
        }
    }

    #[test]
    fn test_is_not_text_by_known_binary_extension() {
        let dir = TempDir::new().unwrap();
        let bin_files = ["archive.zip", "program.EXE", "library.dll", "document.pdf"];
        for name in bin_files {
            let path = create_test_file(&dir, name, b"\xDE\xAD\xBE\xEF");
            assert!(
                !is_text_file(&path).unwrap(),
                "Expected '{}' to be a binary file",
                name
            );
        }
    }

    #[test]
    fn test_is_not_text_by_known_image_extension() {
        let dir = TempDir::new().unwrap();
        let img_files = ["photo.jpg", "logo.PNG", "animated.gif"];
        for name in img_files {
            let path = create_test_file(&dir, name, b"imagedata");
            assert!(
                !is_text_file(&path).unwrap(),
                "Expected '{}' to be a binary (image) file",
                name
            );
        }
    }

    #[test]
    fn test_nonexistent_file_is_not_text() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.file");
        assert!(!is_text_file(&path).unwrap());
    }

    #[test]
    fn test_empty_file_is_text() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "empty.unknown", b"");
        assert!(is_text_file(&path).unwrap());
    }

    #[test]
    fn test_is_text_by_utf8_content() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(
            &dir,
            "content.unknown",
            "Hello, world! This is UTF-8. 🦀".as_bytes(),
        );
        assert!(is_text_file(&path).unwrap());
    }

    #[test]
    fn test_is_not_text_due_to_null_byte() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(
            &dir,
            "binary.unknown",
            b"Here is some text\x00with a null byte.",
        );
        assert!(!is_text_file(&path).unwrap());
    }

    #[test]
    fn test_is_text_by_printable_ratio_heuristic() {
        let dir = TempDir::new().unwrap();
        let latin1_text = b"K\xf6nnen"; // "Können" in Latin-1
        let path = create_test_file(&dir, "legacy_encoding.unknown", latin1_text);
        assert!(is_text_file(&path).unwrap());
    }

    #[test]
    fn test_is_not_text_due_to_low_printable_ratio() {
        let dir = TempDir::new().unwrap();
        let random_binary = b"abc\x01\x02\x03\x04\x05\x06\x07\x08\x80\x90\xA0";
        let path = create_test_file(&dir, "random.unknown", random_binary);
        assert!(!is_text_file(&path).unwrap());
    }

    #[test]
    fn test_is_image_file_logic() {
        assert!(is_image_file(&Path::new("image.jpg")));
        assert!(is_image_file(&Path::new("image.PNG")));
        assert!(is_image_file(&Path::new("image.svg")));
        assert!(!is_image_file(&Path::new("document.txt")));
        assert!(!is_image_file(&Path::new("archive.zip")));
        assert!(!is_image_file(&Path::new("no_extension")));
    }

    #[test]
    fn test_batch_classify_files_logic() {
        let dir = TempDir::new().unwrap();
        let p1 = create_test_file(&dir, "text.txt", b"text");
        let p2 = create_test_file(&dir, "image.png", b"png");
        let p3 = create_test_file(&dir, "binary.zip", b"zip");
        let p4 = create_test_file(&dir, "vector.svg", b"<svg>");

        let paths: Vec<&Path> = vec![&p1, &p2, &p3, &p4];
        let results = batch_classify_files(&paths);

        assert_eq!(results.len(), 4);
        // (is_text, is_image)
        assert_eq!(results[0], (true, false));
        assert_eq!(results[1], (false, true));
        assert_eq!(results[2], (false, false));
        assert_eq!(results[3], (true, false));
    }
}
