//! Tells text files from binary ones.
//!
//! There is one rule, [`is_binary`], and everything that has to know whether a file is
//! text applies it: the scan (`FileItem::is_binary`), the preview, the content search
//! and the generated output. They can therefore not disagree about a file.
//!
//! The rule:
//!
//! 1. A file whose extension names a format that is certainly binary (an image, an
//!    archive, an executable, ...) is binary. This is a shortcut that saves reading
//!    the file, and it covers the formats whose first bytes can pass for text (a PDF
//!    starts with a line of ASCII).
//! 2. Every other file is what its content says: binary if its first
//!    [`SAMPLE_SIZE`] bytes hold a NUL byte or many control characters, text
//!    otherwise.
//!
//! An extension never makes a file text. A list of "text extensions" would have to
//! know every language and tool there is, and it would be wrong about files that
//! carry such an extension for other reasons.

use anyhow::Result;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Extensions (lowercase) of formats that are binary whatever a sample of the content
/// looks like.
///
/// Only formats belong here whose files are never text. Extensions that are merely
/// often binary (`dat`, `tmp`, `bak`, `cache`, ...) or that are shared with text
/// formats (`d`: D sources and dependency files of Make; `snap`: snapshot tests;
/// `raw`: camera images, but also plain dumps and text) are left to the content.
const BINARY_EXTENSIONS: &[&str] = &[
    // Images (SVG is text and is not listed)
    "jpg", "jpeg", "jfif", "png", "gif", "bmp", "ico", "icns", "webp", "tiff", "tif", "cr2", "nef",
    "orf", "dng", "heic", "heif", "avif", "psd", // Audio and video
    "mp3", "wav", "flac", "ogg", "m4a", "mp4", "avi", "mkv", "mov", "wmv", "flv", "webm",
    // Archives and packages
    "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "jar", "war", "crate", "gem", "whl",
    "egg", "deb", "rpm", "msi", "dmg", "iso", "flatpak",
    // Executables, libraries and other compiler output
    "exe", "dll", "so", "dylib", "bin", "o", "obj", "a", "lib", "rlib", "rmeta", "pdb", "ilk",
    "class", "pyc", "pyo", "wasm", // Documents and databases
    "pdf", "docx", "xlsx", "pptx", "db", "sqlite", "sqlite3", // Fonts
    "ttf", "otf", "woff", "woff2", "eot",
];

/// How many leading bytes of a file decide whether its content is text.
pub const SAMPLE_SIZE: usize = 1024;

/// True if the extension of `path` names a format that is certainly binary. Does not
/// touch the file system. The case of the extension does not matter.
pub fn has_binary_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            BINARY_EXTENSIONS
                .iter()
                .any(|binary| extension.eq_ignore_ascii_case(binary))
        })
}

/// The rule that tells text from binary (pure, no IO): `path` is the name of the file,
/// `content` its content or any prefix of it that holds the first [`SAMPLE_SIZE`]
/// bytes (or the whole content, if that is shorter). See the module documentation.
///
/// The verdict depends on nothing but the extension and those leading bytes. It does
/// not depend on the size of the file: which files are too large to be listed,
/// searched or embedded is the business of the configured size limit alone.
pub fn is_binary(path: &Path, content: &[u8]) -> bool {
    has_binary_extension(path) || !is_text_content(content)
}

/// Applies [`is_binary`] to the file at `path`, reading no more than the sample.
///
/// A file that does not exist is not text; one that exists but cannot be read is an
/// error.
pub fn is_text_file(path: &Path) -> Result<bool> {
    is_text_file_with(path, |path| File::open(path))
}

/// [`is_text_file`] with the function that opens the file as a parameter.
///
/// `open` is not called for a file whose extension settles the question: such a file
/// costs no file system access at all.
pub fn is_text_file_with<R: Read>(
    path: &Path,
    open: impl FnOnce(&Path) -> io::Result<R>,
) -> Result<bool> {
    if has_binary_extension(path) {
        return Ok(false);
    }
    // Opened without asking for the metadata first: the scan calls this for every
    // file it lists, and each system call counts there.
    let file = match open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.into()),
    };
    Ok(is_text_content(&read_sample(file)?))
}

/// Reads the bytes [`is_binary`] looks at from the start of `reader`: the first
/// [`SAMPLE_SIZE`] bytes, or everything if there is less.
pub fn read_sample(reader: impl Read) -> io::Result<Vec<u8>> {
    let mut sample = Vec::with_capacity(SAMPLE_SIZE);
    reader.take(SAMPLE_SIZE as u64).read_to_end(&mut sample)?;
    Ok(sample)
}

/// Guesses whether raw bytes are text, based on their leading sample.
///
/// Only the first [`SAMPLE_SIZE`] bytes are inspected, so the verdict is the same for
/// the whole content of a file and for the sample read from it.
pub fn is_text_content(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(SAMPLE_SIZE)];
    if sample.is_empty() {
        return true;
    }
    if sample.contains(&0) {
        return false;
    }
    match std::str::from_utf8(sample) {
        Ok(_) => return true,
        // A full sample may end inside a multi-byte character. That is the cut, not
        // the content: everything before it is valid UTF-8.
        Err(error) if error.error_len().is_none() && sample.len() == SAMPLE_SIZE => return true,
        Err(_) => {}
    }
    // Not valid UTF-8: rather than requiring printable ASCII, check for the absence of
    // control characters (excluding whitespace). This correctly handles legacy 8-bit
    // encodings like Latin-1.
    let control_char_count = sample
        .iter()
        .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
        .count();

    // If less than 5% of the bytes are weird control characters, we assume it's text.
    let ratio = control_char_count as f32 / sample.len() as f32;
    ratio < 0.05
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

    /// Images and archives are recognised by their extension, whatever they contain;
    /// SVG is an image format that is text.
    #[test]
    fn test_images_and_archives_are_not_text_but_svg_is() {
        let dir = TempDir::new().unwrap();
        let text = create_test_file(&dir, "text.txt", b"text");
        let image = create_test_file(&dir, "image.png", b"png");
        let upper_case_image = create_test_file(&dir, "image.JPG", b"jpg");
        let archive = create_test_file(&dir, "binary.zip", b"zip");
        let vector = create_test_file(&dir, "vector.svg", b"<svg>");

        assert!(is_text_file(&text).unwrap());
        assert!(!is_text_file(&image).unwrap());
        assert!(!is_text_file(&upper_case_image).unwrap());
        assert!(!is_text_file(&archive).unwrap());
        assert!(is_text_file(&vector).unwrap());
    }

    /// The size of a file says nothing about whether it is text: only the leading
    /// bytes are looked at. (The file is sparse, so the test writes almost nothing.)
    #[test]
    fn test_size_does_not_decide_whether_a_file_is_text() {
        let dir = TempDir::new().unwrap();
        let size = 64 * 1024 * 1024;

        let grow = |path: &Path| {
            let file = fs::OpenOptions::new().append(true).open(path).unwrap();
            file.set_len(size).unwrap();
        };

        // The sparse rest reads as NUL bytes, but lies beyond the sniffed prefix.
        let text = create_test_file(&dir, "huge_text.unknown", &vec![b'a'; SAMPLE_SIZE]);
        grow(&text);
        let binary = create_test_file(&dir, "huge_binary.unknown", b"\x00\x01\x02");
        grow(&binary);

        assert_eq!(fs::metadata(&text).unwrap().len(), size);
        assert!(is_text_file(&text).unwrap());
        assert!(!is_text_file(&binary).unwrap());
    }

    // =========================================================================
    // SECTION: The rule (pure)
    // =========================================================================

    const TEXT: &[u8] = b"plain text\n";
    const LATIN1: &[u8] = b"K\xf6nnen";
    const NUL: &[u8] = b"text\x00with a NUL byte";
    const CONTROL: &[u8] = b"abc\x01\x02\x03\x04\x05\x06\x07\x08\x80\x90\xA0";

    /// (file name, content, is it binary?)
    #[test]
    fn test_rule_table() {
        let cases: &[(&str, &[u8], bool)] = &[
            // Certainly binary formats, whatever the sample looks like.
            ("photo.jpg", TEXT, true),
            ("logo.PNG", TEXT, true),
            ("icon.icns", TEXT, true),
            ("archive.zip", TEXT, true),
            ("archive.tar.gz", TEXT, true),
            ("program.EXE", TEXT, true),
            ("library.dylib", TEXT, true),
            ("module.wasm", TEXT, true),
            ("module.WASM", TEXT, true),
            ("manual.pdf", b"%PDF-1.7\n1 0 obj\n", true),
            ("data.sqlite", TEXT, true),
            ("font.woff2", TEXT, true),
            ("firmware.bin", TEXT, true),
            // An image format that is text.
            ("vector.svg", b"<svg/>", false),
            ("module.wat", b"(module)", false),
            // Extensions that used to be taken for binary without a look.
            ("main.d", b"import std.stdio;\n", false),
            ("main.o.d", b"main.o: main.c main.h\n", false),
            ("test__case.snap", b"---\nsource: src/lib.rs\n---\n", false),
            ("notes.tmp", TEXT, false),
            ("settings.json.bak", TEXT, false),
            ("old.backup", TEXT, false),
            ("build.cache", TEXT, false),
            ("values.dat", b"1;2;3\n", false),
            ("dump.raw", b"0 1 2 3\n", false),
            ("photo.raw", NUL, true),
            ("symbols.exp", TEXT, false),
            ("Cargo.lock.temp", TEXT, false),
            // ... and are binary if their content is.
            ("main.d", NUL, true),
            ("page.tmp", NUL, true),
            ("values.dat", CONTROL, true),
            // Extensions with upper case letters, which the former lists never matched.
            ("analysis.R", b"x <- 1\n", false),
            ("report.Rmd", b"# Title\n", false),
            ("BUILD", b"cc_library()\n", false),
            ("WORKSPACE", b"workspace()\n", false),
            // Unknown and missing extensions: the content decides.
            ("Makefile", b"all:\n\ttrue\n", false),
            ("notes.unknown", TEXT, false),
            ("notes.unknown", LATIN1, false),
            ("notes.unknown", NUL, true),
            ("notes.unknown", CONTROL, true),
            (".gitignore", b"target/\n", false),
            ("empty.unknown", b"", false),
            // A text extension does not make binary content text (e.g. UTF-16).
            ("utf16.txt", b"\xff\xfeh\x00i\x00", true),
            ("data.json", NUL, true),
            ("code.rs", "fn main() {} // 🦀".as_bytes(), false),
        ];
        for (name, content, expected) in cases {
            assert_eq!(
                is_binary(Path::new(name), content),
                *expected,
                "{name} with {content:?}"
            );
        }
    }

    /// The file on disk gets the verdict of the rule: `is_text_file` is the rule
    /// applied to what it reads.
    #[test]
    fn test_file_verdict_is_the_verdict_of_the_rule() {
        let dir = TempDir::new().unwrap();
        let long_text = "ä".repeat(SAMPLE_SIZE);
        let cases: &[(&str, &[u8])] = &[
            ("photo.jpg", TEXT),
            ("main.d", TEXT),
            ("main2.d", NUL),
            ("case.snap", TEXT),
            ("module.wasm", b"\x00asm\x01\x00\x00\x00"),
            ("notes.unknown", LATIN1),
            ("random.unknown", CONTROL),
            ("utf16.txt", b"\xff\xfeh\x00i\x00"),
            ("long.txt", long_text.as_bytes()),
            ("empty.txt", b""),
        ];
        for (name, content) in cases {
            let path = create_test_file(&dir, name, content);
            assert_eq!(
                is_text_file(&path).unwrap(),
                !is_binary(&path, content),
                "{name}"
            );
        }
    }

    /// The opener is the only way to the content, and it is not used where the
    /// extension settles the question.
    #[test]
    fn test_certainly_binary_extension_never_opens_the_file() {
        let opened = std::cell::RefCell::new(Vec::new());
        let open = |path: &Path| {
            opened.borrow_mut().push(path.to_path_buf());
            Ok(&b"text"[..])
        };

        for name in ["a.png", "b.ZIP", "c.wasm", "d.pdf", "dir/e.exe"] {
            assert!(!is_text_file_with(Path::new(name), open).unwrap(), "{name}");
        }
        assert!(opened.borrow().is_empty(), "{:?}", opened.borrow());

        assert!(is_text_file_with(Path::new("f.raw"), open).unwrap());
        assert!(is_text_file_with(Path::new("g.txt"), open).unwrap());
        assert_eq!(*opened.borrow(), [Path::new("f.raw"), Path::new("g.txt")]);
    }

    #[test]
    fn test_opener_errors_are_passed_on_except_for_a_missing_file() {
        let missing = |_: &Path| -> io::Result<&[u8]> { Err(io::ErrorKind::NotFound.into()) };
        let denied =
            |_: &Path| -> io::Result<&[u8]> { Err(io::ErrorKind::PermissionDenied.into()) };

        assert!(!is_text_file_with(Path::new("gone.txt"), missing).unwrap());
        assert!(is_text_file_with(Path::new("locked.txt"), denied).is_err());
    }

    #[test]
    fn test_extension_shortcut_does_not_read_the_file() {
        let dir = TempDir::new().unwrap();
        // Neither file exists: only the name is looked at.
        assert!(!is_text_file(&dir.path().join("missing.png")).unwrap());
        assert!(has_binary_extension(Path::new("a/b/c.TAR")));
        assert!(!has_binary_extension(Path::new("a/b/png")));
        assert!(!has_binary_extension(Path::new("a/b/.png")));
        assert!(!has_binary_extension(Path::new("archive.zip/readme")));
    }

    /// No entry of the list is dead: all are lowercase (the comparison ignores the
    /// case of the file's extension only), without a dot, and listed once.
    #[test]
    fn test_binary_extensions_are_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for extension in BINARY_EXTENSIONS {
            assert!(
                !extension.is_empty()
                    && extension
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "{extension:?}"
            );
            assert!(seen.insert(extension), "{extension:?} is listed twice");
            assert!(has_binary_extension(Path::new(&format!(
                "file.{extension}"
            ))));
            assert!(has_binary_extension(Path::new(&format!(
                "file.{}",
                extension.to_uppercase()
            ))));
        }
    }

    // =========================================================================
    // SECTION: The sample
    // =========================================================================

    #[test]
    fn test_sample_is_the_beginning_of_the_content() {
        let content: Vec<u8> = (0..3 * SAMPLE_SIZE).map(|i| (i % 251) as u8).collect();

        assert_eq!(read_sample(&content[..]).unwrap(), &content[..SAMPLE_SIZE]);
        assert_eq!(read_sample(&content[..10]).unwrap(), &content[..10]);
        assert!(read_sample(&[][..]).unwrap().is_empty());
    }

    /// The whole content and the sample read from the file get the same verdict,
    /// also when the sample ends inside a character.
    #[test]
    fn test_verdict_is_the_same_for_the_sample_and_the_whole_content() {
        // "é" is two bytes; the odd offset puts one of them on each side of the cut.
        let cut_inside_a_character = format!("a{}", "é".repeat(SAMPLE_SIZE));
        // Coloured terminal output: valid UTF-8 with many control characters.
        let escapes = "\x1b[31mé\x1b[0m".repeat(SAMPLE_SIZE);
        let escapes_cut_inside_a_character = format!("a{escapes}");
        let binary_after_the_sample = [vec![b'a'; SAMPLE_SIZE], vec![0u8; 16]].concat();

        for (content, is_text) in [
            (cut_inside_a_character.as_bytes(), true),
            (escapes.as_bytes(), true),
            (escapes_cut_inside_a_character.as_bytes(), true),
            (&binary_after_the_sample[..], true),
            (NUL, false),
        ] {
            let sample = read_sample(content).unwrap();
            assert_eq!(is_text_content(content), is_text);
            assert_eq!(is_text_content(&sample), is_text);
        }
    }

    /// A short file that ends inside a character is not cut: the bytes are invalid.
    #[test]
    fn test_incomplete_character_at_the_end_of_a_short_file_is_not_utf8() {
        // Invalid UTF-8 and more than 5% control characters.
        let content = b"\x01\x02\x03\x04\xc3";
        assert!(!is_text_content(content));
        // Without the control characters it passes for a legacy encoding.
        assert!(is_text_content(b"caf\xc3"));
    }
}
