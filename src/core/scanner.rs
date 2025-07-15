use std::path::Path;
use std::collections::HashSet;
use tokio::sync::mpsc;
use walkdir::WalkDir;
use anyhow::Result;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use globset::GlobSet; // <-- HINZUGEFÜGT

use super::{FileItem, ScanProgress, build_globset_from_patterns}; // <-- MODIFIZIERT

pub struct DirectoryScanner {
    ignore_patterns: HashSet<String>,
}

impl DirectoryScanner {
    pub fn new(ignore_patterns: HashSet<String>) -> Self {
        Self { ignore_patterns }
    }
    
    pub async fn scan_directory(
        &self,
        root_path: &Path,
        progress_sender: mpsc::UnboundedSender<ScanProgress>,
        cancel_flag: Arc<AtomicBool>,
    ) -> Result<(Vec<FileItem>, usize, Vec<String>)> {
        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed: 0, total: 0, status: "Counting files...".to_string(),
            file_size: None, line_count: None,
        })?;

        // Build the globset ONCE before the loops for maximum performance.
        let ignore_glob_set = build_globset_from_patterns(&self.ignore_patterns);

        let mut total = 0;
        for (i, entry) in WalkDir::new(root_path).into_iter().filter_map(Result::ok).enumerate() {
            if i % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
                return Err(anyhow::anyhow!("Scan cancelled by user during counting phase."));
            }
            if !Self::should_ignore(entry.path(), &ignore_glob_set) {
                total += 1;
            }
        }

        if cancel_flag.load(Ordering::Relaxed) {
             return Err(anyhow::anyhow!("Scan cancelled by user."));
        }
        
        progress_sender.send(ScanProgress {
            current_file: root_path.to_path_buf(),
            processed: 0, total, status: "Scanning files...".to_string(),
            file_size: None, line_count: None,
        })?;
        
        let mut files = Vec::new();
        let mut processed = 0;
        let mut large_files_count = 0;
        let mut large_files_names = Vec::new();

        for entry in WalkDir::new(root_path).into_iter().filter_map(Result::ok) {
            if cancel_flag.load(Ordering::Relaxed) { return Err(anyhow::anyhow!("Scan cancelled by user.")); }

            let path = entry.path();
            
            if Self::should_ignore(path, &ignore_glob_set) {
                continue;
            }
            
            processed += 1;
            
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(e) => { tracing::warn!("Failed to get metadata for {}: {}", path.display(), e); continue; }
            };
            
            if metadata.is_dir() {
                 files.push(FileItem {
                    path: path.to_path_buf(), is_directory: true, is_binary: false, size: 0,
                    depth: path.strip_prefix(root_path).map(|p| p.components().count()).unwrap_or(0),
                    parent: path.parent().map(|p| p.to_path_buf()), children: Vec::new(),
                });
                continue;
            }

            let size = metadata.len();
            if size > 20 * 1024 * 1024 {
                large_files_count += 1;
                large_files_names.push(path.display().to_string());
                tracing::warn!("File {} exceeds 20MB limit, skipping", path.display());
                continue;
            }
            
            files.push(FileItem {
                path: path.to_path_buf(), is_directory: false,
                is_binary: Self::is_likely_binary_by_extension(path), size,
                depth: path.strip_prefix(root_path).map(|p| p.components().count()).unwrap_or(0),
                parent: path.parent().map(|p| p.to_path_buf()), children: Vec::new(),
            });
            
            if processed % 10 == 0 || processed < 100 {
                progress_sender.send(ScanProgress {
                    current_file: path.to_path_buf(), processed, total,
                    status: "Scanning...".to_string(), file_size: None, line_count: None,
                })?;
            }
        }
        
        if !cancel_flag.load(Ordering::Relaxed) {
            progress_sender.send(ScanProgress {
                current_file: root_path.to_path_buf(), processed, total,
                status: "Scan complete!".to_string(), file_size: None, line_count: None,
            })?;
        }

        Ok((files, large_files_count, large_files_names))
    }

    fn is_likely_binary_by_extension(path: &Path) -> bool {
        // Unchanged
        if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            let ext_lower = extension.to_lowercase();
            const DEFINITELY_TEXT: &[&str] = &["txt","md","markdown","rst","asciidoc","adoc","rs","py","js","ts","jsx","tsx","java","c","cpp","cxx","cc","h","hpp","hxx","go","rb","php","swift","kt","kts","scala","clj","cljs","hs","ml","fs","fsx","html","htm","xml","xhtml","css","scss","sass","less","svg","vue","svelte","json","yaml","yml","toml","ini","cfg","conf","config","properties","sql","sh","bash","zsh","fish","ps1","bat","cmd","dockerfile","makefile","cmake","gradle","maven","pom","build","tex","bib","r","m","pl","lua","vim","el","lisp","dart","elm","ex","exs","erl","hrl","nim","crystal","cr","zig","odin","v","log","trace","out","err","diff","patch","gitignore","gitattributes","editorconfig","env","example","sample","template","spec","test","readme","license","changelog","todo","notes","doc","docs","man","help","faq","lock","sum","mod","work","pest","ron","d.ts","mjs","cjs","coffee","graphql","gql","prisma","proto","csv","tsv","data","org","R","Rmd","jl","pyi","rakefile","gemfile","procfile","capfile","jenkinsfile","fastfile","npmignore","dockerignore","eslintrc","babelrc","nvmrc","rvmrc"];
            const DEFINITELY_BINARY: &[&str] = &["exe","dll","so","dylib","app","deb","rpm","msi","zip","tar","gz","bz2","7z","rar","jar","war","mp3","mp4","avi","mkv","mov","wmv","flv","webm","m4a","wav","ogg","jpg","jpeg","png","gif","bmp","ico","webp","tiff","tif","raw","heic","heif","pdf","doc","docx","xls","xlsx","ppt","pptx","bin","dat","db","sqlite","sqlite3","dmg","iso","img","icns","ico","pkg","class","pyc","pyo","o","obj","lib","a","rlib"];
            if DEFINITELY_TEXT.contains(&ext_lower.as_str()) { return false; }
            if DEFINITELY_BINARY.contains(&ext_lower.as_str()) { return true; }
        }
        false
    }
    
    // *** MODIFIED: Uses the pre-compiled globset for a simple, fast check ***
    fn should_ignore(path: &Path, ignore_glob_set: &GlobSet) -> bool {
        // A special check for `.git` is still useful for performance, as it can prune huge trees early.
        if path.components().any(|c| c.as_os_str() == ".git") {
            return true;
        }
        ignore_glob_set.is_match(path)
    }
}

impl Default for DirectoryScanner {
    // Unchanged
    fn default() -> Self {
        let mut ignore_patterns = HashSet::new();
        ignore_patterns.insert("node_modules/".to_string());
        ignore_patterns.insert("target/".to_string());
        ignore_patterns.insert(".git/".to_string());
        ignore_patterns.insert("*.log".to_string());
        ignore_patterns.insert("*.tmp".to_string());
        Self { ignore_patterns }
    }
}