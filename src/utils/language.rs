//! Maps a file to the language its content is highlighted as.

use std::path::Path;

/// The identifier of the language of the file at `path`, decided by its extension;
/// `"plaintext"` for everything that is not known. The identifiers are the ones the
/// editor of the UI knows.
pub fn language_of(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => "rust",
        Some("js") | Some("mjs") | Some("cjs") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("py") => "python",
        Some("html") | Some("htm") => "html",
        Some("css") => "css",
        Some("json") => "json",
        Some("md") => "markdown",
        Some("toml") => "toml",
        Some("yaml") | Some("yml") => "yaml",
        Some("sh") => "shell",
        Some("go") => "go",
        Some("java") => "java",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("hpp") | Some("cxx") | Some("hxx") => "cpp",
        _ => "plaintext",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_extensions_map_to_their_language() {
        for (file, language) in [
            ("src/main.rs", "rust"),
            ("a.mjs", "javascript"),
            ("a.tsx", "typescript"),
            ("index.htm", "html"),
            ("conf.yml", "yaml"),
            ("lib.hxx", "cpp"),
            ("lib.h", "c"),
        ] {
            assert_eq!(language_of(Path::new(file)), language, "{file}");
        }
    }

    #[test]
    fn everything_else_is_plain_text() {
        for file in ["README", "notes.txt", "archive.RS", ".gitignore", ""] {
            assert_eq!(language_of(Path::new(file)), "plaintext", "{file}");
        }
    }
}
