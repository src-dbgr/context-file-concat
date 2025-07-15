use std::collections::HashSet;
use globset::{Glob, GlobSet, GlobSetBuilder};

/// Builds a `GlobSet` from a set of `.gitignore`-style patterns.
/// This is the centralized and optimized logic for pattern matching.
pub fn build_globset_from_patterns(patterns: &HashSet<String>) -> GlobSet {
    let mut builder = GlobSetBuilder::new();

    for pattern in patterns {
        let trimmed_pattern = pattern.trim();
        if trimmed_pattern.is_empty() || trimmed_pattern.starts_with('#') {
            continue;
        }

        // *** KORRIGIERT: Behandelt Verzeichnis-Patterns korrekt ***
        if let Some(dir_pattern) = trimmed_pattern.strip_suffix('/') {
            // Für ein Verzeichnis-Pattern wie "target/", fügen wir zwei Globs hinzu:
            
            // 1. Um das Verzeichnis selbst zu matchen (z.B. "**/target")
            if let Ok(glob) = Glob::new(&format!("**/{}", dir_pattern)) {
                builder.add(glob);
            }
            // 2. Um alle Inhalte innerhalb dieses Verzeichnisses zu matchen (z.B. "**/target/**")
            if let Ok(glob) = Glob::new(&format!("**/{}/**", dir_pattern)) {
                builder.add(glob);
            }
        } else {
            // Für Datei-Patterns (z.B. "*.log") oder exakte Namen (".DS_Store"),
            // wird ein Glob erstellt, der sie überall im Verzeichnisbaum findet.
            if let Ok(glob) = Glob::new(&format!("**/{}", trimmed_pattern)) {
                builder.add(glob);
            }
        }
    }

    builder.build().unwrap_or_else(|e| {
        tracing::error!("Failed to build glob set from patterns: {}", e);
        GlobSet::empty()
    })
}