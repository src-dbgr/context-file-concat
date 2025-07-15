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

        // --- MODIFIZIERTE LOGIK ---
        // Prüfen, ob das Pattern wie ein Verzeichnis behandelt werden soll.
        // Dies ist der Fall, wenn es mit '/' endet ODER ein einfacher Name ohne Wildcards/Slashes ist (z.B. "target").
        let is_dir_pattern_suffix = trimmed_pattern.ends_with('/');
        let is_simple_name_like_dir = !trimmed_pattern.contains('/') 
            && !trimmed_pattern.contains('*') 
            && !trimmed_pattern.contains('?');

        if is_dir_pattern_suffix || is_simple_name_like_dir {
            // Normalisiere das Pattern, indem der eventuelle Schrägstrich am Ende entfernt wird.
            let base_pattern = if is_dir_pattern_suffix {
                trimmed_pattern.strip_suffix('/').unwrap_or(trimmed_pattern)
            } else {
                trimmed_pattern
            };

            // 1. Glob, um das Verzeichnis/die Datei selbst zu matchen (z.B. `**/target`).
            if let Ok(glob) = Glob::new(&format!("**/{}", base_pattern)) {
                builder.add(glob);
            }
            // 2. Glob, um alles INNERHALB des Verzeichnisses zu matchen (z.B. `**/target/**`).
            if let Ok(glob) = Glob::new(&format!("**/{}/**", base_pattern)) {
                builder.add(glob);
            }
        } else {
            // Alle anderen Patterns (wie `*.log` oder `src/*.rs`) werden als normale Globs behandelt.
            // Das `**/` Präfix stellt sicher, dass sie in jeder Tiefe gefunden werden.
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