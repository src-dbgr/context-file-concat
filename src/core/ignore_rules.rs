//! The ignore rules of a project: the one place that decides whether a path is hidden
//! by the user's ignore patterns.
//!
//! # Semantics
//!
//! The patterns follow the `.gitignore` syntax, as implemented by the `ignore` crate,
//! and are anchored at the project root:
//!
//! - `name` matches a file or directory of that name at any depth, `*.log` any such file.
//! - `dir/` matches directories only.
//! - `/x` and `a/b` (a slash at the start or in the middle) match relative to the
//!   project root only.
//! - `!pattern` is a negation: it re-includes what another pattern excludes.
//! - An excluded directory hides everything below it. As in git, a negation cannot
//!   re-include an entry whose parent directory is excluded.
//!
//! The patterns are a *set*, so there is no "last pattern wins" as in a `.gitignore`
//! file. Instead **negations take precedence over all other patterns**, in whatever
//! order they were added.
//!
//! Lines git would not read as a pattern (blank ones, comments starting with `#`) and
//! patterns that are not valid globs are not rules: they match nothing.

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::Match;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The ignore patterns of a project, ready to be asked about paths.
///
/// Besides answering, the rules keep track of which patterns were *active*, i.e.
/// decided the fate of at least one path they were asked about (see
/// [`matched_patterns`](Self::matched_patterns)).
#[derive(Debug)]
pub struct IgnoreRules {
    project_root: PathBuf,
    matcher: Gitignore,
    /// The patterns as the caller wrote them, by the form the matcher reports them in.
    patterns_by_rule: HashMap<String, Vec<String>>,
    matched_patterns: Mutex<HashSet<String>>,
}

impl IgnoreRules {
    /// Builds the rules from `patterns`, anchored at `project_root`.
    pub fn new<P: AsRef<str>>(
        project_root: impl Into<PathBuf>,
        patterns: impl IntoIterator<Item = P>,
    ) -> Self {
        let project_root = project_root.into();

        // The order of the lines is what decides between contradicting patterns
        // (the last matching line wins). Negations go last so they take precedence;
        // sorting makes the remaining order independent of the caller's collection.
        let mut ordered: Vec<String> = patterns
            .into_iter()
            .map(|pattern| pattern.as_ref().to_string())
            .collect();
        ordered.sort_by(|a, b| (is_negation(a), a).cmp(&(is_negation(b), b)));
        ordered.dedup();

        let mut builder = GitignoreBuilder::new(&project_root);
        let mut patterns_by_rule: HashMap<String, Vec<String>> = HashMap::new();
        for pattern in ordered {
            // An invalid glob is not a rule; the other patterns stay in effect.
            if builder.add_line(None, &pattern).is_err() {
                tracing::warn!("Ignoring invalid ignore pattern {pattern:?}");
                continue;
            }
            patterns_by_rule
                .entry(rule_text(&pattern).to_string())
                .or_default()
                .push(pattern);
        }
        let matcher = builder.build().unwrap_or_else(|error| {
            tracing::warn!("Ignore patterns could not be compiled: {error}");
            Gitignore::empty()
        });

        Self {
            project_root,
            matcher,
            patterns_by_rule,
            matched_patterns: Mutex::new(HashSet::new()),
        }
    }

    /// `true` if there is no rule at all, so nothing is ever ignored.
    pub fn is_empty(&self) -> bool {
        self.matcher.is_empty()
    }

    /// `true` if `path` is hidden by the rules: it is excluded itself, or it lies in
    /// an excluded directory.
    ///
    /// `is_dir` tells whether `path` is a directory (patterns like `dir/` depend on
    /// it). A path outside the project root is never ignored.
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        if self.is_empty() {
            return false;
        }
        let Ok(relative) = path.strip_prefix(&self.project_root) else {
            return false;
        };
        // Top-down, the way a walk meets the entries: the first excluded directory on
        // the way to `path` hides it, whatever the rules say about `path` itself.
        let mut current = self.project_root.clone();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            current.push(component);
            let is_last = components.peek().is_none();
            if self.ignores_entry(&current, !is_last || is_dir) {
                return true;
            }
        }
        false
    }

    /// `true` if the rules exclude the entry `path` itself, not looking at the
    /// directories above it.
    ///
    /// This is the question of a top-down walk, which has asked about the parent
    /// directories already and does not descend into excluded ones. Everybody else
    /// wants [`is_ignored`](Self::is_ignored).
    pub fn ignores_entry(&self, path: &Path, is_dir: bool) -> bool {
        match self.matcher.matched(path, is_dir) {
            Match::None => false,
            Match::Ignore(glob) => {
                self.record_match(glob.original());
                true
            }
            Match::Whitelist(glob) => {
                self.record_match(glob.original());
                false
            }
        }
    }

    /// The patterns that were active so far: those that excluded, or as a negation
    /// re-included, at least one of the paths the rules were asked about.
    ///
    /// Of several patterns matching the same path, the one that decided is reported.
    pub fn matched_patterns(&self) -> HashSet<String> {
        self.lock_matched_patterns().clone()
    }

    fn record_match(&self, rule: &str) {
        let mut matched = self.lock_matched_patterns();
        match self.patterns_by_rule.get(rule) {
            Some(patterns) => matched.extend(patterns.iter().cloned()),
            None => {
                matched.insert(rule.to_string());
            }
        }
    }

    /// The set is plain data, so a lock poisoned by a panicking thread is taken over.
    fn lock_matched_patterns(&self) -> std::sync::MutexGuard<'_, HashSet<String>> {
        self.matched_patterns
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn is_negation(pattern: &str) -> bool {
    pattern.starts_with('!')
}

/// The form in which the matcher reports a pattern line: without trailing whitespace,
/// unless the last blank is escaped (the rule of `.gitignore` files).
fn rule_text(pattern: &str) -> &str {
    if pattern.ends_with("\\ ") {
        pattern
    } else {
        pattern.trim_end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/project";

    fn rules(patterns: &[&str]) -> IgnoreRules {
        IgnoreRules::new(ROOT, patterns)
    }

    fn file(relative: &str) -> (PathBuf, bool) {
        (Path::new(ROOT).join(relative), false)
    }

    fn dir(relative: &str) -> (PathBuf, bool) {
        (Path::new(ROOT).join(relative), true)
    }

    fn set(patterns: &[&str]) -> HashSet<String> {
        patterns.iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn patterns_follow_gitignore_semantics() {
        struct Case {
            name: &'static str,
            patterns: &'static [&'static str],
            ignored: Vec<(PathBuf, bool)>,
            kept: Vec<(PathBuf, bool)>,
        }
        let cases = [
            Case {
                name: "a plain name matches files and directories at any depth",
                patterns: &["target"],
                ignored: vec![
                    dir("target"),
                    file("target"),
                    dir("crates/a/target"),
                    file("target/debug/app"),
                    file("crates/a/target/x.o"),
                ],
                kept: vec![file("targets"), file("src/target.rs"), dir("src")],
            },
            Case {
                name: "a glob matches at any depth",
                patterns: &["*.log"],
                ignored: vec![file("a.log"), file("deep/down/b.log"), dir("odd.log")],
                kept: vec![file("a.log.txt"), file("log"), file("src/main.rs")],
            },
            Case {
                name: "a directory pattern matches directories only",
                patterns: &["build/"],
                ignored: vec![dir("build"), dir("sub/build"), file("build/out.txt")],
                kept: vec![file("build"), file("sub/build")],
            },
            Case {
                name: "an anchored pattern matches at the project root only",
                patterns: &["/x"],
                ignored: vec![file("x"), dir("x"), file("x/inner.txt")],
                kept: vec![file("sub/x"), dir("sub/x"), file("sub/x/inner.txt")],
            },
            Case {
                name: "a pattern with a slash in the middle is anchored as well",
                patterns: &["src/core/"],
                ignored: vec![dir("src/core"), file("src/core/scanner.rs")],
                kept: vec![dir("other/src/core"), file("src/core"), file("src/main.rs")],
            },
            Case {
                name: "a double star spans directories",
                patterns: &["docs/**/*.md"],
                ignored: vec![file("docs/a.md"), file("docs/x/y/b.md")],
                kept: vec![file("a.md"), file("docs/a.txt"), file("src/docs/a.md")],
            },
            Case {
                name: "a negation re-includes what another pattern excludes",
                patterns: &["*.log", "!keep.log"],
                ignored: vec![file("a.log"), file("sub/b.log")],
                kept: vec![file("keep.log"), file("sub/keep.log")],
            },
            Case {
                name: "a negation wins, in whatever order the patterns sort",
                patterns: &["!a.txt", "a.txt", "!z.txt", "z.txt", "*.txt"],
                ignored: vec![file("other.txt")],
                kept: vec![file("a.txt"), file("z.txt")],
            },
            Case {
                name: "a negation cannot re-include below an excluded directory",
                patterns: &["out", "!out/keep.txt", "!keep.txt"],
                ignored: vec![dir("out"), file("out/keep.txt"), file("out/a/keep.txt")],
                kept: vec![file("keep.txt"), file("src/keep.txt")],
            },
            Case {
                name: "contents can be excluded while the directory and one file stay",
                patterns: &["out/*", "!out/keep.txt"],
                ignored: vec![file("out/a.txt"), dir("out/sub"), file("out/sub/keep.txt")],
                kept: vec![dir("out"), file("out/keep.txt")],
            },
            Case {
                name: "a negation alone excludes nothing",
                patterns: &["!src"],
                ignored: vec![],
                kept: vec![dir("src"), file("src/main.rs"), file("README.md")],
            },
            Case {
                name: "blank lines, comments and invalid globs are not rules",
                patterns: &["", "   ", "# comment", "a[", "*.tmp"],
                ignored: vec![file("x.tmp")],
                kept: vec![file("# comment"), file("a["), file("comment")],
            },
            Case {
                name: "paths outside the project root are never ignored",
                patterns: &["*.log", "elsewhere"],
                ignored: vec![],
                kept: vec![
                    (PathBuf::from("/elsewhere/a.log"), false),
                    (PathBuf::from("/elsewhere"), true),
                    (PathBuf::from("relative/a.log"), false),
                ],
            },
        ];

        for case in cases {
            let rules = rules(case.patterns);
            for (path, is_dir) in &case.ignored {
                assert!(
                    rules.is_ignored(path, *is_dir),
                    "{}: {path:?} (dir: {is_dir}) should be ignored",
                    case.name
                );
            }
            for (path, is_dir) in &case.kept {
                assert!(
                    !rules.is_ignored(path, *is_dir),
                    "{}: {path:?} (dir: {is_dir}) should be kept",
                    case.name
                );
            }
        }
    }

    #[test]
    fn the_order_of_the_patterns_does_not_matter() {
        let forwards = rules(&["*.txt", "!a.txt", "docs/"]);
        let backwards = rules(&["docs/", "!a.txt", "*.txt"]);

        for (path, is_dir) in [
            file("a.txt"),
            file("b.txt"),
            dir("docs"),
            file("docs/a.txt"),
        ] {
            assert_eq!(
                forwards.is_ignored(&path, is_dir),
                backwards.is_ignored(&path, is_dir),
                "{path:?}"
            );
        }
    }

    #[test]
    fn no_patterns_means_no_rules() {
        let rules = rules(&[]);

        assert!(rules.is_empty());
        assert!(!rules.is_ignored(&file("anything").0, false));
        assert!(rules.matched_patterns().is_empty());
        assert!(IgnoreRules::new(ROOT, ["", "# only a comment"]).is_empty());
    }

    #[test]
    fn matched_patterns_are_those_that_decided_about_a_path() {
        let rules = rules(&["*.log", "target", "never_matches", "!keep.log"]);
        assert!(rules.matched_patterns().is_empty());

        assert!(rules.is_ignored(&file("a.log").0, false));
        assert_eq!(rules.matched_patterns(), set(&["*.log"]));

        assert!(!rules.is_ignored(&file("src/main.rs").0, false));
        assert_eq!(rules.matched_patterns(), set(&["*.log"]));

        // The directory decides; the rules are not asked about the file below it.
        assert!(rules.is_ignored(&file("target/debug/b.log").0, false));
        assert_eq!(rules.matched_patterns(), set(&["*.log", "target"]));

        assert!(!rules.is_ignored(&file("keep.log").0, false));
        assert_eq!(
            rules.matched_patterns(),
            set(&["*.log", "target", "!keep.log"])
        );
    }

    #[test]
    fn matched_patterns_are_reported_as_the_caller_wrote_them() {
        let rules = rules(&["*.log  ", "*.log", "/x"]);

        assert!(rules.is_ignored(&file("a.log").0, false));
        assert!(rules.is_ignored(&file("x").0, false));

        assert_eq!(rules.matched_patterns(), set(&["*.log  ", "*.log", "/x"]));
    }

    #[test]
    fn ignores_entry_looks_at_the_entry_alone() {
        let rules = rules(&["target", "/x"]);

        assert!(rules.ignores_entry(&dir("target").0, true));
        // The walk would never get here; asked anyway, the entry itself is not excluded.
        assert!(!rules.ignores_entry(&file("target/debug/app").0, false));
        assert!(rules.is_ignored(&file("target/debug/app").0, false));
    }
}
