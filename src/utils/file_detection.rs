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
    "component", // WebAssembly text formats
    "pest",
    "lalrpop",
    "y",
    "l",
    "lex",
    "yacc", // Parser generators
    "capnp",
    "fbs",
    "schema",
    "avdl",
    "thrift", // Schema definitions
    "gn",
    "gni",
    "bp",
    "BUILD",
    "WORKSPACE",
    "bzl", // Build files
    "nix",
    "drv",
    "store-path", // Nix files
    "dhall",
    "purescript",
    "purs",
    "elm",
    "roc", // Functional languages
    "gleam",
    "grain",
    "hx",
    "hxml",
    "moon",
    "zig", // Modern languages
    "just",
    "justfile",
    "task",
    "taskfile", // Task runners
    "editorconfig",
    "clang-format",
    "rustfmt", // Editor configs
    "modulemap",
    "def",
    "exports",
    "version", // Module definitions
    "in",
    "am",
    "ac",
    "m4",
    "cmake",
    "ctest", // Build system templates
    "service",
    "socket",
    "timer",
    "mount", // Systemd files
    "desktop",
    "appdata",
    "metainfo", // Desktop files
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
const CONTENT_CHECK_BUFFER_SIZE: usize = 1024; // 1KB für Content-Check

/// Determines if a file is likely to be a text file
/// OPTIMIERT für bessere Performance bei großen Directory-Scans
pub fn is_text_file(path: &Path) -> Result<bool> {
    // PERFORMANCE: Frühe Extension-Checks mit HashSet-Lookups
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();

        // O(1) Lookups statt linearer Suche
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

    // PERFORMANCE: Frühe Größen-Prüfung ohne File-Handle
    match std::fs::metadata(path) {
        Ok(metadata) => {
            if metadata.len() > MAX_CONTENT_CHECK_SIZE {
                return Ok(false); // Große unbekannte Dateien = binär
            }

            // Leere Dateien sind Text
            if metadata.len() == 0 {
                return Ok(true);
            }
        }
        Err(_) => return Ok(false), // Nicht lesbare Dateien = binär
    }

    // Content-Check nur für kleine, unbekannte Extensions
    check_file_content_optimized(path)
}

/// Determines if a file is an image file (für Icon-Anzeige)
/// OPTIMIERT mit HashSet-Lookup
pub fn is_image_file(path: &Path) -> bool {
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        get_image_ext_set().contains(ext_lower.as_str()) || ext_lower == "svg"
    } else {
        false
    }
}

/// OPTIMIERTE Version des Content-Checks
/// Reduziert I/O und verbessert Performance
fn check_file_content_optimized(path: &Path) -> Result<bool> {
    // PERFORMANCE: Kleinerer Buffer für schnelleren I/O
    let mut buffer = [0u8; CONTENT_CHECK_BUFFER_SIZE];

    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(CONTENT_CHECK_BUFFER_SIZE, file);

    let bytes_read = reader.read(&mut buffer)?;

    if bytes_read == 0 {
        return Ok(true); // Leere Dateien sind Text
    }

    // PERFORMANCE: Früher Null-Byte-Check (häufigster Indikator für binär)
    if buffer[..bytes_read].contains(&0) {
        return Ok(false);
    }

    // PERFORMANCE: Schneller UTF-8-Check ohne String-Allocation
    match std::str::from_utf8(&buffer[..bytes_read]) {
        Ok(_) => Ok(true),
        Err(_) => {
            // Fallback: Prüfe auf häufige nicht-UTF8 aber text-ähnliche Encodings
            // Wenn > 95% der Bytes druckbare ASCII-Zeichen sind, behandle als Text
            let printable_count = buffer[..bytes_read]
                .iter()
                .filter(|&&b| b >= 32 && b <= 126 || b == 9 || b == 10 || b == 13)
                .count();

            let ratio = printable_count as f32 / bytes_read as f32;
            Ok(ratio > 0.95)
        }
    }
}

/// NEUE FUNKTION: Batch-Processing für bessere Performance bei vielen Dateien
pub fn batch_classify_files(paths: &[&Path]) -> Vec<(bool, bool)> {
    // Rückgabe: (is_text, is_image) für jeden Pfad
    paths
        .iter()
        .map(|path| {
            let is_text = is_text_file(path).unwrap_or(false);
            let is_image = if !is_text { is_image_file(path) } else { false };
            (is_text, is_image)
        })
        .collect()
}
