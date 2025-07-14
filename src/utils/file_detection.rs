use std::path::Path;
use std::fs::File;
use std::io::{BufReader, Read};
use anyhow::Result;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "asciidoc", "adoc",
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "cpp", "cxx", "cc", "h", "hpp", "hxx",
    "go", "rb", "php", "swift", "kt", "kts", "scala", "clj", "cljs", "hs", "ml", "fs", "fsx",
    "html", "htm", "xml", "xhtml", "css", "scss", "sass", "less", "svg", "vue", "svelte",
    "json", "yaml", "yml", "toml", "ini", "cfg", "conf", "config", "properties",
    "sql", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "dockerfile", "makefile", "cmake",
    "gradle", "maven", "pom", "build", "tex", "bib", "r", "m", "pl", "lua", "vim", "el", "lisp",
    "dart", "elm", "ex", "exs", "erl", "hrl", "nim", "crystal", "cr", "zig", "odin", "v",
    "log", "trace", "out", "err", "diff", "patch", "gitignore", "gitattributes", "editorconfig",
    "env", "example", "sample", "template", "spec", "test", "readme", "license", "changelog",
    "todo", "notes", "doc", "docs", "man", "help", "faq",
    "lock", "sum", "mod", "work", "pest", "ron", "rlib", "pdb", "map", "d.ts",
    "mjs", "cjs", "coffee", "litcoffee", "ls", "flow", "pegjs",
    "graphql", "gql", "prisma", "proto", "thrift", "avsc", "jsonl", "ndjson",
    "csv", "tsv", "psv", "ssv", "tab", "data", "dat", "idx",
    "org", "tex", "cls", "sty", "bib", "bst", "aux", "fdb_latexmk", "fls",
    "R", "Rmd", "Rnw", "jl", "ipynb", "pyx", "pxd", "pxi", "pyi",
    "makefile", "gnumakefile", "dockerfile", "containerfile", "vagrantfile",
    "rakefile", "gemfile", "guardfile", "procfile", "capfile", "berksfile",
    "jenkinsfile", "dangerfile", "fastfile", "appfile", "deliverfile", "snapfile",
    "ignore", "keep", "gitkeep", "npmignore", "dockerignore", "eslintrc", "babelrc",
    "browserslistrc", "nvmrc", "rvmrc", "rbenv-version", "ruby-version", "node-version",
    "wasm", "wat", "wit", "component", // WebAssembly text formats
    "pest", "lalrpop", "y", "l", "lex", "yacc", // Parser generators
    "capnp", "fbs", "schema", "avdl", "thrift", // Schema definitions
    "gn", "gni", "bp", "BUILD", "WORKSPACE", "bzl", // Build files
    "nix", "drv", "store-path", // Nix files
    "dhall", "purescript", "purs", "elm", "roc", // Functional languages
    "gleam", "grain", "hx", "hxml", "moon", "zig", // Modern languages
    "just", "justfile", "task", "taskfile", // Task runners
    "editorconfig", "clang-format", "rustfmt", // Editor configs
    "modulemap", "def", "exports", "version", // Module definitions
    "in", "am", "ac", "m4", "cmake", "ctest", // Build system templates
    "service", "socket", "timer", "mount", // Systemd files
    "desktop", "appdata", "metainfo", // Desktop files
];

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "ico", "webp", // SVG ENTFERNT - ist jetzt nur Text
    "tiff", "tif", "raw", "cr2", "nef", "orf", "dng",
    "heic", "heif", "avif", "jfif",
];

const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "app", "deb", "rpm", "msi",
    "zip", "tar", "gz", "bz2", "7z", "rar", "jar", "war",
    "mp3", "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "bin", "dat", "db", "sqlite", "sqlite3",
    "rlib", "rmeta", "so", "d", "pdb", "ilk", "exp", "lib", "a",
    "obj", "o", "class", "pyc", "pyo", "__pycache__",
    "cache", "tmp", "temp", "swap", "bak", "backup",
    "fingerprint", "deps", "incremental",
    "crate", "gem", "whl", "egg", "rpm", "deb", "snap", "flatpak",
];

/// Determines if a file is likely to be a text file
pub fn is_text_file(path: &Path) -> Result<bool> {
    // First check by extension
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        
        // Known text extensions
        if TEXT_EXTENSIONS.contains(&ext_lower.as_str()) {
            return Ok(true);
        }
        
        // Known binary extensions
        if BINARY_EXTENSIONS.contains(&ext_lower.as_str()) {
            return Ok(false);
        }
        
        // Known image extensions
        if IMAGE_EXTENSIONS.contains(&ext_lower.as_str()) {
            return Ok(false);
        }
    }
    
    // SIZE-CHECK: Skip content check for large files
    if let Ok(metadata) = std::fs::metadata(path) {
        if metadata.len() > 20 * 1024 * 1024 { // 20MB limit for content check
            return Ok(false); // Assume binary for large unknown files
        }
    }
    
    // If extension is unknown AND file is small, check file content
    check_file_content(path)
}

/// Determines if a file is an image file (für Icon-Anzeige)
pub fn is_image_file(path: &Path) -> bool {
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        // SVG wird hier NICHT als Image behandelt für Icons, aber kann später erweitert werden
        IMAGE_EXTENSIONS.contains(&ext_lower.as_str()) || ext_lower == "svg"
    } else {
        false
    }
}

/// Checks file content to determine if it's text or binary
fn check_file_content(path: &Path) -> Result<bool> {
    let start = std::time::Instant::now();
    tracing::info!("🔍 Starting content check for: {}", path.display());
    
    let file_open_start = std::time::Instant::now();
    let file = File::open(path)?;
    tracing::info!("📂 File::open took: {:?} for {}", file_open_start.elapsed(), path.display());
    
    let reader_start = std::time::Instant::now();
    let mut reader = BufReader::new(file);
    let mut buffer = [0; 1024];
    tracing::info!("📖 BufReader created in: {:?}", reader_start.elapsed());
    
    let read_start = std::time::Instant::now();
    let bytes_read = reader.read(&mut buffer)?;
    tracing::info!("📄 Read {} bytes in: {:?} from {}", bytes_read, read_start.elapsed(), path.display());
    
    if bytes_read == 0 {
        tracing::info!("✅ Empty file check completed in: {:?}", start.elapsed());
        return Ok(true);
    }
    
    let null_check_start = std::time::Instant::now();
    let has_null_bytes = buffer[..bytes_read].contains(&0);
    tracing::info!("🔍 Null byte check took: {:?}", null_check_start.elapsed());
    
    if has_null_bytes {
        tracing::info!("✅ Binary (null bytes) detected in: {:?}", start.elapsed());
        return Ok(false);
    }
    
    let utf8_check_start = std::time::Instant::now();
    let is_utf8 = std::str::from_utf8(&buffer[..bytes_read]).is_ok();
    tracing::info!("🔤 UTF-8 check took: {:?}", utf8_check_start.elapsed());
    
    tracing::info!("✅ Content check completed in: {:?} - Result: {}", start.elapsed(), is_utf8);
    Ok(is_utf8)
}