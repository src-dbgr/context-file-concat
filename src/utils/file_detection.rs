use std::path::Path;
use std::fs::File;
use std::io::{BufReader, Read};
use anyhow::Result;

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rst", "asciidoc",
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "c", "cpp", "h", "hpp",
    "go", "rb", "php", "swift", "kt", "scala", "clj", "hs", "ml", "fs",
    "html", "htm", "xml", "css", "scss", "sass", "less",
    "json", "yaml", "yml", "toml", "ini", "cfg", "conf",
    "sql", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
    "dockerfile", "makefile", "cmake", "gradle", "maven",
    "tex", "bib", "r", "m", "pl", "lua", "vim", "el",
];

const BINARY_EXTENSIONS: &[&str] = &[
    "exe", "dll", "so", "dylib", "app", "deb", "rpm", "msi",
    "zip", "tar", "gz", "bz2", "7z", "rar", "jar", "war",
    "jpg", "jpeg", "png", "gif", "bmp", "ico", "svg", "webp",
    "mp3", "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "bin", "dat", "db", "sqlite", "sqlite3",
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
    }
    
    // If extension is unknown, check file content
    check_file_content(path)
}

/// Checks file content to determine if it's text or binary
fn check_file_content(path: &Path) -> Result<bool> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut buffer = [0; 1024]; // Read first 1KB
    
    let bytes_read = reader.read(&mut buffer)?;
    
    if bytes_read == 0 {
        return Ok(true); // Empty file, consider as text
    }
    
    // Check for null bytes (common in binary files)
    if buffer[..bytes_read].contains(&0) {
        return Ok(false);
    }
    
    // Check UTF-8 validity
    match std::str::from_utf8(&buffer[..bytes_read]) {
        Ok(_) => Ok(true),
        Err(_) => {
            // Try to detect common text encodings
            // For simplicity, we'll be conservative and say it's binary
            // if it's not valid UTF-8
            Ok(false)
        }
    }
}

/// Get a human-readable file type description
pub fn get_file_type_description(path: &Path) -> String {
    if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
        let ext_lower = extension.to_lowercase();
        
        match ext_lower.as_str() {
            "rs" => "Rust source".to_string(),
            "py" => "Python script".to_string(),
            "js" => "JavaScript".to_string(),
            "ts" => "TypeScript".to_string(),
            "java" => "Java source".to_string(),
            "c" => "C source".to_string(),
            "cpp" | "cxx" | "cc" => "C++ source".to_string(),
            "h" | "hpp" => "Header file".to_string(),
            "html" | "htm" => "HTML document".to_string(),
            "css" => "Stylesheet".to_string(),
            "json" => "JSON data".to_string(),
            "xml" => "XML document".to_string(),
            "md" | "markdown" => "Markdown document".to_string(),
            "txt" => "Text file".to_string(),
            "pdf" => "PDF document".to_string(),
            "jpg" | "jpeg" => "JPEG image".to_string(),
            "png" => "PNG image".to_string(),
            "zip" => "ZIP archive".to_string(),
            _ => format!("{} file", ext_lower.to_uppercase()),
        }
    } else {
        "Unknown file type".to_string()
    }
}