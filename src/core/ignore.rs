use globset::{Glob, GlobSet, GlobSetBuilder};
use std::collections::HashSet;

/// Builds a `GlobSet` from a set of `.gitignore`-style patterns and returns the patterns used.
/// The index of a match in the GlobSet corresponds to the index in the returned `Vec<String>`.
pub fn build_globset_from_patterns(patterns: &HashSet<String>) -> (GlobSet, Vec<String>) {
    let mut builder = GlobSetBuilder::new();
    let mut glob_patterns_list = Vec::new();

    for pattern in patterns {
        let trimmed_pattern = pattern.trim();
        if trimmed_pattern.is_empty() || trimmed_pattern.starts_with('#') {
            continue;
        }

        let is_dir_pattern_suffix = trimmed_pattern.ends_with('/');
        let is_simple_name_like_dir = !trimmed_pattern.contains('/')
            && !trimmed_pattern.contains('*')
            && !trimmed_pattern.contains('?');

        if is_dir_pattern_suffix || is_simple_name_like_dir {
            let base_pattern = if is_dir_pattern_suffix {
                trimmed_pattern.strip_suffix('/').unwrap_or(trimmed_pattern)
            } else {
                trimmed_pattern
            };

            // 1. Glob, um das Verzeichnis/die Datei selbst zu matchen (z.B. `**/target`).
            if let Ok(glob) = Glob::new(&format!("**/{}", base_pattern)) {
                builder.add(glob);
                glob_patterns_list.push(pattern.clone());
            }
            // 2. Glob, um alles INNERHALB des Verzeichnisses zu matchen (z.B. `**/target/**`).
            if let Ok(glob) = Glob::new(&format!("**/{}/**", base_pattern)) {
                builder.add(glob);
                glob_patterns_list.push(pattern.clone());
            }
        } else {
            // Alle anderen Patterns (wie `*.log` oder `src/*.rs`) werden als normale Globs behandelt.
            // Das `**/` Präfix stellt sicher, dass sie in jeder Tiefe gefunden werden.
            if let Ok(glob) = Glob::new(&format!("**/{}", trimmed_pattern)) {
                builder.add(glob);
                glob_patterns_list.push(pattern.clone());
            }
        }
    }

    let glob_set = builder.build().unwrap_or_else(|e| {
        tracing::error!("Failed to build glob set from patterns: {}", e);
        GlobSet::empty()
    });

    (glob_set, glob_patterns_list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_simple_file_pattern() {
        let mut patterns = HashSet::new();
        patterns.insert("*.log".to_string());

        let (glob_set, _) = build_globset_from_patterns(&patterns);

        assert!(glob_set.is_match(Path::new("project/app.log")));
        assert!(glob_set.is_match(Path::new("app.log")));
        assert!(glob_set.is_match(Path::new("deep/nested/path/to/error.log")));
        assert!(!glob_set.is_match(Path::new("project/app.txt")));
        assert!(!glob_set.is_match(Path::new("project/log.txt")));
    }

    #[test]
    fn test_directory_pattern() {
        let mut patterns = HashSet::new();
        patterns.insert("target/".to_string());

        let (glob_set, _) = build_globset_from_patterns(&patterns);

        assert!(glob_set.is_match(Path::new("my_project/target")));
        assert!(glob_set.is_match(Path::new("target")));
        assert!(glob_set.is_match(Path::new("my_project/target/debug/app")));
        assert!(glob_set.is_match(Path::new("target/release/lib.rlib")));
        assert!(!glob_set.is_match(Path::new("my_project/src/target.rs")));
        assert!(!glob_set.is_match(Path::new("my_project/src")));
        assert!(!glob_set.is_match(Path::new("other_target/file")));
    }

    #[test]
    fn test_deep_directory_pattern() {
        let mut patterns = HashSet::new();
        patterns.insert("__pycache__".to_string());

        let (glob_set, _) = build_globset_from_patterns(&patterns);

        assert!(glob_set.is_match(Path::new("src/app/__pycache__")));
        assert!(glob_set.is_match(Path::new("src/app/__pycache__/some_file.pyc")));
        assert!(glob_set.is_match(Path::new("__pycache__")));
        assert!(!glob_set.is_match(Path::new("src/pycache")));
        assert!(!glob_set.is_match(Path::new("src/app_pycache/file")));
    }

    #[test]
    fn test_multiple_and_complex_patterns() {
        let mut patterns = HashSet::new();
        patterns.insert("node_modules/".to_string());
        patterns.insert("*.tmp".to_string());
        patterns.insert("build".to_string());

        let (glob_set, _) = build_globset_from_patterns(&patterns);

        assert!(glob_set.is_match(Path::new("node_modules")));
        assert!(glob_set.is_match(Path::new("project/node_modules/library/index.js")));
        assert!(glob_set.is_match(Path::new("session.tmp")));
        assert!(glob_set.is_match(Path::new("logs/user.tmp")));
        assert!(glob_set.is_match(Path::new("dist/build")));
        assert!(glob_set.is_match(Path::new("dist/build/output.js")));
        assert!(!glob_set.is_match(Path::new("project/src/builder.rs")));
    }
}
