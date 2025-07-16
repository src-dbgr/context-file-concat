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
    "rlib",
    "pdb",
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
    "dat",
    "idx",
    "org",
    "tex",
    "cls",
    "sty",
    "bib",
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
    "makefile",
    "gnumakefile",
    "dockerfile",
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
    "pest",
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
    "elm",
    "roc",
    "gleam",
    "grain",
    "hx",
    "hxml",
    "moon",
    "zig",
    "just",
    "justfile",
    "task",
    "taskfile",
    "editorconfig",
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
    "cmake",
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
    "doc",
    "docx",
    "xls",
    "xlsx",
    "ppt",
    "pptx",
    "bin",
    "dat",
    "db",
    "sqlite",
    "sqlite3",
    "rlib",
    "rmeta",
    "so",
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
    "rpm",
    "deb",
    "snap",
    "flatpak",
];

// PERFORMANCE: Statische Sets für O(1) Lookups erstellen
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

const MAX_CONTENT_CHECK_SIZE: u64 = 20 * 1024 * 1024; // 20MB
const CONTENT_CHECK_BUFFER_SIZE: usize = 1024; // 1KB for Content-Check

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
            // Fallback: Check for frequent non-UTF8 but text-similar encodings
            // If > 95% of Bytes are ASCII-Symbols, treat them as text
            let printable_count = buffer[..bytes_read]
                .iter()
                .filter(|&&b| (32..=126).contains(&b) || b == 9 || b == 10 || b == 13)
                .count();

            let ratio = printable_count as f32 / bytes_read as f32;
            Ok(ratio > 0.95)
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
