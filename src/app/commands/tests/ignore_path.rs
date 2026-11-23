use super::*;

/// No scan is running, so the new pattern is applied to the loaded list directly;
/// no scan is started for it.
#[tokio::test]
async fn test_add_ignore_path_applies_pattern_to_loaded_list() {
    let mut harness = TestHarness::new();
    harness.create_file("src/main.rs", "");
    harness.create_dir("docs");
    harness.create_file("docs/guide.md", "");
    harness.set_initial_files(&["src", "docs", "src/main.rs", "docs/guide.md"]);

    let path_to_ignore = harness.root_path.join("docs");
    let payload = json!(path_to_ignore);
    add_ignore_path(typed(payload), harness.proxy.clone(), harness.state.clone()).await;

    let final_state = harness.last_state_update().unwrap();
    assert!(!final_state.is_scanning, "No scan must have been started");
    assert_eq!(final_state.visible_files_count, 2);
    assert!(!final_state
        .root_rows()
        .into_iter()
        .any(|n| n.name == "docs"));
    let state = harness.state.lock();
    assert!(!state.is_scanning() && !state.is_fully_scanned);
    assert!(state.config().ignore_patterns.contains("/docs/"));
}

#[tokio::test]
async fn test_add_ignore_path_does_nothing_when_no_directory_is_loaded() {
    let mut harness = TestHarness::new();
    let initial_patterns_count;
    {
        let mut state = harness.state.lock();
        state.current_path = String::new();
        initial_patterns_count = state.config().ignore_patterns.len();
    }

    let payload = json!("/some/path/to/ignore.txt");
    add_ignore_path(typed(payload), harness.proxy.clone(), harness.state.clone()).await;

    {
        let final_patterns_count = harness.state.lock().config().ignore_patterns.len();
        assert_eq!(
            initial_patterns_count, final_patterns_count,
            "Ignore patterns should not change"
        );
    }

    // Like every config command it is answered, with the unchanged state.
    assert_eq!(harness.state_updates_sent(), 1);
    harness
        .expect_no_event_within(QUIET_PERIOD, "Nothing else is sent")
        .await;
}

#[tokio::test]
async fn test_add_ignore_path_handles_path_outside_root() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["src/main.rs"]);
    let initial_patterns_count = harness.state.lock().config().ignore_patterns.len();
    let outside_path = json!("/etc/hosts");

    add_ignore_path(
        typed(outside_path),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    // Answered with the unchanged state.
    assert_eq!(harness.state_updates_sent(), 1);
    harness
        .expect_no_event_within(QUIET_PERIOD, "Nothing else is sent")
        .await;
    let final_patterns_count = harness.state.lock().config().ignore_patterns.len();
    assert_eq!(initial_patterns_count, final_patterns_count);
}

/// The names of a nested path are joined with `/` whatever the separator of the
/// platform is, and the pattern is anchored at the root. The paths are built with
/// `join`, so on Windows they read `src\gen\out`, which is not what a gitignore
/// pattern looks like.
#[test]
fn test_gitignore_pattern_is_anchored_and_joins_the_names_with_slashes() {
    let nested = Path::new("src").join("gen").join("out");
    assert_eq!(
        gitignore_pattern(&nested, true).as_deref(),
        Ok("/src/gen/out/")
    );
    assert_eq!(
        gitignore_pattern(&nested.join("a b.rs"), false).as_deref(),
        Ok("/src/gen/out/a b.rs")
    );
    assert_eq!(
        gitignore_pattern(Path::new("docs"), true).as_deref(),
        Ok("/docs/")
    );
    assert_eq!(
        gitignore_pattern(Path::new("notes.txt"), false).as_deref(),
        Ok("/notes.txt")
    );
}

/// A path as Windows spells it.
#[cfg(windows)]
#[test]
fn test_gitignore_pattern_of_a_path_with_backslashes() {
    assert_eq!(
        gitignore_pattern(Path::new(r"src\gen"), true).as_deref(),
        Ok("/src/gen/")
    );
    assert_eq!(
        gitignore_pattern(Path::new(r"src\gen\mod.rs"), false).as_deref(),
        Ok("/src/gen/mod.rs")
    );
}

/// Only a path that names an entry below the root has a pattern.
#[test]
fn test_gitignore_pattern_needs_a_path_below_the_root() {
    let outside = Err(NoPattern::NotInTheLoadedDirectory);
    assert_eq!(gitignore_pattern(Path::new(""), true), outside);
    assert_eq!(
        gitignore_pattern(&Path::new("..").join("other"), false),
        outside
    );
    assert_eq!(gitignore_pattern(Path::new("/absolute"), false), outside);
}

/// A name that is not valid Unicode has no pattern: a pattern is text.
#[cfg(unix)]
#[test]
fn test_gitignore_pattern_of_a_name_that_is_not_unicode_is_none() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let name = OsStr::from_bytes(b"bad\xff.txt");

    let refused = Err(NoPattern::Refused(IgnorePathRefusal::NotUnicode {
        name: "bad\u{fffd}.txt".to_string(),
    }));
    assert_eq!(gitignore_pattern(Path::new(name), false), refused);
    assert_eq!(
        gitignore_pattern(&Path::new("src").join(name), true),
        refused
    );
    // The name that stands in the way is the one that is named.
    assert_eq!(
        gitignore_pattern(&Path::new(name).join("inside.txt"), false),
        refused
    );
}

/// An entry of a project (path relative to the root, with `/`) and whether it is
/// a directory.
type Entry = (&'static str, bool);

const FILE: bool = false;

const DIR: bool = true;

/// Asserts that the pattern made for `entry` hides `entry` (and, for a directory,
/// what lies in it) and none of `others`, by the rules the application applies.
#[track_caller]
fn assert_pattern_hides_only(entry: Entry, others: &[Entry]) {
    let root = Path::new("/project");
    let path_of = |relative: &str| {
        relative
            .split('/')
            .fold(root.to_path_buf(), |p, n| p.join(n))
    };
    let (relative, is_directory) = entry;
    let pattern = gitignore_pattern(path_of(relative).strip_prefix(root).unwrap(), is_directory)
        .unwrap_or_else(|_| panic!("{relative:?} has a pattern"));
    let rules = crate::core::IgnoreRules::new(root, [&pattern]);

    assert!(
        rules.is_ignored(&path_of(relative), is_directory),
        "{pattern:?} must hide {relative:?}"
    );
    if is_directory {
        assert!(
            rules.is_ignored(&path_of(relative).join("inside.txt"), false),
            "{pattern:?} must hide what lies in {relative:?}"
        );
    }
    for (other, other_is_directory) in others {
        assert!(
            !rules.is_ignored(&path_of(other), *other_is_directory),
            "{pattern:?}, made for {relative:?}, must not hide {other:?}"
        );
    }
}

/// The pattern is anchored: it hides the entry whose button was pressed, not the
/// entries of the same name elsewhere in the project.
#[test]
fn test_pattern_of_an_entry_does_not_hide_its_namesakes() {
    assert_pattern_hides_only(
        ("README.md", FILE),
        &[("sub/README.md", FILE), ("a/b/README.md", FILE)],
    );
    assert_pattern_hides_only(("docs", DIR), &[("src/docs", DIR), ("src/docs/x.md", FILE)]);
    assert_pattern_hides_only(
        ("src/gen", DIR),
        &[("gen", DIR), ("other/src/gen", DIR), ("src/general", DIR)],
    );
}

/// Wildcards in a name are taken literally.
#[test]
fn test_pattern_of_a_name_with_wildcards_hides_only_that_name() {
    assert_pattern_hides_only(("a*b.txt", FILE), &[("aXb.txt", FILE), ("ab.txt", FILE)]);
    assert_pattern_hides_only(("a?b.txt", FILE), &[("aXb.txt", FILE)]);
    assert_pattern_hides_only(
        ("**", FILE),
        &[("other.txt", FILE), ("sub", DIR), ("sub/deep.txt", FILE)],
    );
    assert_pattern_hides_only(("**", DIR), &[("sub", DIR), ("sub/deep.txt", FILE)]);
    assert_pattern_hides_only(
        ("src/a*b/x?.rs", FILE),
        &[("src/aXb/xy.rs", FILE), ("src/a*b/xy.rs", FILE)],
    );
}

/// Brackets and braces in a name are neither a character class nor alternatives.
#[test]
fn test_pattern_of_a_name_with_brackets_or_braces_hides_only_that_name() {
    assert_pattern_hides_only(("a[1].txt", FILE), &[("a1.txt", FILE)]);
    assert_pattern_hides_only(("dir[x]", DIR), &[("dirx", DIR), ("dirx/f.txt", FILE)]);
    assert_pattern_hides_only(("a]b", FILE), &[("ab", FILE)]);
    assert_pattern_hides_only(("a{b,c}.txt", FILE), &[("ab.txt", FILE), ("ac.txt", FILE)]);
    assert_pattern_hides_only(("{x}", DIR), &[("x", DIR)]);
}

/// A name that begins like a negation or a comment is a name.
#[test]
fn test_pattern_of_a_name_that_begins_with_a_bang_or_a_hash_hides_that_name() {
    assert_pattern_hides_only(("!important", FILE), &[("important", FILE)]);
    assert_pattern_hides_only(("!keep", DIR), &[("keep", DIR)]);
    assert_pattern_hides_only(("#notes", FILE), &[("notes", FILE)]);
    assert_pattern_hides_only(("src/!x", FILE), &[("src/x", FILE)]);
}

/// Spaces at the end of a name belong to the name.
#[test]
fn test_pattern_of_a_name_that_ends_with_spaces_hides_only_that_name() {
    assert_pattern_hides_only(("trail ", FILE), &[("trail", FILE), ("trail  ", FILE)]);
    assert_pattern_hides_only(("trail  ", FILE), &[("trail", FILE), ("trail ", FILE)]);
    assert_pattern_hides_only(("dir ", DIR), &[("dir", DIR)]);
    assert_pattern_hides_only(("sp /f.txt", FILE), &[("sp/f.txt", FILE)]);
    assert_pattern_hides_only((" lead", FILE), &[("lead", FILE)]);
    assert_eq!(
        gitignore_pattern(Path::new("trail  "), false).as_deref(),
        Ok("/trail [ ]")
    );
}

/// So does every other kind of ASCII whitespace: the matcher would drop it from
/// the end of a pattern, and the pattern would hide the name without it instead.
#[test]
fn test_pattern_of_a_name_that_ends_with_ascii_whitespace_hides_only_that_name() {
    assert_eq!(
        gitignore_pattern(Path::new("tab\t"), false).as_deref(),
        Ok("/tab[\t]")
    );
    assert_pattern_hides_only(
        ("tab\t", FILE),
        &[
            ("tab", FILE),
            ("tab ", FILE),
            ("tab\t\t", FILE),
            ("tabX", FILE),
        ],
    );
    for whitespace in ['\t', '\n', '\u{b}', '\u{c}', '\r', ' '] {
        assert!(whitespace.is_ascii() && whitespace.is_whitespace());
        let name: &'static str = format!("name{whitespace}").leak();
        assert_pattern_hides_only((name, FILE), &[("name", FILE), ("name_", FILE)]);
        let nested: &'static str = format!("src/name{whitespace}").leak();
        assert_pattern_hides_only((nested, FILE), &[("src/name", FILE), (name, FILE)]);
        let directory: &'static str = format!("dir{whitespace}").leak();
        assert_pattern_hides_only((directory, DIR), &[("dir", DIR), ("dir/f.txt", FILE)]);
    }
    // Only the last character is written differently; the whitespace before it,
    // of whatever kind, is kept by it.
    assert_pattern_hides_only(
        ("mixed \t", FILE),
        &[("mixed", FILE), ("mixed ", FILE), ("mixed\t", FILE)],
    );
    assert_pattern_hides_only(
        ("mixed\t ", FILE),
        &[("mixed", FILE), ("mixed\t", FILE), ("mixed ", FILE)],
    );
    assert_pattern_hides_only(
        ("nbsp\u{a0} ", FILE),
        &[("nbsp", FILE), ("nbsp\u{a0}", FILE)],
    );
    // Whitespace that is not at the end is nothing special.
    assert_pattern_hides_only(("a\tb", FILE), &[("ab", FILE), ("a b", FILE)]);
    // A name that would close the class, or escape its closing bracket.
    assert_pattern_hides_only(("a] ", FILE), &[("a]", FILE), ("a", FILE)]);
    #[cfg(unix)]
    assert_pattern_hides_only((r"a\ ", FILE), &[(r"a\", FILE), ("a ", FILE)]);
}

/// A file name that ends with whitespace outside ASCII has no pattern: the
/// matcher would drop that whitespace and hide the name without it.
#[test]
fn test_gitignore_pattern_of_a_file_that_ends_with_other_whitespace_is_none() {
    for whitespace in ['\u{a0}', '\u{3000}', '\u{85}', '\u{2028}', '\u{2003}'] {
        assert!(!whitespace.is_ascii() && whitespace.is_whitespace());
        let name = format!("name{whitespace}");
        let refused = |name: &str| {
            Err(NoPattern::Refused(IgnorePathRefusal::TrailingWhitespace {
                name: name.to_string(),
            }))
        };
        assert_eq!(gitignore_pattern(Path::new(&name), false), refused(&name));
        assert_eq!(
            gitignore_pattern(&Path::new("src").join(&name), false),
            refused(&name)
        );
        // ASCII whitespace before it does not change that.
        let name = format!("name {whitespace}");
        assert_eq!(gitignore_pattern(Path::new(&name), false), refused(&name));

        // Elsewhere in a path it is a character like any other: the slash of a
        // directory pattern stands behind it, and so does the rest of a name.
        let directory: &'static str = format!("dir{whitespace}").leak();
        assert_pattern_hides_only((directory, DIR), &[("dir", DIR), ("dir/f.txt", FILE)]);
        let inside: &'static str = format!("dir{whitespace}/f.txt").leak();
        assert_pattern_hides_only((inside, FILE), &[("dir/f.txt", FILE)]);
        let middle: &'static str = format!("a{whitespace}b").leak();
        assert_pattern_hides_only((middle, FILE), &[("ab", FILE), ("a", FILE)]);
    }
}

/// The ignore button on a file whose name has no pattern adds nothing, and what
/// its pattern would have been (the name without the whitespace) stays visible.
/// The user is told why nothing happened, and the command is answered with a
/// state update like every other.
#[tokio::test]
async fn test_add_ignore_path_for_a_name_that_ends_with_other_whitespace_adds_nothing() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["name\u{a0}", "name"]);
    let patterns = harness.state.lock().config().ignore_patterns.clone();

    add_ignore_path(
        typed(json!(harness.root_path.join("name\u{a0}"))),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    {
        let state = harness.state.lock();
        assert_eq!(state.config().ignore_patterns, patterns);
        assert_eq!(state.full_file_list().len(), 2);
    }
    let events: Vec<UserEvent> = std::iter::from_fn(|| harness.event_rx.try_recv().ok()).collect();
    assert!(
        matches!(
            events.as_slice(),
            [
                UserEvent::IgnorePathRefused(IgnorePathRefusal::TrailingWhitespace { name }),
                UserEvent::StateUpdate(_)
            ] if name == "name\u{a0}"
        ),
        "{events:?}"
    );
}

/// The same for a name that is not valid Unicode. The UI holds such a path as
/// text (lossily), and that is what it sends: the entry is recognized by it, and
/// no pattern is made of a path that names nothing.
#[cfg(unix)]
#[tokio::test]
async fn test_add_ignore_path_for_a_name_that_is_not_unicode_says_so() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let mut harness = TestHarness::new();
    let path = harness.root_path.join(OsStr::from_bytes(b"bad\xff.txt"));
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![file_item(path.clone(), false)]);
        filtering::apply_filters(&mut state);
    }
    let patterns = harness.state.lock().config().ignore_patterns.clone();

    add_ignore_path(
        typed(json!(path.to_string_lossy())),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    assert_eq!(harness.state.lock().config().ignore_patterns, patterns);
    let events: Vec<UserEvent> = std::iter::from_fn(|| harness.event_rx.try_recv().ok()).collect();
    assert!(
        matches!(
            events.as_slice(),
            [
                UserEvent::IgnorePathRefused(IgnorePathRefusal::NotUnicode { name }),
                UserEvent::StateUpdate(_)
            ] if name == "bad\u{fffd}.txt"
        ),
        "{events:?}"
    );

    // A name with U+FFFD in it that is listed as such is a name like any other.
    let literal = harness.root_path.join("odd\u{fffd}.txt");
    harness
        .state
        .lock()
        .set_full_file_list(vec![file_item(literal.clone(), false)]);
    assert_eq!(
        ignore_pattern_for_path(&harness.state.lock(), &literal).as_deref(),
        Ok("/odd\u{fffd}.txt")
    );
}

/// A path that is no entry of the loaded directory adds nothing either, and
/// that is all: there is no row whose button could have been pressed.
#[tokio::test]
async fn test_add_ignore_path_outside_the_loaded_directory_is_not_reported() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["name"]);

    add_ignore_path(
        typed(json!("/somewhere/else")),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    assert_eq!(harness.state_updates_sent(), 1);
}

/// A backslash in a name (not a separator on Unix) is that character, not an
/// escape of the one after it.
#[cfg(unix)]
#[test]
fn test_pattern_of_a_name_with_a_backslash_hides_only_that_name() {
    assert_pattern_hides_only((r"back\slash", FILE), &[("backslash", FILE)]);
    assert_pattern_hides_only((r"mid\dle/x", FILE), &[("middle/x", FILE)]);
    assert_pattern_hides_only((r"end\", FILE), &[("end", FILE)]);
    assert_pattern_hides_only((r"end\", DIR), &[("end", DIR)]);
    assert_pattern_hides_only((r"a\*b", FILE), &[("a*b", FILE), (r"a\Xb", FILE)]);
}

/// The pattern of a nested directory removes that directory, and only it, from
/// the loaded list: it is a valid gitignore pattern on every platform.
#[tokio::test]
async fn test_add_ignore_path_for_a_nested_directory_uses_slashes() {
    let harness = TestHarness::new();
    harness.create_file("src/gen/out.rs", "");
    harness.create_file("src/main.rs", "");
    harness.create_file("gen/keep.rs", "");
    harness.set_initial_files(&[
        "src",
        "src/gen",
        "src/gen/out.rs",
        "src/main.rs",
        "gen",
        "gen/keep.rs",
    ]);
    let nested = harness.root_path.join("src").join("gen");

    add_ignore_path(
        typed(json!(nested)),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    let state = harness.state.lock();
    assert!(
        state.config().ignore_patterns.contains("/src/gen/"),
        "{:?}",
        state.config().ignore_patterns
    );
    let remaining: HashSet<PathBuf> = state
        .full_file_list()
        .iter()
        .map(|item| item.path.clone())
        .collect();
    let root = &harness.root_path;
    assert_eq!(
        remaining,
        HashSet::from([
            root.join("src"),
            root.join("src").join("main.rs"),
            root.join("gen"),
            root.join("gen").join("keep.rs"),
        ])
    );
}

/// Whether the path is a directory is taken from the loaded list, not from the
/// disk: the command runs on the ordered lane, which does no file IO.
#[tokio::test]
async fn test_add_ignore_path_takes_the_kind_of_the_path_from_the_loaded_list() {
    let mut harness = TestHarness::new();
    // Nothing of this exists on disk.
    let dir = harness.root_path.join("build");
    let file = harness.root_path.join("notes");
    {
        let mut state = harness.state.lock();
        state.set_full_file_list(vec![
            file_item(dir.clone(), true),
            file_item(file.clone(), false),
        ]);
        filtering::apply_filters(&mut state);
    }

    add_ignore_path(
        typed(json!(dir)),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
    add_ignore_path(
        typed(json!(file)),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    assert_eq!(harness.state_updates_sent(), 2);
    let state = harness.state.lock();
    assert!(state.config().ignore_patterns.contains("/build/"));
    assert!(state.config().ignore_patterns.contains("/notes"));
    assert!(state.full_file_list().is_empty());
}

#[tokio::test]
async fn test_add_ignore_path_handles_duplicate_pattern() {
    let mut harness = TestHarness::new();
    harness.set_initial_files(&["docs/guide.md"]);

    let path_to_ignore = harness.root_path.join("docs");
    let payload = json!(path_to_ignore);
    add_ignore_path(
        typed(payload.clone()),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;

    // The first call has been answered (no scan runs for it); drop its events.
    assert!(harness.last_state_update().is_some());

    let _ = harness.last_state_update();
    let patterns_before = harness.state.lock().config().ignore_patterns.clone();

    add_ignore_path(typed(payload), harness.proxy.clone(), harness.state.clone()).await;

    // The command is answered, but nothing has changed and no scan is started.
    let ui_state = harness
        .last_state_update()
        .expect("the command is answered");
    assert!(!ui_state.is_scanning);
    assert!(!ui_state.patterns_need_rescan);
    let state = harness.state.lock();
    assert!(!state.is_scanning());
    assert_eq!(state.config().ignore_patterns, patterns_before);
}
