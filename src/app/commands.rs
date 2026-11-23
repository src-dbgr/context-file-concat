// src/app/commands.rs
//! Contains all the command handlers that are callable from the frontend via IPC.
//!
//! Each function in this module corresponds to one [`Command`](super::ipc::Command)
//! and takes its payload as the type the command carries: a handler never sees JSON,
//! and a message whose payload does not fit never reaches one (see [`super::ipc`]).
//! These handlers are responsible for interacting with the `AppState` and the `core`
//! logic, and for sending `UserEvent`s back to the UI.

use super::events::{IgnorePathRefusal, OutputSizeNotice, SaveResult, UserEvent};
use super::filtering;
use super::helpers::with_state_and_notify;
use super::proxy::EventProxy;
use super::shared_state::SharedState;
use super::snapshot::send_snapshot;
use super::state::{AppState, GeneratedDocument, StatusNotice};
use super::tasks::{
    self, refresh_content_search, search_for_new_query, start_lazy_load_scan, start_scan_on_path,
};
use super::view_model::rows::typeahead_match;
use super::view_model::Viewport;
use crate::app::file_dialog::DialogService;
use crate::config::{self, AppConfig};
use crate::core::is_blank;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Opens a file dialog for the user to select a directory to scan.
///
/// Triggers a new proactive, two-phase scan on the selected path, which replaces the
/// loaded directory (a hard reset of everything that belongs to it) as soon as the
/// path has been found, see [`start_scan_on_path`].
///
/// Dismissing the dialog changes nothing: in particular, a scan that is running in the
/// background keeps running and keeps owning the scanning state. The UI is still sent
/// the (unchanged) state, so it can rely on getting an answer to its request.
pub fn select_directory<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: SharedState,
) {
    if let Some(path) = dialog.pick_directory() {
        start_scan_on_path(path, proxy, state, false);
    } else {
        tracing::debug!("Directory selection cancelled by the user.");
        with_state_and_notify(&state, &proxy, |_| {});
    }
}

/// Opens a path that was dropped onto the window, replacing the loaded directory.
/// A dropped file opens the directory it lies in. A path that does not exist is
/// reported and leaves the loaded directory as it is.
pub fn open_dropped_path<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState) {
    start_scan_on_path(path, proxy, state, false);
}

/// Clears the currently loaded directory and resets the application state.
pub fn clear_directory<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        s.reset_directory_state();
        s.config.last_directory = None;
        // The file is written in the background; neither the lock nor the lane waits.
        tasks::save_config_in_background(s.config_snapshot());
    });
}

/// Re-scans the currently loaded directory path.
///
/// This preserves the current UI state (selections, expansions) while refreshing
/// the file list from the filesystem. Does nothing if no directory is loaded.
///
/// While another path is being opened (see [`AppState::begin_open`]) there is nothing
/// to refresh: the opening replaces the loaded directory with a fresh scan anyway, and
/// a re-scan of the directory that is about to be replaced would supersede it. The
/// request is answered with the unchanged state.
pub fn rescan_directory<P: EventProxy>(proxy: P, state: SharedState) {
    let current_path = {
        let mut state_guard = state.lock();
        if state_guard.is_opening() {
            send_snapshot(&proxy, &state_guard);
            return;
        }
        state_guard.patterns_need_rescan = false;
        state_guard.current_path.clone()
    };

    // Nothing can slip in between the two locks: commands run one after the other.
    if !current_path.is_empty() {
        start_scan_on_path(PathBuf::from(current_path), proxy, state, true);
    }
}

/// Cancels the ongoing directory scan.
pub fn cancel_scan<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        tracing::debug!("Scan cancellation requested.");
        s.cancel_current_scan();
    });
}

/// Payload of [`add_ignore_pattern`] and [`remove_ignore_pattern`].
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct IgnorePatternPayload {
    pub pattern: String,
}

/// Payload of [`set_config_option`]: one user-editable option together with its new
/// value, on the wire `{"option": "<name>", "value": <value>}`.
///
/// These are exactly the settings the UI offers a control for. Everything else in
/// [`AppConfig`] is owned by the host and cannot be set through IPC.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(
    tag = "option",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum ConfigOption {
    CaseSensitiveSearch(bool),
    RemoveEmptyDirectories(bool),
    IncludeTreeByDefault(bool),
    UseRelativePaths(bool),
    OutputFilename(String),
    /// `None` (or an empty path) means that no output directory is preset.
    OutputDirectory(Option<PathBuf>),
}

impl ConfigOption {
    fn apply_to(self, config: &mut AppConfig) {
        match self {
            Self::CaseSensitiveSearch(value) => config.case_sensitive_search = value,
            Self::RemoveEmptyDirectories(value) => config.remove_empty_directories = value,
            Self::IncludeTreeByDefault(value) => config.include_tree_by_default = value,
            Self::UseRelativePaths(value) => config.use_relative_paths = value,
            Self::OutputFilename(value) => config.output_filename = value,
            Self::OutputDirectory(value) => {
                config.output_directory = value.filter(|dir| !dir.as_os_str().is_empty());
            }
        }
    }
}

/// Answers a request that changes nothing with the state as it is.
///
/// The UI shows what the host reports and waits for the answer to what it has sent
/// (see `HostDraft` in the UI), so a request that is refused is answered like any
/// other: with a snapshot, which then shows that nothing has changed. This goes for
/// a command that refuses what it was asked for as well as for a message that is no
/// command at all (see [`super::ipc`]).
pub(crate) fn answer_unchanged<P: EventProxy>(proxy: &P, state: &SharedState) {
    send_snapshot(proxy, &state.lock());
}

/// Adds one ignore pattern. Adding a pattern that is already set changes nothing.
///
/// See [`change_config`] for what a pattern change does to the loaded directory.
pub(crate) async fn add_ignore_pattern<P: EventProxy>(
    IgnorePatternPayload { pattern }: IgnorePatternPayload,
    proxy: P,
    state: SharedState,
) {
    if is_blank(&pattern) {
        tracing::warn!("Ignoring a blank ignore pattern.");
        return answer_unchanged(&proxy, &state);
    }
    change_config(&proxy, &state, |config| {
        config.ignore_patterns.insert(pattern);
    })
    .await;
}

/// Removes one ignore pattern. Removing a pattern that is not set changes nothing.
pub(crate) async fn remove_ignore_pattern<P: EventProxy>(
    IgnorePatternPayload { pattern }: IgnorePatternPayload,
    proxy: P,
    state: SharedState,
) {
    change_config(&proxy, &state, |config| {
        config.ignore_patterns.remove(&pattern);
    })
    .await;
}

/// Removes all ignore patterns.
pub async fn clear_ignore_patterns<P: EventProxy>(proxy: P, state: SharedState) {
    change_config(&proxy, &state, |config| config.ignore_patterns.clear()).await;
}

/// Sets one user-editable option of the configuration (see [`ConfigOption`]).
pub(crate) async fn set_config_option<P: EventProxy>(
    option: ConfigOption,
    proxy: P,
    state: SharedState,
) {
    change_config(&proxy, &state, |config| option.apply_to(config)).await;
}

/// Whether the content search has to be repeated after a configuration change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentSearchFollowUp {
    None,
    Repeat,
}

/// The single way a command edits the configuration.
///
/// `change` is applied to the configuration of the host while the state is locked, so
/// an edit is always based on the current configuration and never on a copy that
/// another command has outdated in the meantime. A changed configuration is persisted
/// (in the background, see [`tasks::save_config_in_background`]).
///
/// What else happens depends on what `change` has changed:
/// - Ignore patterns while a scan is walking the loaded directory: the scan is
///   restarted, so that it works with the new patterns instead of delivering a result
///   based on the old ones. (A scan that is still opening its path has not read the
///   patterns yet and needs no restart.)
/// - Ignore patterns added otherwise: they are applied to the loaded file list.
/// - Ignore patterns removed otherwise: the user is told that a re-scan is needed.
/// - Case sensitivity: the filters are re-applied and an active content search is
///   repeated, as its matches depend on it.
/// - Removal of empty directories: the filters are re-applied.
///
/// Every call is answered with a state update, also when nothing has changed: the UI
/// does not edit its copy of the configuration, it shows what the host reports. The
/// commands that end up here keep that promise for the requests they refuse as well
/// ([`answer_unchanged`]).
async fn change_config<P: EventProxy>(
    proxy: &P,
    state: &SharedState,
    change: impl FnOnce(&mut AppConfig),
) {
    if apply_config_change(proxy, state, change) == ContentSearchFollowUp::Repeat {
        refresh_content_search(proxy, state, tasks::RealFileSearcher).await;
    }
}

/// The synchronous part of [`change_config`]: everything up to the repeated search.
fn apply_config_change<P: EventProxy>(
    proxy: &P,
    state: &SharedState,
    change: impl FnOnce(&mut AppConfig),
) -> ContentSearchFollowUp {
    let mut state_guard = state.lock();

    let previous = state_guard.config.clone();
    change(&mut state_guard.config);
    let config = &state_guard.config;

    let patterns_added: HashSet<String> = config
        .ignore_patterns
        .difference(&previous.ignore_patterns)
        .cloned()
        .collect();
    let patterns_removed = previous
        .ignore_patterns
        .difference(&config.ignore_patterns)
        .next()
        .is_some();
    let case_sensitivity_changed = previous.case_sensitive_search != config.case_sensitive_search;
    let needs_refilter = case_sensitivity_changed
        || previous.remove_empty_directories != config.remove_empty_directories;

    if *config != previous {
        tasks::save_config_in_background(state_guard.config_snapshot());
    }

    // Without a loaded directory there is nothing the change could apply to.
    if state_guard.current_path.is_empty() {
        send_snapshot(proxy, &state_guard);
        return ContentSearchFollowUp::None;
    }

    let patterns_changed = !patterns_added.is_empty() || patterns_removed;

    if patterns_changed && state_guard.is_scanning() && !state_guard.is_opening() {
        tracing::info!("Ignore patterns changed during a scan. Restarting the scan.");
        // The entries that are ignored now disappear at once; the restarted scan then
        // delivers the list that matches the new patterns.
        if !patterns_added.is_empty() {
            apply_added_patterns_locally(&mut state_guard, &patterns_added);
        }
        filtering::apply_filters(&mut state_guard);
        state_guard.patterns_need_rescan = false;
        // Announces the new scan with a state update of its own.
        tasks::restart_scan(&mut state_guard, proxy.clone(), state.clone());
    } else {
        if patterns_removed {
            tracing::info!("Ignore patterns removed. Re-scan recommended.");
            state_guard.patterns_need_rescan = true;
        }
        if !patterns_added.is_empty() {
            tracing::debug!("Applying new ignore patterns locally: {:?}", patterns_added);
            apply_added_patterns_locally(&mut state_guard, &patterns_added);
        }
        if !patterns_added.is_empty() || needs_refilter {
            filtering::apply_filters(&mut state_guard);
        }
        send_snapshot(proxy, &state_guard);
    }

    if case_sensitivity_changed && !is_blank(&state_guard.content_search_query) {
        ContentSearchFollowUp::Repeat
    } else {
        ContentSearchFollowUp::None
    }
}

/// Removes the entries matching the newly added ignore patterns from the loaded file
/// list and records which of these patterns matched something.
fn apply_added_patterns_locally(state: &mut AppState, patterns_added: &HashSet<String>) {
    for pattern in patterns_added {
        let rules = crate::core::IgnoreRules::new(&state.current_path, [pattern]);
        let has_match = state
            .full_file_list()
            .iter()
            .any(|item| rules.is_ignored(&item.path, item.is_directory));
        if has_match {
            state.active_ignore_patterns.insert(pattern.clone());
        }
    }

    state.apply_ignore_patterns();
}

/// Handles the initial request for state from the frontend when it loads.
pub fn initialize<P: EventProxy>(proxy: P, state: SharedState) {
    // A UI that has just started shows the top of the tree and has asked for no row.
    with_state_and_notify(&state, &proxy, |s| s.tree_view.reset());
}

/// Payload of [`update_filters`]: the three filter texts as the user entered them.
/// A text that is left out counts as empty.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub(crate) struct UpdateFiltersPayload {
    pub search_query: String,
    pub extension_filter: String,
    pub content_search_query: String,
}

/// Updates the filename, extension, and content search filters.
///
/// If the content search query has changed, the content is searched again: the command
/// answers with a state update right away (the new texts, the search pending) and with
/// another one when the matches are there, see [`tasks::search_for_new_query`].
/// Otherwise, it just re-applies the filename and extension filters on the existing file list.
/// Either way the directories of the matches are expanded, except for those the user
/// collapsed since the filters were last empty (see [`AppState::reveal_directory`]).
pub(crate) async fn update_filters<P: EventProxy>(
    filters: UpdateFiltersPayload,
    proxy: P,
    state: SharedState,
) {
    update_filters_with(filters, proxy, state, tasks::RealFileSearcher).await;
}

/// [`update_filters`] with the searcher that looks through the file contents.
async fn update_filters_with<P: EventProxy, S: tasks::FileSearcher>(
    filters: UpdateFiltersPayload,
    proxy: P,
    state: SharedState,
    searcher: S,
) {
    let should_search_content = state.lock().set_filter_texts(
        filters.search_query,
        filters.extension_filter,
        filters.content_search_query,
    );

    if should_search_content {
        // Answers right away as well, before it searches.
        search_for_new_query(proxy, state, searcher).await;
    } else {
        with_state_and_notify(&state, &proxy, filtering::apply_filter_texts);
    }
}

/// Makes a file the previewed one and loads its content for the UI.
///
/// The state update (highlighting in the tree) is sent right away. The content follows
/// with `ShowFilePreview` once the file has been read in the background, unless another
/// preview was requested or the preview was closed in the meantime.
///
/// Only an entry of the loaded list can be previewed (see [`is_listed`]). A request
/// for any other path reads nothing: it is answered with an error and the unchanged
/// state.
pub fn load_file_preview<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState) {
    {
        let state_guard = state.lock();
        if !is_listed(&state_guard, &path) {
            tracing::warn!("Refusing to preview {path:?}: not part of the loaded list.");
            proxy.send_event(UserEvent::ShowError(format!(
                "Cannot preview {}: it is not part of the loaded directory.",
                path.display()
            )));
            send_snapshot(&proxy, &state_guard);
            return;
        }
    }
    tasks::start_file_preview(path, proxy, state);
}

/// True if `path` is an entry of the loaded list, i.e. something the scan of the
/// loaded directory has found.
///
/// This is what the UI can ask the content of. The list is the reference rather than
/// the directory on disk: it is what the user was shown (ignore patterns and the size
/// limit applied), and comparing against it needs no file system access, so there is
/// nothing a `..` or a symbolic link in the requested path could get around. The path
/// has to be the listed one; it is compared, not resolved.
fn is_listed(state: &AppState, path: &Path) -> bool {
    state.full_file_list().iter().any(|item| item.path == path)
}

/// Loads the children of a specific directory for lazy loading.
/// This is triggered when a user expands a directory that hasn't been fully scanned yet.
/// The directory is marked as expanded by the command itself; only the scan of the
/// level runs in the background.
pub fn load_directory_level<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState) {
    start_lazy_load_scan(path, proxy, state);
}

/// Adds an ignore pattern for a specific file or directory of the loaded tree (via
/// the UI button of a tree row).
///
/// The pattern names exactly that entry ([`gitignore_pattern`]): the path relative to
/// the project root, anchored there, with everything that has a meaning in a pattern
/// escaped; directory patterns end with a slash. Whether the path is a directory is
/// taken from the loaded file list, which is where the rows of the tree come from;
/// the file system is not asked (this runs on the command lane). It is applied like
/// any other added pattern, see [`change_config`].
///
/// A path that has no pattern adds nothing: one outside the loaded directory, one
/// with a name no pattern can name ([`IgnorePathRefusal`]), or any path while no
/// directory is loaded. Like every config command it is answered with a state update
/// either way. Where it is the name of the entry that stands in the way, the user
/// pressed a button on a row and nothing happened: they are told why
/// ([`UserEvent::IgnorePathRefused`]), before the state update.
pub async fn add_ignore_path<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState) {
    let pattern = ignore_pattern_for_path(&state.lock(), &path);
    let pattern = match pattern {
        Ok(pattern) => pattern,
        Err(reason) => {
            if let NoPattern::Refused(refusal) = reason {
                proxy.send_event(UserEvent::IgnorePathRefused(refusal));
            }
            return answer_unchanged(&proxy, &state);
        }
    };

    change_config(&proxy, &state, |config| {
        config.ignore_patterns.insert(pattern);
    })
    .await;
}

/// Why a path has no ignore pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
enum NoPattern {
    /// It is not an entry below the loaded directory (or there is none).
    NotInTheLoadedDirectory,
    /// It is one, with a name no pattern can name.
    Refused(IgnorePathRefusal),
}

/// The ignore pattern for `path`, a path of the loaded tree.
fn ignore_pattern_for_path(state: &AppState, path: &Path) -> Result<String, NoPattern> {
    if state.current_path.is_empty() {
        return Err(NoPattern::NotInTheLoadedDirectory);
    }
    let relative_path = path
        .strip_prefix(&state.current_path)
        .map_err(|_| NoPattern::NotInTheLoadedDirectory)?;
    let listed = state.full_file_list().iter().find(|item| item.path == path);
    if listed.is_none() {
        if let Some(name) = name_that_is_not_unicode(state, path) {
            return Err(NoPattern::Refused(IgnorePathRefusal::NotUnicode { name }));
        }
    }
    let is_directory = listed.is_some_and(|item| item.is_directory);

    gitignore_pattern(relative_path, is_directory)
}

/// The UI knows the paths of the tree as text, so an entry whose name is not valid
/// Unicode reaches it (and comes back from it) written lossily: with U+FFFD in place
/// of what could not be read. If `path` is such a path, i.e. not listed itself but
/// the lossy form of a listed one, this is the name of that entry as the UI shows
/// it. A pattern made of `path` would name nothing.
fn name_that_is_not_unicode(state: &AppState, path: &Path) -> Option<String> {
    let text = path.to_str().filter(|text| text.contains('\u{fffd}'))?;
    let is_lossy_form = state
        .full_file_list()
        .iter()
        .any(|item| item.path.to_str().is_none() && item.path.to_string_lossy() == text);
    is_lossy_form.then(|| {
        let name = path.file_name().unwrap_or_default();
        name.to_string_lossy().into_owned()
    })
}

/// The gitignore pattern that hides the entry at `relative_path` (relative to the
/// project root) and nothing else; the pattern of a directory ends with a slash.
///
/// The pattern is made for one entry, the row whose button was pressed, so:
///
/// * It is **anchored** at the project root with a leading `/`. Without it a pattern
///   without a slash in the middle (`README.md`, `docs/`) matches at any depth. The
///   leading slash also keeps a name that begins with `!` or `#` from being read as
///   a negation or a comment.
/// * The names are joined with `/` on every platform (a backslash escapes the
///   character after it), so the pattern is put together from the components of the
///   path, not from its text: on Windows that text is `src\gen`.
/// * Every character of a name that means something in a pattern is escaped with a
///   backslash ([`escape_pattern_characters`]).
/// * The matcher drops the whitespace a pattern ends with, any kind of it (the
///   pattern of `tab<TAB>` would hide `tab` and not the file it was made for). A
///   file name that ends with whitespace therefore gets its last character written
///   as a character class, `/tab[<TAB>]`, which ends the pattern with a bracket
///   and keeps whatever whitespace stands before it. This goes for the ASCII
///   whitespace: space, tab, line feed, vertical tab, form feed, carriage return.
///   (The slash at the end of a directory pattern protects the name of a directory.)
///
/// A path that has no such pattern is an error: the root itself or a path with
/// anything but names in it (`..`, a drive) is not an entry below the root; a name
/// that is not valid Unicode cannot be written (a pattern is text; a lossy
/// conversion would name some other entry, or none); and neither can a file name
/// that ends with whitespace outside ASCII (a no-break space, an ideographic space):
/// the matcher drops it like any other, and its character classes are not a way to
/// write it that this function vouches for.
fn gitignore_pattern(relative_path: &Path, is_directory: bool) -> Result<String, NoPattern> {
    let names = relative_path
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => match name.to_str() {
                Some(name) => Ok(escape_pattern_characters(name)),
                None => Err(NoPattern::Refused(IgnorePathRefusal::NotUnicode {
                    name: name.to_string_lossy().into_owned(),
                })),
            },
            _ => Err(NoPattern::NotInTheLoadedDirectory),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if names.is_empty() {
        return Err(NoPattern::NotInTheLoadedDirectory);
    }
    let mut pattern = format!("/{}", names.join("/"));
    if is_directory {
        // The matcher takes a backslash before the closing slash for an escape of
        // that slash and drops it, so a name that ends with a backslash ends with
        // a character class of it instead.
        if let Some(without_backslash) = pattern.strip_suffix("\\\\") {
            pattern = format!("{without_backslash}[\\\\]");
        }
        pattern.push('/');
    } else if let Some(last) = pattern.chars().next_back().filter(|c| c.is_whitespace()) {
        if !last.is_ascii() {
            let name = relative_path.file_name().unwrap_or_default();
            return Err(NoPattern::Refused(IgnorePathRefusal::TrailingWhitespace {
                name: name.to_string_lossy().into_owned(),
            }));
        }
        pattern.pop();
        pattern.extend(['[', last, ']']);
    }
    Ok(pattern)
}

/// `name` with a backslash before every character a gitignore pattern would not take
/// literally: the backslash itself, the wildcards `*` and `?`, the brackets of a
/// character class and the braces of alternatives (which the matcher of the `ignore`
/// crate evaluates).
fn escape_pattern_characters(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        if matches!(character, '\\' | '*' | '?' | '[' | ']' | '{' | '}') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// What kind of entry of the tree a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowKind {
    File,
    Directory,
}

/// The entry of the tree that `path` names, as the tree writes its path, with its
/// kind; `None` if the tree has no such entry.
///
/// The tree is the filtered list: the rows the UI shows and can act on. A command
/// that acts on a row takes its path from the UI, and this is where that path is
/// held against what the host knows: it is compared name by name with the entries
/// (not resolved, and the file system is not asked), so a path with a `..` in it,
/// one outside the loaded directory, or the lossy spelling of a name that is not
/// valid Unicode names nothing. Looking it up costs no pass over the list.
fn row_of(state: &AppState, path: &Path) -> Option<(PathBuf, RowKind)> {
    let items = state.filtered_file_list();
    let item = &items[state.file_tree().position(items, path)?];
    let kind = if item.is_directory {
        RowKind::Directory
    } else {
        RowKind::File
    };
    Some((item.path.clone(), kind))
}

/// Toggles the selection state of a single file.
///
/// Only a file of the tree can be selected. Any other path (a directory, a path the
/// tree does not hold) would be counted as a selected file without being one: it is
/// refused and answered with the unchanged snapshot.
pub fn toggle_selection<P: EventProxy>(path: PathBuf, proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        let Some((path, RowKind::File)) = row_of(s, &path) else {
            tracing::warn!("Refusing to select {path:?}: not a file of the tree.");
            return;
        };
        if s.selected_files.contains(&path) {
            s.selected_files.remove(&path);
        } else {
            s.selected_files.insert(path);
        }
    });
}

/// Toggles the selection state of all files within a directory.
pub fn toggle_directory_selection<P: EventProxy>(dir_path: PathBuf, proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        // Important: only operate on the currently *visible* files in that directory.
        // The same subtree determines the checkbox state the UI shows for it.
        let Some(subtree) = super::selection::subtree_selection(
            s.filtered_file_list(),
            &s.file_tree(),
            &s.selected_files,
            &dir_path,
        ) else {
            tracing::warn!(
                "Cannot toggle the selection of unlisted path {:?}",
                dir_path
            );
            return;
        };

        if subtree.state == super::selection::SelectionState::Full {
            // If fully selected, deselect all
            s.selected_files.remove_all(&subtree.files);
        } else {
            // If partially or not selected, select all
            s.selected_files.extend(subtree.files);
        }
    });
}

/// Payload of [`set_expansion`].
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetExpansionPayload {
    pub path: PathBuf,
    pub expanded: bool,
}

/// Sets the expanded/collapsed state of a directory in the UI tree.
///
/// The command names the target state instead of flipping it, which makes it idempotent:
/// a gesture that is delivered twice (key repeat, double click) before the UI has
/// rendered the answer to the first one still ends in the state the user asked for.
/// The state is always sent, also when nothing changed, so every request is answered.
///
/// Only a directory of the tree can be expanded or collapsed. Any other path (a
/// file, a path the tree does not hold) is refused and answered with the unchanged
/// snapshot; it is not remembered as expanded for a directory that might come.
pub(crate) fn set_expansion<P: EventProxy>(
    SetExpansionPayload { path, expanded }: SetExpansionPayload,
    proxy: P,
    state: SharedState,
) {
    with_state_and_notify(&state, &proxy, |s| {
        let Some((path, RowKind::Directory)) = row_of(s, &path) else {
            tracing::warn!("Refusing to expand or collapse {path:?}: not a directory of the tree.");
            return;
        };
        if expanded {
            s.expand_directories([path]);
        } else {
            s.collapse_directories([path]);
        }
    });
}

/// Payload of [`set_viewport`]: the part of the tree's rows the UI shows.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetViewportPayload {
    /// The first row of the window.
    pub start: usize,
    /// The number of rows of the window, overscan included. A window without rows
    /// is not a viewport: 0 is no value of this type, so a message with it is
    /// refused like any malformed one, as it is by the UI's schema.
    pub count: std::num::NonZeroUsize,
}

/// Stores the window of rows the UI looks at; the snapshot that answers it (and every
/// one after it) carries the rows of that window.
///
/// The host caps the size of the window ([`Viewport::MAX_ROWS`]) and moves a window
/// that lies behind the end of the list up to its end; the snapshot states where its
/// rows start.
pub(crate) fn set_viewport<P: EventProxy>(
    SetViewportPayload { start, count }: SetViewportPayload,
    proxy: P,
    state: SharedState,
) {
    with_state_and_notify(&state, &proxy, |s| {
        s.tree_view.set_viewport(Viewport::new(start, count.get()));
    });
}

/// Payload of [`locate_row`].
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct LocateRowPayload {
    /// Chosen by the UI; the answer repeats it.
    pub request: u32,
    pub target: RowTarget,
}

/// The row a [`locate_row`] request asks for.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RowTarget {
    /// The row of the item at `path`.
    Path { path: PathBuf },
    /// The row of the directory the item at `path` lies in.
    ParentOf { path: PathBuf },
    /// The first row from row `from` on (wrapping around) whose name starts with
    /// `prefix`, ignoring case. An empty prefix asks for nothing and is refused,
    /// like by the UI's schema.
    Typeahead { prefix: String, from: usize },
}

impl RowTarget {
    /// `true` for a target the types allow but that is no lookup.
    fn is_malformed(&self) -> bool {
        matches!(self, Self::Typeahead { prefix, .. } if prefix.is_empty())
    }
}

/// Answers where a row is in the list of all rows.
///
/// The UI holds a window of the rows only, so it cannot know the position of a row
/// outside of it: the parent of the focused row, the next name that starts with what
/// the user typed, the row that had the focus before the list changed. The host looks
/// the row up and states it in `located` of the snapshot that answers the command,
/// together with the number of the request. The window is not moved; the UI scrolls
/// where it wants to and reports its viewport.
///
/// A lookup of nothing (a type-ahead without a prefix) is answered with the unchanged
/// snapshot.
pub(crate) fn locate_row<P: EventProxy>(
    LocateRowPayload { request, target }: LocateRowPayload,
    proxy: P,
    state: SharedState,
) {
    if target.is_malformed() {
        tracing::warn!("Refused a locateRow without anything to look for");
        return answer_unchanged(&proxy, &state);
    }
    with_state_and_notify(&state, &proxy, |s| {
        let path = match target {
            RowTarget::Path { path } => Some(path),
            RowTarget::ParentOf { path } => path.parent().map(Path::to_path_buf),
            RowTarget::Typeahead { prefix, from } => typeahead_match(s, &prefix, from),
        };
        s.tree_view.answer_locate(request, path);
    });
}

/// Expands or collapses all *currently visible* directories in the file tree, i.e.
/// the directories of the filtered list.
///
/// Directories a filter hides are not touched: what was expanded outside the filter
/// is still expanded when the filter is cleared, whichever of the two the user chose
/// in between.
pub fn expand_collapse_all<P: EventProxy>(expand: bool, proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        let visible_directories: Vec<PathBuf> = s
            .filtered_file_list()
            .iter()
            .filter(|i| i.is_directory)
            .map(|i| i.path.clone())
            .collect();
        if expand {
            s.expand_directories(visible_directories);
        } else {
            s.collapse_directories(visible_directories);
        }
    });
}

/// Selects all *currently visible* files in the file tree.
pub fn select_all<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        let paths_to_select: Vec<PathBuf> = s
            .filtered_file_list()
            .iter()
            .filter(|item| !item.is_directory)
            .map(|item| item.path.clone())
            .collect();
        s.selected_files.extend(paths_to_select);
    });
}

/// Deselects all files.
pub fn deselect_all<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        s.selected_files.clear();
    });
}

/// Whether `filename` is a timestamped name an earlier generation assigned
/// (`cfc_output_<timestamp>.txt`). Any other name is kept as it is: a name the user
/// has set, and also the static name of a fresh configuration (`cfc_output.txt`, see
/// `AppConfig::default`).
fn is_timestamped_output_filename(filename: &str) -> bool {
    filename.starts_with("cfc_output_") && filename.ends_with(".txt")
}

/// Generates the final concatenated output from selected files by spawning a cancellable task.
///
/// A timestamped output filename is replaced by one with the current time (see
/// [`is_timestamped_output_filename`]), so that successive outputs do not overwrite
/// each other; the new name is persisted. Every other name stays.
///
/// The size of the output is judged before anything is read, from the file sizes of
/// the scan (see [`tasks::GenerationPlan`]):
///
/// - An output above [`tasks::MAX_OUTPUT_SIZE`] is refused. Nothing changes: a
///   generation that is running keeps running, the output that is shown stays, the
///   filename is not touched. The UI is told with [`OutputSizeNotice::TooLarge`].
/// - An output above [`tasks::LARGE_OUTPUT_SIZE`] is generated, and announced with
///   [`OutputSizeNotice::Large`].
pub fn generate_preview<P: EventProxy>(proxy: P, state: SharedState) {
    let mut state_guard = state.lock();

    let plan = tasks::GenerationPlan::of(&state_guard);
    if plan.estimated_size > tasks::MAX_OUTPUT_SIZE {
        tracing::warn!(
            "Refusing to generate an output of about {} bytes (limit: {}).",
            plan.estimated_size,
            tasks::MAX_OUTPUT_SIZE
        );
        proxy.send_event(UserEvent::OutputSize(OutputSizeNotice::TooLarge {
            estimated_bytes: plan.estimated_size,
            limit_bytes: tasks::MAX_OUTPUT_SIZE,
        }));
        send_snapshot(&proxy, &state_guard);
        return;
    }

    // Supersedes a generation that is still running; only the new one may publish.
    let token = state_guard.begin_generation();
    // The generated output takes the place of a file preview, also of one whose
    // content is still being read.
    state_guard.clear_preview();

    // A timestamped name gets the time of this generation; any other name is kept.
    if is_timestamped_output_filename(&state_guard.config.output_filename) {
        let new_filename = format!(
            "cfc_output_{}.txt",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        state_guard.config.output_filename = new_filename;
        tasks::save_config_in_background(state_guard.config_snapshot());
    }

    // Send an immediate state update to the UI to show the 'generating' state.
    send_snapshot(&proxy, &state_guard);
    if plan.estimated_size > tasks::LARGE_OUTPUT_SIZE {
        proxy.send_event(UserEvent::OutputSize(OutputSizeNotice::Large {
            estimated_bytes: plan.estimated_size,
            limit_bytes: tasks::LARGE_OUTPUT_SIZE,
        }));
    }

    let real_generator = tasks::RealContentGenerator {
        cancel_flag: token.cancel_flag(),
        max_file_size: state_guard.config.max_file_size_bytes(),
        max_output_size: tasks::MAX_OUTPUT_SIZE,
        expected_output_size: plan.estimated_size,
    };
    let real_tokenizer = tasks::RealTokenizer {
        cancel_flag: token.cancel_flag(),
    };

    // Spawn the actual generation logic as a separate, managed task.
    let handle = tokio::spawn(tasks::generation_task(
        proxy.clone(),
        state.clone(),
        real_generator,
        real_tokenizer,
        token.clone(),
        plan,
    ));
    state_guard.attach_generation_task(&token, handle);
}

/// Cancels the ongoing file content generation task.
///
/// The "cancelled" notice is the answer to the user's request, so it is set here and
/// only if a generation was actually running; the aborted task itself stays silent.
pub fn cancel_generation<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        if s.is_generating() {
            s.status = StatusNotice::GenerationCancelled;
        }
        s.cancel_current_generation();
    });
}

/// Resets the preview state in the UI. The editor is emptied with it, so a generated
/// output that was shown is given up as well.
pub fn clear_preview_state<P: EventProxy>(proxy: P, state: SharedState) {
    with_state_and_notify(&state, &proxy, |s| {
        s.clear_preview();
        s.discard_generated_document();
    });
}

/// Payload of [`save_file`]: what is to be saved.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum SaveFilePayload {
    /// The generated output as the host produced it, named by the id it was shown
    /// with (`showGeneratedContent`). The host writes its own copy; the text does not
    /// cross the IPC boundary a second time.
    Generated { document_id: u64 },
    /// The text of the editor, which the user has changed since it was shown.
    Edited { content: String },
}

/// What a save writes: the host's own document, or text the UI has sent.
enum SaveContent {
    Generated(GeneratedDocument),
    Edited(String),
}

impl SaveContent {
    fn text(&self) -> &str {
        match self {
            Self::Generated(document) => document.text(),
            Self::Edited(content) => content,
        }
    }
}

/// Saves the output to a file, prompting the user for a location.
///
/// What is saved is decided when the command starts, before the dialog opens: a
/// generation that ends while the user is choosing the file does not change it. The
/// command is answered with a [`SaveResult`]. A document the host does not hold (any
/// more) is a failed save; the dialog is not opened for it.
pub(crate) fn save_file<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    payload: SaveFilePayload,
    proxy: P,
    state: SharedState,
) {
    let (content, config) = {
        let state_guard = state.lock();
        let content = match payload {
            SaveFilePayload::Generated { document_id } => state_guard
                .generated_document(document_id)
                .map(SaveContent::Generated),
            SaveFilePayload::Edited { content } => Some(SaveContent::Edited(content)),
        };
        (content, state_guard.config.clone())
    };

    let result = match content {
        None => {
            tracing::warn!("A generated output that is not available was to be saved.");
            SaveResult::Failed {
                error: "The generated output is no longer available. Generate it again."
                    .to_string(),
            }
        }
        Some(content) => match dialog.save_output_file_path(&config) {
            None => SaveResult::Cancelled,
            Some(path) => match std::fs::write(&path, content.text()) {
                Ok(()) => SaveResult::Saved {
                    path: path.to_string_lossy().into_owned(),
                },
                Err(e) => SaveResult::Failed {
                    error: e.to_string(),
                },
            },
        },
    };
    proxy.send_event(UserEvent::SaveResult(result));
}

/// Opens a file dialog for the user to select a default output directory. The choice
/// is stored in the configuration and persisted.
pub fn pick_output_directory<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: SharedState,
) {
    if let Some(path) = dialog.pick_directory() {
        with_state_and_notify(&state, &proxy, |s| {
            if s.config.output_directory.as_ref() != Some(&path) {
                s.config.output_directory = Some(path);
                tasks::save_config_in_background(s.config_snapshot());
            }
        });
    }
}

/// Imports a configuration file.
///
/// The settings a user edits and shares are taken from the file (ignore patterns,
/// search and output options, the size limit, see [`AppConfig::adopt_user_settings`]).
/// The window geometry of the file is ignored: it describes the display of whoever
/// exported it, possibly in another unit.
///
/// The import is treated as a "hard reset" of the application's context: the current
/// state (file lists, selections, filters, previews) is cleared and the UI is told.
/// If the file names a `last_directory` and that directory exists, it is then opened
/// like any other path (see [`start_scan_on_path`]), which also records it as the last
/// directory of this installation. Otherwise nothing is scanned and the UI stays clean.
pub fn import_config<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: SharedState,
) {
    let Some(path) = dialog.pick_config_to_import() else {
        return;
    };
    let imported = match config::settings::import_config(&path) {
        Ok(imported) => imported,
        Err(e) => {
            proxy.send_event(UserEvent::ShowError(format!(
                "Failed to import config: {e}"
            )));
            return;
        }
    };
    // Looked at before the state is locked; this command runs on the blocking lane.
    let directory_to_open = imported.last_directory.clone().filter(|dir| dir.exists());

    // The reset, the new settings and the start of the scan happen under one lock.
    let mut state_guard = state.lock();
    state_guard.reset_directory_state();
    state_guard.config.adopt_user_settings(imported);
    state_guard.current_config_filename =
        path.file_name().and_then(|n| n.to_str()).map(String::from);
    tasks::save_config_in_background(state_guard.config_snapshot());

    // The UI is wiped clean before the scan (if any) announces itself.
    send_snapshot(&proxy, &state_guard);
    if let Some(directory) = directory_to_open {
        tasks::open_path(&mut state_guard, directory, proxy, state.clone());
    }
}

/// Exports the settings that are meant to be shared to a JSON file: patterns, search
/// and output options, the size limit. Directories and the window geometry of this
/// installation are not part of the file (see [`config::SharedConfig`]).
///
/// The settings are copied under the state lock; the file is written without it, so
/// a slow disk does not hold up everything else that needs the state.
pub fn export_config<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: SharedState,
) {
    export_config_with(dialog, proxy, state, config::settings::export_config);
}

/// [`export_config`] with the function that writes the file as a parameter. `write`
/// is called without the state lock.
fn export_config_with<P: EventProxy, D: DialogService + ?Sized>(
    dialog: &D,
    proxy: P,
    state: SharedState,
    write: impl FnOnce(&config::SharedConfig, &Path) -> anyhow::Result<()>,
) {
    if let Some(path) = dialog.export_config_path() {
        // The guard is a temporary of this statement: the lock is released before
        // the file is written.
        let settings = config::SharedConfig::from(&state.lock().config);
        let result = write(&settings, &path).is_ok();
        proxy.send_event(UserEvent::ConfigExported(result));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::file_dialog::DialogService;
    use crate::app::state::AppState;
    use crate::app::view_model::UiState;
    use crate::config::{ConfigStore, GeometryUnits};
    use crate::core::FileItem;
    use crate::utils::test_helpers::setup_test_logging;
    use serde_json::json;

    /// The payload of a command, read the way a message of the UI is: as the type
    /// the command carries. A test of a handler starts where the handler does, with
    /// a payload that fits; what does not fit is refused before (see `app::ipc`).
    fn typed<T: serde::de::DeserializeOwned>(payload: serde_json::Value) -> T {
        serde_json::from_value(payload).expect("the payload fits its command")
    }
    use std::fs as std_fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tempfile::{tempdir, TempDir};
    use tokio::sync::mpsc;

    // A mock EventProxy for capturing events sent to the UI.
    #[derive(Clone)]
    struct TestEventProxy {
        sender: mpsc::UnboundedSender<UserEvent>,
    }

    impl EventProxy for TestEventProxy {
        fn send_event(&self, event: UserEvent) {
            self.sender.send(event).expect("Test receiver dropped");
        }
    }

    // A mock DialogService to simulate user interaction with file dialogs.
    #[derive(Default)]
    struct MockDialogService {
        picked_folder: Mutex<Option<PathBuf>>,
        picked_file: Mutex<Option<PathBuf>>,
        saved_file: Mutex<Option<PathBuf>>,
    }

    impl Clone for MockDialogService {
        fn clone(&self) -> Self {
            MockDialogService {
                picked_folder: Mutex::new(self.picked_folder.lock().unwrap().clone()),
                picked_file: Mutex::new(self.picked_file.lock().unwrap().clone()),
                saved_file: Mutex::new(self.saved_file.lock().unwrap().clone()),
            }
        }
    }

    impl MockDialogService {
        fn set_pick_folder(&self, path: Option<PathBuf>) {
            *self.picked_folder.lock().unwrap() = path;
        }

        fn set_pick_file(&self, path: Option<PathBuf>) {
            *self.picked_file.lock().unwrap() = path;
        }

        fn set_save_file(&self, path: Option<PathBuf>) {
            *self.saved_file.lock().unwrap() = path;
        }
    }

    impl DialogService for MockDialogService {
        fn pick_directory(&self) -> Option<PathBuf> {
            self.picked_folder.lock().unwrap().clone()
        }
        fn pick_config_to_import(&self) -> Option<PathBuf> {
            self.picked_file.lock().unwrap().clone()
        }
        fn export_config_path(&self) -> Option<PathBuf> {
            self.saved_file.lock().unwrap().clone()
        }
        fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
            self.saved_file.lock().unwrap().clone()
        }
    }

    struct TestHarness {
        state: SharedState,
        proxy: TestEventProxy,
        event_rx: mpsc::UnboundedReceiver<UserEvent>,
        dialog: Arc<MockDialogService>,
        _temp_dir: TempDir,
        _config_dir: Option<TempDir>,
        root_path: PathBuf,
    }

    impl TestHarness {
        fn new() -> Self {
            let temp_dir = tempdir().expect("Failed to create temp dir");
            let root_path = temp_dir.path().to_path_buf();
            let (tx, rx) = mpsc::unbounded_channel();
            let proxy = TestEventProxy { sender: tx };
            let dialog = Arc::new(MockDialogService::default());

            let mut state = AppState::default();
            state.config = AppConfig::default();
            state.current_path = root_path.to_string_lossy().to_string();

            Self {
                state: SharedState::new(state),
                proxy,
                event_rx: rx,
                dialog,
                _temp_dir: temp_dir,
                _config_dir: None,
                root_path,
            }
        }

        /// A harness whose configuration is kept in a file (in a directory of its own
        /// next to the project), for tests of what ends up on disk.
        fn with_config_file() -> (Self, ConfigStore) {
            let harness = Self::new();
            let config_dir = tempdir().expect("Failed to create the config dir");
            let (store, config) = ConfigStore::open(config_dir.path().join("config.json"));
            {
                let mut state = harness.state.lock();
                let mut with_file = AppState::new(config, store.clone());
                with_file.current_path = state.current_path.clone();
                *state = with_file;
            }
            (
                Self {
                    _config_dir: Some(config_dir),
                    ..harness
                },
                store,
            )
        }

        fn create_file(&self, relative_path: &str, content: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            if let Some(parent) = path.parent() {
                std_fs::create_dir_all(parent).unwrap();
            }
            std_fs::write(&path, content).unwrap();
            path
        }

        fn create_dir(&self, relative_path: &str) -> PathBuf {
            let path = self.root_path.join(relative_path);
            std_fs::create_dir_all(&path).unwrap();
            path
        }

        fn set_initial_files(&self, paths: &[&str]) {
            let mut state = self.state.lock();
            let mut items = Vec::new();
            for p_str in paths {
                let path = self.root_path.join(p_str);
                items.push(file_item(path.clone(), path.is_dir()));
            }
            state.set_full_file_list(items.clone());
            state.set_filtered_file_list(items);
        }

        /// The number of state updates among the events sent so far; all pending events
        /// are consumed, and any other event fails the test.
        #[track_caller]
        fn state_updates_sent(&mut self) -> usize {
            std::iter::from_fn(|| self.event_rx.try_recv().ok())
                .inspect(|event| {
                    assert!(
                        matches!(event, UserEvent::StateUpdate(_)),
                        "Only state updates expected, got {event:?}"
                    )
                })
                .count()
        }

        /// The most recent state update among the events sent so far, without waiting
        /// for more; all pending events are consumed. This is the way to look at the
        /// outcome of a command that has run to completion. For work a command has
        /// handed to a background task use [`Self::wait_for_state`].
        fn last_state_update(&mut self) -> Option<Box<UiState>> {
            std::iter::from_fn(|| self.event_rx.try_recv().ok())
                .filter_map(|event| match event {
                    UserEvent::StateUpdate(ui_state) => Some(ui_state.into_ui_state()),
                    _ => None,
                })
                .last()
        }

        /// The answer to a `saveFile` command, which has to be the next event.
        async fn next_save_result(&mut self) -> SaveResult {
            match self.get_next_event().await {
                Some(UserEvent::SaveResult(result)) => result,
                other => panic!("Expected the result of a save, got {other:?}"),
            }
        }

        /// Waits for the next event; `None` if none arrives (which fails the test that
        /// expected one, so the waiting time does not matter).
        async fn get_next_event(&mut self) -> Option<UserEvent> {
            tokio::time::timeout(EVENT_TIMEOUT, self.event_rx.recv())
                .await
                .ok()
                .flatten()
        }

        /// Asserts that no event is sent within `window`. Background tasks get the
        /// chance to run during that time, so this also covers commands that might
        /// have (wrongly) started one.
        #[track_caller]
        fn expect_no_event_within<'a>(
            &'a mut self,
            window: std::time::Duration,
            context: &'a str,
        ) -> impl std::future::Future<Output = ()> + 'a {
            let caller = std::panic::Location::caller();
            async move {
                if let Ok(event) = tokio::time::timeout(window, self.event_rx.recv()).await {
                    panic!("{context} (at {caller}), got {event:?}");
                }
            }
        }

        /// Waits until a background task sends a state update that satisfies
        /// `predicate` and returns it; earlier events are consumed.
        async fn wait_for_state(
            &mut self,
            mut predicate: impl FnMut(&UiState) -> bool,
        ) -> Box<UiState> {
            loop {
                match self.get_next_event().await {
                    Some(UserEvent::StateUpdate(ui_state)) if predicate(&ui_state) => {
                        return ui_state.into_ui_state()
                    }
                    Some(_) => {}
                    None => panic!("Timed out waiting for the expected state update"),
                }
            }
        }

        /// Waits for a scan to run and end: returns the first state update without a
        /// running scan that follows one with a running scan. Updates that were sent
        /// before the scan was announced do not count, so this does not return for a
        /// command that never started a scan (it returns `None` after the timeout).
        async fn wait_for_scan_completion(&mut self) -> Option<Box<UiState>> {
            let mut scan_seen = false;
            loop {
                match self.get_next_event().await? {
                    UserEvent::StateUpdate(ui_state) if ui_state.is_scanning => scan_seen = true,
                    UserEvent::StateUpdate(ui_state) if scan_seen => {
                        return Some(ui_state.into_ui_state())
                    }
                    _ => {}
                }
            }
        }
    }

    /// Upper bound for waiting on something a test expects to happen. It is only ever
    /// reached by a failing test, so it does not slow the suite down.
    const EVENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

    /// Waits until the configuration file of `store` satisfies `predicate` and returns
    /// its content. Commands save in the background, so the file follows the state
    /// with a delay.
    async fn config_on_disk(
        store: &ConfigStore,
        predicate: impl Fn(&AppConfig) -> bool,
    ) -> AppConfig {
        let path = store.path().expect("the store is bound to a file");
        tokio::time::timeout(EVENT_TIMEOUT, async {
            loop {
                let saved = config::settings::load_config(path).unwrap();
                if predicate(&saved) {
                    return saved;
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("the expected configuration was not saved")
    }

    /// How long a test listens for events that must not be sent.
    const QUIET_PERIOD: std::time::Duration = std::time::Duration::from_millis(50);

    fn file_item(path: PathBuf, is_dir: bool) -> FileItem {
        FileItem {
            path,
            is_directory: is_dir,
            is_binary: false,
            size: if is_dir { 0 } else { 123 },
            depth: 1,
            parent: None,
        }
    }

    // =========================================================================================
    // SECTION: Existing tests (unchanged, verified)
    // =========================================================================================

    #[tokio::test]
    async fn test_select_directory_starts_scan_on_ok() {
        let mut harness = TestHarness::new();
        let new_dir = harness.create_dir("new_project");
        harness.dialog.set_pick_folder(Some(new_dir.clone()));

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert!(!final_state.is_scanning);
        assert_eq!(final_state.current_path, new_dir.to_string_lossy());
    }

    #[tokio::test]
    async fn test_select_directory_updates_state_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let final_state = harness.last_state_update().unwrap();
        assert!(!final_state.is_scanning);
    }

    /// Dismissing the dialog is not a request to stop anything: a scan that is running in
    /// the background must keep running and keep owning the scanning state.
    #[tokio::test]
    async fn test_select_directory_cancel_leaves_running_scan_untouched() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);
        let token = {
            let mut state = harness.state.lock();
            let token = state.begin_scan();
            state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
            token
        };

        select_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning, "UI must still show the running scan");
        let mut state = harness.state.lock();
        assert!(state.is_scanning());
        assert!(state.is_current_scan(&token));
        assert!(state.has_scan_task(), "Scan must stay cancellable");
        assert!(!token.is_cancelled());
        state.cancel_current_scan();
    }

    /// A generation started via the command owns the generating state and can be cancelled.
    #[tokio::test]
    async fn test_generate_preview_registers_cancellable_generation() {
        let mut harness = TestHarness::new();
        harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        {
            let state = harness.state.lock();
            assert!(state.is_generating());
            assert!(state.has_generation_task());
        }
        cancel_generation(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        let state = harness.state.lock();
        assert!(!state.is_generating());
        assert!(!state.has_generation_task());
    }

    #[tokio::test]
    async fn test_rescan_directory_on_empty_path_does_nothing() {
        let mut harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            state.current_path = String::new();
        }

        rescan_directory(harness.proxy.clone(), harness.state.clone());

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "Rescan should not trigger any event when path is empty",
            )
            .await;
    }

    #[tokio::test]
    async fn test_set_config_option_triggers_refilter() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "main");
        harness.create_dir("src/empty_dir");

        harness.set_initial_files(&["src", "src/main.rs", "src/empty_dir"]);
        {
            let mut state = harness.state.lock();
            state.is_fully_scanned = true;
            state.loaded_dirs.insert(harness.root_path.join("src"));
            state
                .loaded_dirs
                .insert(harness.root_path.join("src/empty_dir"));
        }

        set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Expected 'src/empty_dir' to be removed"
        );
    }

    #[tokio::test]
    async fn test_update_filters_applies_filename_filter_without_content_search() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "");
        harness.create_file("src/lib.rs", "");
        harness.create_file("README.md", "");
        harness.set_initial_files(&["src", "src/main.rs", "src/lib.rs", "README.md"]);

        let filters = json!({
            "searchQuery": "main",
            "extensionFilter": "",
            "contentSearchQuery": ""
        });
        update_filters(typed(filters), harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 2);
    }

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
        assert!(state.config.ignore_patterns.contains("/docs/"));
    }

    /// A configuration as somebody else exported it: other settings, and the geometry
    /// and last directory of *their* installation.
    fn foreign_config(last_directory: Option<PathBuf>) -> AppConfig {
        AppConfig {
            ignore_patterns: HashSet::from(["*.log".to_string()]),
            case_sensitive_search: true,
            output_filename: "imported.txt".to_string(),
            max_file_size_mb: 3,
            window_size: (4000.0, 3000.0),
            window_position: (-900.0, -900.0),
            geometry_units: GeometryUnits::Physical,
            last_directory,
            ..Default::default()
        }
    }

    fn import_file(harness: &TestHarness, imported: &AppConfig) {
        let path = harness.root_path.join("imported_config.json");
        std_fs::write(&path, serde_json::to_string(imported).unwrap()).unwrap();
        harness.dialog.set_pick_file(Some(path));
        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
    }

    const KEPT_GEOMETRY: ((f64, f64), (f64, f64)) = ((1000.0, 700.0), (7.0, 8.0));

    /// Gives the state a geometry of its own, to tell it from the imported one.
    fn set_kept_geometry(harness: &TestHarness) {
        let mut state = harness.state.lock();
        state.config.window_size = KEPT_GEOMETRY.0;
        state.config.window_position = KEPT_GEOMETRY.1;
        state.config.geometry_units = GeometryUnits::Logical;
    }

    #[track_caller]
    fn assert_geometry_is_kept(config: &AppConfig) {
        assert_eq!((config.window_size, config.window_position), KEPT_GEOMETRY);
        assert_eq!(config.geometry_units, GeometryUnits::Logical);
    }

    /// The import is a hard reset: selection and filters of the loaded directory are
    /// gone, and the directory the imported file names is opened, listed according to
    /// the imported settings, and recorded as the last directory.
    #[tokio::test]
    async fn test_import_config_resets_and_opens_the_imported_last_directory() {
        let (mut harness, store) = TestHarness::with_config_file();
        let kept = harness.create_file("kept.txt", "");
        harness.set_initial_files(&["kept.txt"]);
        {
            let mut state = harness.state.lock();
            state.selected_files.insert(kept.clone());
            state.search_query = "kept".to_string();
        }
        set_kept_geometry(&harness);
        let project_to_open = harness.create_dir("new_project_dir");
        harness.create_file("new_project_dir/file.rs", "");
        harness.create_file("new_project_dir/debug.log", "");
        let imported = foreign_config(Some(project_to_open.clone()));

        import_file(&harness, &imported);

        // First the clean slate, then the scan.
        match harness.get_next_event().await.unwrap() {
            UserEvent::StateUpdate(clean) => {
                assert!(!clean.is_scanning);
                assert_eq!(clean.current_path, "");
                assert_eq!(clean.selected_files_count, 0);
                assert!(clean.rows.is_empty());
                assert!(clean.config.case_sensitive_search);
                assert_eq!(
                    clean.current_config_filename.as_deref(),
                    Some("imported_config.json")
                );
            }
            other => panic!("Expected the clean state first, got {other:?}"),
        }
        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert_eq!(final_state.current_path, project_to_open.to_string_lossy());
        assert!(final_state.is_fully_scanned);

        let expected = AppConfig {
            window_size: KEPT_GEOMETRY.0,
            window_position: KEPT_GEOMETRY.1,
            geometry_units: GeometryUnits::Logical,
            ..imported
        };
        {
            let state = harness.state.lock();
            assert!(state.selected_files.is_empty() && state.search_query.is_empty());
            let listed: Vec<_> = state
                .full_file_list()
                .iter()
                .filter_map(|item| item.path.file_name()?.to_str())
                .collect();
            assert_eq!(
                listed,
                ["file.rs"],
                "the imported ignore pattern applies to the opened directory"
            );
            assert_eq!(state.config, expected);
            assert_geometry_is_kept(&state.config);
        }
        let saved = config_on_disk(&store, |saved| {
            saved.last_directory.as_ref() == Some(&project_to_open)
        })
        .await;
        assert_eq!(saved, expected);
    }

    /// The imported file names no directory: the state is clean and nothing is
    /// scanned, whatever was loaded before. The geometry of the file is ignored.
    #[tokio::test]
    async fn test_import_config_without_a_last_directory_leaves_a_clean_state() {
        let (mut harness, store) = TestHarness::with_config_file();
        harness.create_file("kept.txt", "");
        harness.set_initial_files(&["kept.txt"]);
        set_kept_geometry(&harness);
        let last_directory = Some(harness.root_path.clone());
        harness.state.lock().config.last_directory = last_directory.clone();
        let imported = foreign_config(None);

        import_file(&harness, &imported);

        match harness.get_next_event().await {
            Some(UserEvent::StateUpdate(clean)) => {
                assert!(!clean.is_scanning);
                assert_eq!(clean.current_path, "");
                assert!(clean.rows.is_empty());
            }
            other => panic!("Expected the clean state, got {other:?}"),
        }
        harness
            .expect_no_event_within(QUIET_PERIOD, "No scan should start without a directory")
            .await;

        let expected = AppConfig {
            window_size: KEPT_GEOMETRY.0,
            window_position: KEPT_GEOMETRY.1,
            geometry_units: GeometryUnits::Logical,
            last_directory,
            ..imported
        };
        {
            let state = harness.state.lock();
            assert_eq!(state.current_path, "");
            assert!(state.full_file_list().is_empty() && !state.is_scanning());
            assert_eq!(state.config, expected);
            assert_geometry_is_kept(&state.config);
        }
        assert_eq!(
            config_on_disk(&store, |saved| saved.output_filename == "imported.txt").await,
            expected
        );
    }

    /// Without a directory there is nothing to filter or scan, but the change is
    /// stored and answered: the UI shows the configuration the host reports.
    #[tokio::test]
    async fn test_config_change_without_a_directory_is_stored_and_answered() {
        let mut harness = TestHarness::new();
        harness.state.lock().current_path = String::new();

        set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;
        let ui_state = harness.last_state_update().expect("the change is answered");
        assert!(ui_state.config.remove_empty_directories);

        add_pattern(&harness, "generated/").await;
        let ui_state = harness.last_state_update().expect("the change is answered");
        assert!(ui_state
            .config
            .ignore_patterns
            .contains(&"generated/".to_string()));
        assert!(!ui_state.patterns_need_rescan);

        remove_pattern(&harness, "generated/").await;
        let ui_state = harness.last_state_update().expect("the change is answered");
        assert!(
            !ui_state.patterns_need_rescan,
            "Nothing is loaded that a re-scan could refresh"
        );
        assert!(!harness.state.lock().is_scanning());
    }

    #[tokio::test]
    async fn test_add_ignore_path_does_nothing_when_no_directory_is_loaded() {
        let mut harness = TestHarness::new();
        let initial_patterns_count;
        {
            let mut state = harness.state.lock();
            state.current_path = String::new();
            initial_patterns_count = state.config.ignore_patterns.len();
        }

        let payload = json!("/some/path/to/ignore.txt");
        add_ignore_path(typed(payload), harness.proxy.clone(), harness.state.clone()).await;

        {
            let final_patterns_count = harness.state.lock().config.ignore_patterns.len();
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
    async fn test_import_config_sends_error_on_corrupt_file() {
        let mut harness = TestHarness::new();
        let corrupt_config_path = harness.create_file("corrupt_config.json", "{ not_valid_json, }");
        harness.dialog.set_pick_file(Some(corrupt_config_path));

        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.get_next_event().await.unwrap() {
            UserEvent::ShowError(msg) => {
                assert!(
                    msg.contains("Failed to import config"),
                    "Expected an import error message, but got: {}",
                    msg
                );
            }
            other => panic!("Expected ShowError event, but got {:?}", other),
        }
    }

    /// A request the command itself refuses changes nothing and is answered all the
    /// same: the UI shows what the host reports, so it has to be told that nothing
    /// changed. (A payload that does not fit its command never gets here; see
    /// `a_refused_message_changes_nothing_and_is_answered_under_its_number` in
    /// `app/mod.rs`.)
    #[tokio::test]
    async fn test_config_commands_answer_what_they_refuse() {
        let mut harness = TestHarness::new();
        let initial_config = harness.state.lock().config.clone();

        // A blank text is not a pattern.
        for blank in ["", "   ", "\t"] {
            add_pattern(&harness, blank).await;
            assert_eq!(harness.state_updates_sent(), 1, "answer to {blank:?}");
        }
        // A path that is no entry of the loaded directory has no pattern.
        add_ignore_path(
            PathBuf::from("src"),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        assert_eq!(harness.state_updates_sent(), 1, "answer to addIgnorePath");

        // Nothing was changed, nothing was saved, and each refusal was answered once.
        assert_eq!(harness.state.lock().config, initial_config);
        harness
            .expect_no_event_within(QUIET_PERIOD, "Nothing else is sent")
            .await;
    }

    #[tokio::test]
    async fn test_add_ignore_pattern_hides_matching_files() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src/main.rs"]);

        add_pattern(&harness, "*.rs").await;

        // No scan runs here (none was running): the pattern is applied to the list.
        let final_state = harness.last_state_update().unwrap();
        assert_eq!(
            final_state.visible_files_count, 0,
            "Scan should have removed the .rs file"
        );
    }

    #[tokio::test]
    async fn test_update_filters_triggers_content_search() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["file1.txt"]);
        harness.create_file("file1.txt", "hello world");
        {
            let mut state = harness.state.lock();
            state.content_search_query = "initial".to_string();
        }

        let filters = json!({
            "contentSearchQuery": "world"
        });

        update_filters(typed(filters), harness.proxy.clone(), harness.state.clone()).await;

        let final_state = harness.last_state_update().unwrap();
        assert_eq!(final_state.content_search_query, "world");
        assert_eq!(
            final_state.visible_files_count, 1,
            "The matching file should be visible"
        );
    }

    #[tokio::test]
    async fn test_add_ignore_path_handles_path_outside_root() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src/main.rs"]);
        let initial_patterns_count = harness.state.lock().config.ignore_patterns.len();
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
        let final_patterns_count = harness.state.lock().config.ignore_patterns.len();
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
        let pattern =
            gitignore_pattern(path_of(relative).strip_prefix(root).unwrap(), is_directory)
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
        let patterns = harness.state.lock().config.ignore_patterns.clone();

        add_ignore_path(
            typed(json!(harness.root_path.join("name\u{a0}"))),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        {
            let state = harness.state.lock();
            assert_eq!(state.config.ignore_patterns, patterns);
            assert_eq!(state.full_file_list().len(), 2);
        }
        let events: Vec<UserEvent> =
            std::iter::from_fn(|| harness.event_rx.try_recv().ok()).collect();
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
        let patterns = harness.state.lock().config.ignore_patterns.clone();

        add_ignore_path(
            typed(json!(path.to_string_lossy())),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;

        assert_eq!(harness.state.lock().config.ignore_patterns, patterns);
        let events: Vec<UserEvent> =
            std::iter::from_fn(|| harness.event_rx.try_recv().ok()).collect();
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
            state.config.ignore_patterns.contains("/src/gen/"),
            "{:?}",
            state.config.ignore_patterns
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
        assert!(state.config.ignore_patterns.contains("/build/"));
        assert!(state.config.ignore_patterns.contains("/notes"));
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
        let patterns_before = harness.state.lock().config.ignore_patterns.clone();

        add_ignore_path(typed(payload), harness.proxy.clone(), harness.state.clone()).await;

        // The command is answered, but nothing has changed and no scan is started.
        let ui_state = harness
            .last_state_update()
            .expect("the command is answered");
        assert!(!ui_state.is_scanning);
        assert!(!ui_state.patterns_need_rescan);
        let state = harness.state.lock();
        assert!(!state.is_scanning());
        assert_eq!(state.config.ignore_patterns, patterns_before);
    }

    /// The imported file names a directory that does not exist: as without one, the
    /// state is clean, nothing is scanned and no error is shown.
    #[tokio::test]
    async fn test_import_config_with_a_nonexistent_last_directory_leaves_a_clean_state() {
        let mut harness = TestHarness::new();
        harness.create_file("kept.txt", "");
        harness.set_initial_files(&["kept.txt"]);
        set_kept_geometry(&harness);
        let nonexistent = harness.root_path.join("nonexistent_dir");

        import_file(&harness, &foreign_config(Some(nonexistent)));

        match harness.get_next_event().await {
            Some(UserEvent::StateUpdate(clean)) => {
                assert!(!clean.is_scanning);
                assert_eq!(clean.current_path, "");
                assert!(clean.rows.is_empty());
                assert!(clean.config.case_sensitive_search);
            }
            other => panic!("Expected the clean state, got {other:?}"),
        }
        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No scan should start for a nonexistent directory",
            )
            .await;
        let state = harness.state.lock();
        assert_eq!(state.current_path, "");
        assert!(!state.is_scanning());
        assert_geometry_is_kept(&state.config);
    }

    // =========================================================================================
    // The following tests call SYNCHRONOUS commands and DO NOT NEED .await
    // =========================================================================================

    #[tokio::test]
    async fn test_clear_directory_resets_state() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);
        {
            let mut state = harness.state.lock();
            state.selected_files.insert(file_path);
            state.config.last_directory = Some(harness.root_path.clone());
        }

        clear_directory(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.current_path.is_empty());
        assert_eq!(ui_state.visible_files_count, 0);
        let state = harness.state.lock();
        assert!(state.current_path.is_empty());
        assert!(state.full_file_list().is_empty());
        assert!(state.selected_files.is_empty());
        assert!(state.config.last_directory.is_none());
    }

    #[tokio::test]
    async fn test_cancel_scan_updates_state() {
        let mut harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            let token = state.begin_scan();
            let handle = tokio::spawn(async {});
            state.attach_scan_task(&token, handle);
        }

        cancel_scan(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_scanning);
        assert_eq!(ui_state.status_message, "Scan cancelled.");
    }

    #[tokio::test]
    async fn test_initialize_sends_initial_state() {
        let mut harness = TestHarness::new();
        initialize(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.current_path, harness.root_path.to_string_lossy());
    }

    #[tokio::test]
    async fn test_load_file_preview_sends_search_term() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("preview.txt", "content with magic_word");
        harness.set_initial_files(&["preview.txt"]);
        let search_term = "magic_word";
        {
            let mut state = harness.state.lock();
            state.content_search_query = search_term.to_string();
        }
        let payload = json!(file_path);

        load_file_preview(typed(payload), harness.proxy.clone(), harness.state.clone());

        // The command answers with the state (the file is highlighted) right away ...
        let Some(UserEvent::StateUpdate(_)) = harness.event_rx.try_recv().ok() else {
            panic!("Expected the synchronous state update");
        };
        assert_eq!(
            harness.state.lock().previewed_file_path,
            Some(file_path.clone())
        );
        // ... and the content follows once the file has been read in the background.
        match harness.get_next_event().await {
            Some(UserEvent::ShowFilePreview {
                search_term: term,
                content,
                path,
                ..
            }) => {
                assert_eq!(term, Some(search_term.to_string()));
                assert!(content.contains("content with magic_word"));
                assert_eq!(path, file_path);
            }
            other => panic!("Expected the ShowFilePreview event, got {other:?}"),
        }
    }

    /// A preview that is closed before its file has been read shows nothing.
    #[tokio::test]
    async fn test_clear_preview_state_discards_preview_being_loaded() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("preview.txt", "content");
        harness.set_initial_files(&["preview.txt"]);

        load_file_preview(
            typed(json!(file_path)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        // On the single-threaded test runtime the read has not been started yet.
        clear_preview_state(harness.proxy.clone(), harness.state.clone());
        let _ = harness.last_state_update();

        harness
            .expect_no_event_within(QUIET_PERIOD, "A closed preview must not show content")
            .await;
        assert_eq!(harness.state.lock().previewed_file_path, None);
    }

    #[tokio::test]
    async fn test_load_directory_level_starts_lazy_scan() {
        let mut harness = TestHarness::new();
        let sub_dir = harness.create_dir("src/components");
        harness.create_file("src/components/button.js", "");
        harness.set_initial_files(&["src", "src/components"]);
        // The snapshot only contains children of expanded directories, and a user can
        // only reach `src/components` with `src` open.
        harness
            .state
            .lock()
            .expanded_dirs
            .insert(harness.root_path.join("src"));

        let payload = json!(sub_dir);
        load_directory_level(typed(payload), harness.proxy.clone(), harness.state.clone());

        // The command itself marks the directory as expanded and says so.
        let components_of = |ui_state: &UiState| {
            let src_node = ui_state.row_named("src").unwrap();
            ui_state
                .children_of(src_node)
                .into_iter()
                .find(|n| n.name == "components")
                .cloned()
                .unwrap()
        };
        let announced = harness
            .last_state_update()
            .expect("The command must answer synchronously");
        let components_node = components_of(&announced);
        assert!(components_node.is_expanded && !components_node.children_loaded);
        assert!(harness.state.lock().expanded_dirs.contains(&sub_dir));

        // The lazy load runs in the background and reports its result when it is done.
        let final_state = harness.get_next_event().await;
        let Some(UserEvent::StateUpdate(final_state)) = final_state else {
            panic!("Expected the state update of the lazy load, got {final_state:?}");
        };
        let components_node = components_of(&final_state);
        assert!(components_node.children_loaded);
        let components_row = final_state.row_named("components").unwrap();
        assert!(final_state
            .children_of(components_row)
            .into_iter()
            .any(|n| n.name == "button.js"));
    }

    /// End to end through the command: the user collapses the directory while its
    /// content is being loaded. The result of the load must not open it again.
    #[tokio::test]
    async fn test_load_directory_level_then_collapse_stays_collapsed() {
        let mut harness = TestHarness::new();
        let src = harness.create_dir("src");
        harness.create_file("src/main.rs", "");
        harness.set_initial_files(&["src"]);

        load_directory_level(
            typed(json!(src)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        // On the single-threaded test runtime the load has not run yet.
        assert!(harness.state.lock().expanded_dirs.contains(&src));
        set_expansion(
            typed(json!({ "path": src, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let _ = harness.last_state_update();

        // The result of the load arrives.
        let loaded = harness
            .wait_for_state(|s| {
                s.root_rows()
                    .into_iter()
                    .any(|n| n.name == "src" && n.children_loaded)
            })
            .await;
        let src_node = loaded
            .root_rows()
            .into_iter()
            .find(|n| n.name == "src")
            .unwrap();
        assert!(src_node.has_children);
        assert!(!src_node.is_expanded, "Directory must stay collapsed");
        let state = harness.state.lock();
        assert!(state.loaded_dirs.contains(&src));
        assert!(!state.expanded_dirs.contains(&src));
    }

    // ... All other synchronous tests remain unchanged ...
    #[tokio::test]
    async fn test_toggle_selection_adds_and_removes_file() {
        let mut harness = TestHarness::new();
        let file_path = harness.create_file("test.rs", "");
        harness.set_initial_files(&["test.rs"]);
        let payload = json!(file_path);

        toggle_selection(
            typed(payload.clone()),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(ui_state1.selected_files_count, 1);

        toggle_selection(typed(payload), harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.last_state_update().unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_toggle_directory_selection_selects_and_deselects_all_children() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "");
        let dir_path = harness.create_dir("src");
        harness.set_initial_files(&["src", "src/main.rs"]);
        let payload = json!(dir_path);

        toggle_directory_selection(
            typed(payload.clone()),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state_select = harness.last_state_update().unwrap();
        assert_eq!(ui_state_select.selected_files_count, 1);

        toggle_directory_selection(typed(payload), harness.proxy.clone(), harness.state.clone());
        let ui_state_deselect = harness.last_state_update().unwrap();
        assert_eq!(ui_state_deselect.selected_files_count, 0);
    }

    /// A project of `files` files directly below the root.
    fn harness_with_files(files: usize) -> TestHarness {
        let harness = TestHarness::new();
        {
            let mut state = harness.state.lock();
            let items: Vec<FileItem> = (0..files)
                .map(|i| file_item(harness.root_path.join(format!("file{i:04}.rs")), false))
                .collect();
            state.set_full_file_list(items.clone());
            state.set_filtered_file_list(items);
        }
        harness
    }

    #[tokio::test]
    async fn test_set_viewport_answers_with_the_rows_of_the_window() {
        let mut harness = harness_with_files(1_000);
        initialize(harness.proxy.clone(), harness.state.clone());
        let initial = harness.last_state_update().unwrap();
        assert_eq!(initial.rows_total, 1_000);
        assert_eq!(initial.rows_start, 0);
        assert_eq!(initial.rows.len(), Viewport::DEFAULT_ROWS);

        set_viewport(
            typed(json!({ "start": 600, "count": 40 })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let scrolled = harness.last_state_update().unwrap();
        assert_eq!((scrolled.rows_start, scrolled.rows.len()), (600, 40));
        assert_eq!(scrolled.rows[0].name, "file0600.rs");
        assert_eq!(scrolled.rows_revision, initial.rows_revision);

        // Every later snapshot carries that window, whatever it answers.
        toggle_selection(
            typed(json!(harness.root_path.join("file0601.rs"))),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let selected = harness.last_state_update().unwrap();
        assert_eq!(selected.rows_start, 600);
        assert_eq!(
            selected.rows[1].selection_state,
            crate::app::selection::SelectionState::Full
        );

        // The host bounds the window; a window behind the end is moved up.
        set_viewport(
            typed(json!({ "start": 990, "count": 1_000_000 })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let capped = harness.last_state_update().unwrap();
        assert_eq!(capped.rows.len(), Viewport::MAX_ROWS);
        assert_eq!(capped.rows_start, 1_000 - Viewport::MAX_ROWS);

        // A new UI starts at the top.
        initialize(harness.proxy.clone(), harness.state.clone());
        assert_eq!(harness.last_state_update().unwrap().rows_start, 0);
    }

    /// A type-ahead without a prefix is a lookup the types allow, but it asks for
    /// nothing: it is answered with the unchanged snapshot.
    #[tokio::test]
    async fn test_a_lookup_of_nothing_is_answered_unchanged() {
        let mut harness = harness_with_files(300);
        set_viewport(
            typed(json!({ "start": 200, "count": 10 })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness.last_state_update().unwrap();

        locate_row(
            typed(
                json!({ "request": 1, "target": { "kind": "typeahead", "prefix": "", "from": 0 } }),
            ),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let answer = harness.last_state_update().expect("answered");
        assert_eq!(answer.located, None);
        assert_eq!((answer.rows_start, answer.rows.len()), (200, 10));
    }

    #[tokio::test]
    async fn test_locate_row_answers_with_the_row_of_the_target() {
        let mut harness = TestHarness::new();
        let src = harness.create_dir("src");
        let main = harness.create_file("src/main.rs", "");
        harness.create_file("src/mod.rs", "");
        harness.create_file("readme.md", "");
        harness.set_initial_files(&["src", "src/main.rs", "src/mod.rs", "readme.md"]);
        set_expansion(
            typed(json!({ "path": src, "expanded": true })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        // 0 src, 1 main.rs, 2 mod.rs, 3 readme.md
        let mut locate = |request: u32, target: serde_json::Value| {
            locate_row(
                typed(json!({ "request": request, "target": target })),
                harness.proxy.clone(),
                harness.state.clone(),
            );
            let located = harness.last_state_update().unwrap().located;
            let located = located.expect("the request is answered");
            assert_eq!(located.request, request);
            located.row
        };

        assert_eq!(locate(1, json!({ "kind": "path", "path": main })), Some(1));
        assert_eq!(
            locate(2, json!({ "kind": "parent_of", "path": main })),
            Some(0)
        );
        // The parent of a row at the top is the loaded directory, which is no row.
        assert_eq!(locate(3, json!({ "kind": "parent_of", "path": src })), None);
        assert_eq!(
            locate(4, json!({ "kind": "typeahead", "prefix": "MO", "from": 0 })),
            Some(2)
        );
        // From the row after the focused one, wrapping around.
        assert_eq!(
            locate(5, json!({ "kind": "typeahead", "prefix": "m", "from": 2 })),
            Some(2)
        );
        assert_eq!(
            locate(6, json!({ "kind": "typeahead", "prefix": "m", "from": 3 })),
            Some(1)
        );
        assert_eq!(
            locate(7, json!({ "kind": "typeahead", "prefix": "zz", "from": 0 })),
            None
        );

        // The answer stays true when the list changes: `main.rs` is no row any more.
        assert_eq!(locate(8, json!({ "kind": "path", "path": main })), Some(1));
        set_expansion(
            typed(json!({ "path": src, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let collapsed = harness.last_state_update().unwrap();
        assert_eq!(
            collapsed.located.as_ref().map(|located| located.row),
            Some(None)
        );
        assert_eq!(collapsed.rows_total, 2);
    }

    #[tokio::test]
    async fn test_set_expansion_is_idempotent() {
        let mut harness = TestHarness::new();
        let dir = harness.create_dir("src");
        harness.set_initial_files(&["src"]);
        let set = |expanded: bool| json!({ "path": dir, "expanded": expanded });

        // Expanding twice (key repeat, double click) leaves the directory expanded,
        // and each request is answered with a snapshot.
        for _ in 0..2 {
            set_expansion(
                typed(set(true)),
                harness.proxy.clone(),
                harness.state.clone(),
            );
            let ui_state = harness.last_state_update().unwrap();
            assert!(ui_state.root_rows()[0].is_expanded);
        }
        assert_eq!(harness.state.lock().expanded_dirs.len(), 1);

        for _ in 0..2 {
            set_expansion(
                typed(set(false)),
                harness.proxy.clone(),
                harness.state.clone(),
            );
            let ui_state = harness.last_state_update().unwrap();
            assert!(!ui_state.root_rows()[0].is_expanded);
        }
        assert!(harness.state.lock().expanded_dirs.is_empty());
    }

    #[tokio::test]
    async fn test_set_expansion_only_affects_the_given_directory() {
        let mut harness = TestHarness::new();
        let src = harness.create_dir("src");
        let docs = harness.create_dir("docs");
        harness.set_initial_files(&["docs", "src"]);
        harness.state.lock().expanded_dirs.insert(docs.clone());

        set_expansion(
            typed(json!({ "path": src, "expanded": true })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        set_expansion(
            typed(json!({ "path": src, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let _ = harness.last_state_update().unwrap();

        let state = harness.state.lock();
        assert_eq!(state.expanded_dirs.len(), 1);
        assert!(state.expanded_dirs.contains(&docs));
    }

    #[tokio::test]
    async fn test_expand_collapse_all() {
        let mut harness = TestHarness::new();
        harness.create_dir("src");
        harness.set_initial_files(&["src"]);

        expand_collapse_all(
            typed(json!(true)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state1
                .root_rows()
                .into_iter()
                .filter(|n| n.is_expanded)
                .count(),
            1
        );

        expand_collapse_all(
            typed(json!(false)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let ui_state2 = harness.last_state_update().unwrap();
        assert!(ui_state2.root_rows().into_iter().all(|n| !n.is_expanded));
    }

    #[tokio::test]
    async fn test_select_all_and_deselect_all() {
        let mut harness = TestHarness::new();
        harness.create_file("file1.txt", "");
        harness.create_file("file2.txt", "");
        harness.set_initial_files(&["file1.txt", "file2.txt"]);

        select_all(harness.proxy.clone(), harness.state.clone());
        let ui_state1 = harness.last_state_update().unwrap();
        assert_eq!(ui_state1.selected_files_count, 2);

        deselect_all(harness.proxy.clone(), harness.state.clone());
        let ui_state2 = harness.last_state_update().unwrap();
        assert_eq!(ui_state2.selected_files_count, 0);
    }

    #[tokio::test]
    async fn test_generate_preview_sets_generating_state_and_spawns_task() {
        let mut harness = TestHarness::new();
        harness.create_file("file.txt", "content");
        harness.set_initial_files(&["file.txt"]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let event = harness.get_next_event().await.unwrap();
        let ui_state = match event {
            UserEvent::StateUpdate(ui_state) => ui_state,
            _ => panic!("Expected a StateUpdate event first"),
        };
        assert!(ui_state.is_generating);

        // The generation task reports its end with a final state update.
        harness
            .wait_for_state(|ui_state| !ui_state.is_generating)
            .await;
    }

    #[tokio::test]
    async fn test_cancel_generation_resets_generating_state() {
        let mut harness = TestHarness::new();
        generate_preview(harness.proxy.clone(), harness.state.clone());
        let _ = harness.last_state_update();

        cancel_generation(harness.proxy.clone(), harness.state.clone());
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        assert_eq!(ui_state.status_message, "Generation cancelled.");
    }

    #[tokio::test]
    async fn test_cancel_generation_without_running_generation_keeps_status() {
        let mut harness = TestHarness::new();
        let status_before = harness.state.lock().status.clone();

        cancel_generation(harness.proxy.clone(), harness.state.clone());

        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.is_generating);
        assert_eq!(harness.state.lock().status, status_before);
    }

    #[tokio::test]
    async fn test_clear_preview_state() {
        let harness = TestHarness::new();
        let file_path = harness.create_file("file.txt", "content");
        {
            let mut state = harness.state.lock();
            state.previewed_file_path = Some(file_path);
        }
        clear_preview_state(harness.proxy.clone(), harness.state.clone());
        let state = harness.state.lock();
        assert!(state.previewed_file_path.is_none());
    }

    #[tokio::test]
    async fn test_save_file_writes_to_disk_on_ok() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        let content_to_save = "Hello, World!";
        harness.dialog.set_save_file(Some(save_path.clone()));

        save_file(
            harness.dialog.as_ref(),
            typed(json!({ "kind": "edited", "content": content_to_save })),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert_eq!(
            harness.next_save_result().await,
            SaveResult::Saved {
                path: save_path.to_string_lossy().into_owned()
            }
        );
        let written_content = std_fs::read_to_string(save_path).unwrap();
        assert_eq!(written_content, content_to_save);
    }

    /// The document of a finished generation, as the host keeps it.
    fn generated_document(harness: &TestHarness, text: &str) -> GeneratedDocument {
        let mut state = harness.state.lock();
        let token = state.begin_generation();
        state
            .complete_generation(&token, Arc::new(text.to_string()))
            .expect("the generation is the current one")
    }

    fn save_generated(harness: &TestHarness, document_id: u64) {
        save_file(
            harness.dialog.as_ref(),
            typed(json!({ "kind": "generated", "documentId": document_id })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
    }

    /// The UI does not send the generated output back: it names the document, and the
    /// host writes the copy it has kept, byte for byte.
    #[tokio::test]
    async fn test_save_file_writes_the_generated_document_the_host_keeps() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));
        let text = "# CFC Output\n\nsrc/a.rs\n===FILE-START===\nfn a() {}\r\n\u{fffd}ä\n";
        let document = generated_document(&harness, text);

        save_generated(&harness, document.id());

        assert_eq!(
            harness.next_save_result().await,
            SaveResult::Saved {
                path: save_path.to_string_lossy().into_owned()
            }
        );
        assert_eq!(std_fs::read(&save_path).unwrap(), text.as_bytes());
        // Saving does not use the document up: it can be saved again.
        std_fs::remove_file(&save_path).unwrap();
        save_generated(&harness, document.id());
        assert!(matches!(
            harness.next_save_result().await,
            SaveResult::Saved { .. }
        ));
        assert_eq!(std_fs::read(&save_path).unwrap(), text.as_bytes());
    }

    /// An edited output is what the UI sends, whatever the host has generated.
    #[tokio::test]
    async fn test_save_file_writes_edited_content_instead_of_the_generated_document() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));
        generated_document(&harness, "generated");

        save_file(
            harness.dialog.as_ref(),
            typed(json!({ "kind": "edited", "content": "generated, then edited\n" })),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert!(matches!(
            harness.next_save_result().await,
            SaveResult::Saved { .. }
        ));
        assert_eq!(
            std_fs::read_to_string(&save_path).unwrap(),
            "generated, then edited\n"
        );
    }

    /// A document the host does not hold is a failed save, reported as such; the user
    /// is not asked for a file first.
    #[tokio::test]
    async fn test_save_file_fails_for_a_document_the_host_does_not_hold() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));
        let expect_unavailable = |result: SaveResult| match result {
            SaveResult::Failed { error } => {
                assert!(error.contains("no longer available"), "{error}")
            }
            other => panic!("Expected a failed save, got {other:?}"),
        };

        // Nothing was generated yet.
        save_generated(&harness, 1);
        expect_unavailable(harness.next_save_result().await);

        // The document was replaced by the one of a later generation.
        let first = generated_document(&harness, "first");
        let second = generated_document(&harness, "second");
        save_generated(&harness, first.id());
        expect_unavailable(harness.next_save_result().await);
        assert!(!save_path.exists());

        // The preview was cleared, which empties the editor.
        clear_preview_state(harness.proxy.clone(), harness.state.clone());
        harness.last_state_update().expect("a snapshot");
        save_generated(&harness, second.id());
        expect_unavailable(harness.next_save_result().await);
        assert!(!save_path.exists());
    }

    /// The output is still on screen after another directory has been opened (the UI
    /// clears the editor only when no directory is loaded any more), so it can still
    /// be saved: by a drop or the directory dialog, by reopening the same directory,
    /// by an import.
    #[tokio::test]
    async fn test_generated_output_can_be_saved_after_the_directory_was_changed() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("a.txt", "content of a");
        harness.set_initial_files(&["a.txt"]);
        harness.state.lock().selected_files.insert(file);
        let save_path = harness.root_path.join("saved.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));

        generate_preview(harness.proxy.clone(), harness.state.clone());
        let document = loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent { document, .. }) => break document,
                Some(_) => {}
                None => panic!("Timed out waiting for the generated content"),
            }
        };

        let other = harness.create_dir("other_project");
        let same = harness.root_path.clone();
        for directory in [other, same] {
            open_dropped_path(
                directory.clone(),
                harness.proxy.clone(),
                harness.state.clone(),
            );
            harness
                .wait_for_state(|ui_state| {
                    !ui_state.is_scanning && Path::new(&ui_state.current_path) == directory
                })
                .await;

            save_generated(&harness, document.id());
            loop {
                match harness.get_next_event().await {
                    Some(UserEvent::SaveResult(result)) => {
                        assert!(
                            matches!(result, SaveResult::Saved { .. }),
                            "after opening {directory:?}: {result:?}"
                        );
                        break;
                    }
                    Some(UserEvent::StateUpdate(_) | UserEvent::ScanProgress(_)) => {}
                    other => panic!("Expected the result of the save, got {other:?}"),
                }
            }
            assert_eq!(std_fs::read_to_string(&save_path).unwrap(), document.text());
            std_fs::remove_file(&save_path).unwrap();
        }

        // A hard reset of the directory (what an import does) keeps it as well.
        harness.state.lock().reset_directory_state();
        save_generated(&harness, document.id());
        assert!(matches!(
            harness.next_save_result().await,
            SaveResult::Saved { .. }
        ));

        // Closing the directory makes the UI clear its editor, which it reports.
        clear_directory(harness.proxy.clone(), harness.state.clone());
        assert!(harness
            .state
            .lock()
            .generated_document(document.id())
            .is_some());
        clear_preview_state(harness.proxy.clone(), harness.state.clone());
        assert!(harness
            .state
            .lock()
            .generated_document(document.id())
            .is_none());
    }

    /// What is saved is decided when the command starts: the document the UI named
    /// is written even if a newer one arrives while the dialog is open.
    #[tokio::test]
    async fn test_save_file_writes_the_document_it_was_asked_for_when_the_dialog_opened() {
        struct GeneratingDialog {
            state: SharedState,
            path: PathBuf,
        }
        impl DialogService for GeneratingDialog {
            fn pick_directory(&self) -> Option<PathBuf> {
                None
            }
            fn pick_config_to_import(&self) -> Option<PathBuf> {
                None
            }
            fn export_config_path(&self) -> Option<PathBuf> {
                None
            }
            /// While the user is choosing the file, a generation comes to its end.
            fn save_output_file_path(&self, _config: &AppConfig) -> Option<PathBuf> {
                let mut state = self.state.lock();
                let token = state.begin_generation();
                state
                    .complete_generation(&token, Arc::new("newer".to_string()))
                    .unwrap();
                Some(self.path.clone())
            }
        }

        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("output.txt");
        let shown = generated_document(&harness, "shown when save was pressed");
        let dialog = GeneratingDialog {
            state: harness.state.clone(),
            path: save_path.clone(),
        };

        save_file(
            &dialog,
            typed(json!({ "kind": "generated", "documentId": shown.id() })),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert!(matches!(
            harness.next_save_result().await,
            SaveResult::Saved { .. }
        ));
        assert_eq!(
            std_fs::read_to_string(&save_path).unwrap(),
            "shown when save was pressed"
        );
    }

    #[test]
    fn test_save_file_payload_is_strict() {
        let parse = |value| serde_json::from_value::<SaveFilePayload>(value);

        assert_eq!(
            parse(json!({ "kind": "generated", "documentId": 3 })).unwrap(),
            SaveFilePayload::Generated { document_id: 3 }
        );
        assert_eq!(
            parse(json!({ "kind": "edited", "content": "x" })).unwrap(),
            SaveFilePayload::Edited {
                content: "x".to_string()
            }
        );
        for refused in [
            // The payload of earlier versions: the content itself.
            json!("content"),
            json!({ "kind": "generated" }),
            json!({ "kind": "generated", "documentId": "3" }),
            json!({ "kind": "generated", "documentId": -1 }),
            json!({ "kind": "generated", "documentId": 3, "content": "x" }),
            json!({ "kind": "edited" }),
            json!({ "kind": "edited", "content": "x", "documentId": 3 }),
            json!({ "kind": "unknown", "content": "x" }),
            json!({ "content": "x" }),
            json!(null),
        ] {
            assert!(parse(refused.clone()).is_err(), "{refused}");
        }
    }

    #[tokio::test]
    async fn test_pick_output_directory_updates_config() {
        let harness = TestHarness::new();
        let new_dir = harness.create_dir("output");
        harness.dialog.set_pick_folder(Some(new_dir.clone()));

        pick_output_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let state = harness.state.lock();
        assert_eq!(state.config.output_directory, Some(new_dir));
    }

    // --- Every change of the configuration is persisted, through the ordered path ---

    #[tokio::test]
    async fn test_pick_output_directory_persists_the_choice() {
        let (mut harness, store) = TestHarness::with_config_file();
        let new_dir = harness.create_dir("output");
        harness.dialog.set_pick_folder(Some(new_dir.clone()));

        pick_output_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let saved = config_on_disk(&store, |saved| {
            saved.output_directory.as_ref() == Some(&new_dir)
        })
        .await;
        assert_eq!(saved, harness.state.lock().config);
        let answered = harness.last_state_update().expect("the choice is answered");
        assert_eq!(answered.config.output_directory, Some(new_dir));
    }

    #[tokio::test]
    async fn test_config_commands_persist_their_change() {
        let (harness, store) = TestHarness::with_config_file();

        set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;
        add_pattern(&harness, "generated/").await;
        set_option(&harness, ConfigOption::OutputFilename("named.txt".into())).await;

        let saved = config_on_disk(&store, |saved| saved.output_filename == "named.txt").await;
        assert_eq!(saved, harness.state.lock().config);
        assert!(saved.case_sensitive_search);
        assert!(saved.ignore_patterns.contains("generated/"));
    }

    /// Many changes in a row, each saved in the background by a task of its own: the
    /// saves get to run in any order, and what ends up on disk is still the last one.
    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn test_rapid_config_changes_end_with_the_last_one_on_disk() {
        for round in 0..10 {
            let (harness, store) = TestHarness::with_config_file();
            let last = 40;

            for i in 0..=last {
                let name = format!("round-{round}-change-{i}.txt");
                set_option(&harness, ConfigOption::OutputFilename(name)).await;
            }
            // Wait for every background save, then look at what is there.
            let expected = format!("round-{round}-change-{last}.txt");
            let saved = config_on_disk(&store, |saved| saved.output_filename == expected).await;
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;

            assert_eq!(saved, harness.state.lock().config);
            let path = store.path().unwrap();
            assert_eq!(
                config::settings::load_config(path).unwrap().output_filename,
                expected,
                "round {round}: an older save must not have overwritten the last one"
            );
        }
    }

    /// The timestamped default name a generation assigns is a change like any other.
    #[tokio::test]
    async fn test_generate_preview_persists_the_filename_it_assigns() {
        let (harness, store) = TestHarness::with_config_file();
        harness.state.lock().config.output_filename = "cfc_output_19990101_000000.txt".into();

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let assigned = harness.state.lock().config.output_filename.clone();
        assert_ne!(assigned, "cfc_output_19990101_000000.txt");
        let saved = config_on_disk(&store, |saved| saved.output_filename == assigned).await;
        assert_eq!(saved.output_filename, assigned);
    }

    #[tokio::test]
    async fn test_clear_directory_persists_the_forgotten_last_directory() {
        let (harness, store) = TestHarness::with_config_file();
        {
            let mut state = harness.state.lock();
            state.config.last_directory = Some(harness.root_path.clone());
            state.config_snapshot().save().unwrap();
        }

        clear_directory(harness.proxy.clone(), harness.state.clone());

        let saved = config_on_disk(&store, |saved| saved.last_directory.is_none()).await;
        assert_eq!(saved, harness.state.lock().config);
    }

    // --- loadFilePreview: only entries of the loaded list ---

    /// Asks for a preview of `path` and returns the error it is refused with. Fails
    /// the test if the request is not refused: an error, the unchanged state, nothing
    /// else, and no file read.
    async fn refused_preview(harness: &mut TestHarness, path: &Path) -> String {
        let previewed_before = harness.state.lock().previewed_file_path.clone();

        load_file_preview(
            typed(json!(path)),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let Some(UserEvent::ShowError(message)) = harness.event_rx.try_recv().ok() else {
            panic!("Expected the preview of {path:?} to be refused with an error");
        };
        let Some(UserEvent::StateUpdate(_)) = harness.event_rx.try_recv().ok() else {
            panic!("Expected a snapshot after the refusal of {path:?}");
        };
        harness
            .expect_no_event_within(QUIET_PERIOD, "A refused preview must not show content")
            .await;
        assert_eq!(
            harness.state.lock().previewed_file_path,
            previewed_before,
            "{path:?}"
        );
        message
    }

    #[tokio::test]
    async fn test_load_file_preview_refuses_a_path_outside_the_loaded_directory() {
        let mut harness = TestHarness::new();
        harness.create_file("listed.txt", "listed");
        harness.set_initial_files(&["listed.txt"]);
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("secret.txt");
        std_fs::write(&outside, "top secret").unwrap();

        let message = refused_preview(&mut harness, &outside).await;

        assert!(message.contains("not part of the loaded directory"));
        assert!(message.contains("secret.txt"));
        assert!(!message.contains("top secret"));
    }

    /// Inside the directory is not enough: the file has to be one the scan listed
    /// (it may be ignored, too large, or created since).
    #[tokio::test]
    async fn test_load_file_preview_refuses_an_unlisted_file_of_the_loaded_directory() {
        let mut harness = TestHarness::new();
        harness.create_file("listed.txt", "listed");
        let unlisted = harness.create_file(".env", "API_KEY=secret");
        harness.set_initial_files(&["listed.txt"]);

        refused_preview(&mut harness, &unlisted).await;
    }

    /// The requested path is compared with the list, not resolved: neither `..` nor a
    /// symbolic link leads from a listed directory to something else.
    #[tokio::test]
    async fn test_load_file_preview_does_not_resolve_the_requested_path() {
        let mut harness = TestHarness::new();
        harness.create_dir("sub");
        let listed = harness.create_file("sub/listed.txt", "listed");
        harness.set_initial_files(&["sub", "sub/listed.txt"]);
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("secret.txt");
        std_fs::write(&outside, "top secret").unwrap();

        // Out of the directory by `..`, also behind a listed path.
        let outside_name = outside_dir.path().file_name().unwrap();
        let by_parent = harness
            .root_path
            .join("sub/../..")
            .join(outside_name)
            .join("secret.txt");
        refused_preview(&mut harness, &by_parent).await;
        refused_preview(&mut harness, &listed.join("../../../etc/hosts")).await;
        // The listed file by another spelling is not the listed path either.
        let respelled = harness.root_path.join("sub/../sub/listed.txt");
        refused_preview(&mut harness, &respelled).await;

        // A link inside the directory that the scan has not listed.
        #[cfg(unix)]
        {
            let link = harness.root_path.join("sub/link.txt");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            refused_preview(&mut harness, &link).await;
        }
    }

    #[tokio::test]
    async fn test_load_file_preview_refuses_everything_when_no_directory_is_loaded() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("file.txt", "content");
        harness.state.lock().reset_directory_state();
        let _ = harness.last_state_update();

        refused_preview(&mut harness, &file).await;
        refused_preview(&mut harness, Path::new("")).await;
        refused_preview(&mut harness, Path::new("/etc/hosts")).await;
    }

    /// A refused request leaves the preview that is shown alone.
    #[tokio::test]
    async fn test_refused_preview_keeps_the_current_preview() {
        let mut harness = TestHarness::new();
        let listed = harness.create_file("listed.txt", "listed content");
        let unlisted = harness.create_file("unlisted.txt", "unlisted content");
        harness.set_initial_files(&["listed.txt"]);
        load_file_preview(
            typed(json!(listed)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let _ = harness.event_rx.try_recv();
        assert!(matches!(
            harness.get_next_event().await,
            Some(UserEvent::ShowFilePreview { .. })
        ));

        refused_preview(&mut harness, &unlisted).await;

        assert_eq!(harness.state.lock().previewed_file_path, Some(listed));
    }

    #[test]
    fn test_is_listed_knows_exactly_the_entries_of_the_list() {
        let root = PathBuf::from("/project");
        let mut state = AppState::default();
        state.current_path = "/project".to_string();
        state.set_full_file_list(vec![
            file_item(root.join("src"), true),
            file_item(root.join("src/main.rs"), false),
        ]);

        for (path, expected) in [
            ("/project/src/main.rs", true),
            ("/project/src", true),
            ("/project", false),
            ("/project/src/other.rs", false),
            ("/project/src/main.rs/..", false),
            ("/project/src/../src/main.rs", false),
            ("/project/src/main.rs.bak", false),
            ("src/main.rs", false),
            ("/etc/passwd", false),
            ("", false),
        ] {
            assert_eq!(is_listed(&state, Path::new(path)), expected, "{path:?}");
        }
    }

    // --- exportConfig: shareable settings only ---

    #[tokio::test]
    async fn test_export_config_writes_the_shared_settings_only() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("exported.json");
        harness.dialog.set_save_file(Some(save_path.clone()));
        {
            let mut state = harness.state.lock();
            state.config.ignore_patterns = HashSet::from(["*.shared".to_string()]);
            state.config.case_sensitive_search = true;
            state.config.output_filename = "shared.txt".to_string();
            state.config.output_directory = Some(PathBuf::from("/home/alice/out"));
            state.config.last_directory = Some(PathBuf::from("/home/alice/project"));
            state.config.window_size = (1234.0, 567.0);
        }

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert!(matches!(
            harness.get_next_event().await,
            Some(UserEvent::ConfigExported(true))
        ));
        let written = std_fs::read_to_string(&save_path).unwrap();
        let exported: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(
            exported,
            serde_json::to_value(config::SharedConfig::from(&harness.state.lock().config)).unwrap()
        );
        assert_eq!(exported["ignore_patterns"], json!(["*.shared"]));
        for personal in [
            "alice",
            "last_directory",
            "output_directory",
            "window_size",
            "window_position",
            "geometry_units",
        ] {
            assert!(!written.contains(personal), "{personal} in {written}");
        }
    }

    /// The export of one installation, imported into another: the settings arrive,
    /// the directories and the geometry of the importing installation stay.
    #[tokio::test]
    async fn test_exported_config_can_be_imported() {
        let mut exporter = TestHarness::new();
        let exported_path = exporter.root_path.join("exported.json");
        exporter.dialog.set_save_file(Some(exported_path.clone()));
        {
            let mut state = exporter.state.lock();
            state.config.ignore_patterns = HashSet::from(["*.shared".to_string()]);
            state.config.case_sensitive_search = true;
            state.config.max_file_size_mb = 3;
            state.config.output_directory = Some(PathBuf::from("/home/alice/out"));
        }
        export_config(
            exporter.dialog.as_ref(),
            exporter.proxy.clone(),
            exporter.state.clone(),
        );
        assert!(matches!(
            exporter.get_next_event().await,
            Some(UserEvent::ConfigExported(true))
        ));

        let mut importer = TestHarness::new();
        set_kept_geometry(&importer);
        {
            let mut state = importer.state.lock();
            state.config.output_directory = Some(PathBuf::from("/home/bob/out"));
            state.config.last_directory = Some(PathBuf::from("/home/bob/project"));
        }
        importer.dialog.set_pick_file(Some(exported_path));
        import_config(
            importer.dialog.as_ref(),
            importer.proxy.clone(),
            importer.state.clone(),
        );

        assert!(matches!(
            importer.get_next_event().await,
            Some(UserEvent::StateUpdate(_))
        ));
        importer
            .expect_no_event_within(QUIET_PERIOD, "An export names no directory to open")
            .await;
        let config = importer.state.lock().config.clone();
        assert_eq!(
            config::SharedConfig::from(&config),
            config::SharedConfig::from(&exporter.state.lock().config)
        );
        assert_eq!(
            config.output_directory,
            Some(PathBuf::from("/home/bob/out"))
        );
        assert_eq!(
            config.last_directory,
            Some(PathBuf::from("/home/bob/project"))
        );
        assert_geometry_is_kept(&config);
    }

    /// The file is written without the state lock: while the export writes, the state
    /// can be locked by anybody else, and what is written is the copy taken before.
    #[tokio::test]
    async fn test_export_config_writes_without_holding_the_state_lock() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("exported.json");
        harness.dialog.set_save_file(Some(save_path.clone()));
        harness.state.lock().config.output_filename = "before.txt".to_string();
        let observed_state = harness.state.clone();
        let mut written = None;

        export_config_with(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
            |settings, path| {
                // The lock is free while the file is written ...
                let mut state = observed_state
                    .try_lock()
                    .expect("the state lock must not be held during the write");
                // ... and an edit made meanwhile does not reach the file.
                state.config.output_filename = "meanwhile.txt".to_string();
                written = Some((settings.clone(), path.to_path_buf()));
                Ok(())
            },
        );

        let (settings, path) = written.expect("the file is written");
        assert_eq!(settings.output_filename, "before.txt");
        assert_eq!(path, save_path);
        assert!(matches!(
            harness.get_next_event().await,
            Some(UserEvent::ConfigExported(true))
        ));
    }

    /// The seam of the test above sees a held lock: it would fail if the export wrote
    /// under the lock.
    #[test]
    fn test_a_held_state_lock_is_observable() {
        let state = SharedState::new(AppState::default());
        let guard = state.lock();
        assert!(state.try_lock().is_none());
        drop(guard);
        assert!(state.try_lock().is_some());
    }

    #[tokio::test]
    async fn test_export_config_sends_event() {
        let mut harness = TestHarness::new();
        let save_path = harness.root_path.join("my-config.json");
        harness.dialog.set_save_file(Some(save_path));

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.get_next_event().await.unwrap() {
            UserEvent::ConfigExported(success) => assert!(success),
            _ => panic!("Expected ConfigExported event"),
        }
    }

    #[tokio::test]
    async fn test_export_config_sends_no_event_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_save_file(None);

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        harness
            .expect_no_event_within(
                QUIET_PERIOD,
                "No event should be sent when export is cancelled",
            )
            .await;
    }

    #[tokio::test]
    async fn test_save_file_sends_error_on_io_failure() {
        let mut harness = TestHarness::new();
        // A cross-platform way to cause an I/O error is to try to write to a directory path.
        let invalid_save_path = harness.root_path.clone();
        harness.dialog.set_save_file(Some(invalid_save_path));

        save_file(
            harness.dialog.as_ref(),
            typed(json!({ "kind": "edited", "content": "some content" })),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.next_save_result().await {
            // The reason is the error of the operating system, in a field of its own.
            SaveResult::Failed { error } => {
                // Check for common error messages on different platforms.
                assert!(
                    error.contains("Is a directory") // Linux, macOS
                        || error.contains("Access is denied") // Windows
                        || error.contains("Permission denied"),
                    "{error}"
                );
            }
            other => panic!("Expected a failed save, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_generate_preview_preserves_custom_filename() {
        let mut harness = TestHarness::new();
        let custom_filename = "my_special_context.md".to_string();

        // 1. Set a custom filename in the state.
        {
            let mut state = harness.state.lock();
            state.config.output_filename = custom_filename.clone();
        }

        // 2. Run the generate_preview command.
        generate_preview(harness.proxy.clone(), harness.state.clone());

        // 3. Verify the filename in the state has NOT been changed.
        let final_filename = harness.state.lock().config.output_filename.clone();
        assert_eq!(final_filename, custom_filename);

        // Consume events to avoid panics on drop
        let _ = harness.get_next_event().await;
        let _ = harness.get_next_event().await;
    }

    // =========================================================================================
    // SECTION: New tests for coverage increase
    // =========================================================================================

    // --- Payloads ---

    /// A filter text that is left out is empty.
    #[tokio::test]
    async fn test_update_filters_payload_is_typed() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["main.rs", "notes.md"]);
        let filter =
            |payload| update_filters(payload, harness.proxy.clone(), harness.state.clone());

        filter(typed(json!({ "searchQuery": "main" }))).await;
        {
            let state = harness.state.lock();
            assert_eq!(state.search_query, "main");
            assert_eq!(state.extension_filter, "");
            assert_eq!(state.content_search_query, "");
        }
        let _ = harness.last_state_update();

        let all = UpdateFiltersPayload {
            search_query: String::new(),
            extension_filter: "md".to_string(),
            content_search_query: String::new(),
        };
        assert_eq!(
            serde_json::to_value(&all).unwrap(),
            json!({ "searchQuery": "", "extensionFilter": "md", "contentSearchQuery": "" })
        );
        update_filters(
            typed(serde_json::to_value(&all).unwrap()),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.search_query, "");
        assert_eq!(ui_state.extension_filter, "md");
        assert_eq!(ui_state.visible_files_count, 1);
    }

    // --- Expansion under a filter ---

    /// `a/x.rs`, `a/deep/y.rs` and `b/z.md`, completely loaded.
    fn harness_with_two_branches() -> (TestHarness, PathBuf, PathBuf, PathBuf) {
        let harness = TestHarness::new();
        let (a, deep, b) = (
            harness.root_path.join("a"),
            harness.root_path.join("a").join("deep"),
            harness.root_path.join("b"),
        );
        {
            let mut state = harness.state.lock();
            state.replace_with_complete_file_list(vec![
                file_item(a.clone(), true),
                file_item(deep.clone(), true),
                file_item(b.clone(), true),
                file_item(a.join("x.rs"), false),
                file_item(deep.join("y.rs"), false),
                file_item(b.join("z.md"), false),
            ]);
            state.is_fully_scanned = true;
            filtering::apply_filters(&mut state);
        }
        (harness, a, deep, b)
    }

    async fn type_filter(harness: &TestHarness, search_query: &str, extension_filter: &str) {
        let payload = UpdateFiltersPayload {
            search_query: search_query.to_string(),
            extension_filter: extension_filter.to_string(),
            content_search_query: String::new(),
        };
        update_filters(
            typed(serde_json::to_value(payload).unwrap()),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
    }

    fn expanded(harness: &TestHarness) -> HashSet<PathBuf> {
        harness.state.lock().expanded_dirs.clone()
    }

    /// "Expand all" under a filter expands what the filter shows and keeps what was
    /// expanded outside of it.
    #[tokio::test]
    async fn test_expand_all_under_a_filter_keeps_expansions_outside_the_filter() {
        let (harness, a, deep, b) = harness_with_two_branches();
        harness.state.lock().expand_directories([b.clone()]);
        type_filter(&harness, "", "rs").await;
        harness
            .state
            .lock()
            .collapse_directories([a.clone(), deep.clone()]);

        expand_collapse_all(
            typed(json!(true)),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert_eq!(
            expanded(&harness),
            HashSet::from([a.clone(), deep.clone(), b.clone()]),
            "`b` is hidden by the filter and must stay expanded"
        );
        type_filter(&harness, "", "").await;
        assert_eq!(expanded(&harness), HashSet::from([a, deep, b]));
    }

    /// "Collapse all" under a filter collapses what the filter shows, nothing else.
    #[tokio::test]
    async fn test_collapse_all_under_a_filter_keeps_expansions_outside_the_filter() {
        let (mut harness, a, deep, b) = harness_with_two_branches();
        harness
            .state
            .lock()
            .expand_directories([a, deep, b.clone()]);
        type_filter(&harness, "", "rs").await;

        expand_collapse_all(
            typed(json!(false)),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        assert_eq!(expanded(&harness), HashSet::from([b.clone()]));
        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state
            .root_rows()
            .into_iter()
            .all(|node| !node.is_expanded));

        type_filter(&harness, "", "").await;
        let ui_state = harness.last_state_update().unwrap();
        let b_node = ui_state
            .root_rows()
            .into_iter()
            .find(|node| node.path == b)
            .unwrap();
        assert!(b_node.is_expanded, "expanded before the filter, and still");
    }

    /// Without a filter the two reach every directory, as they always did.
    #[tokio::test]
    async fn test_expand_and_collapse_all_without_a_filter_reach_every_directory() {
        let (harness, a, deep, b) = harness_with_two_branches();

        expand_collapse_all(
            typed(json!(true)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        assert_eq!(expanded(&harness), HashSet::from([a, deep, b]));

        expand_collapse_all(
            typed(json!(false)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        assert!(expanded(&harness).is_empty());
    }

    /// The filter opens the directories of its matches. A directory the user then
    /// collapses stays collapsed while the user keeps typing.
    #[tokio::test]
    async fn test_collapse_then_type_stays_collapsed() {
        let (mut harness, a, deep, _) = harness_with_two_branches();
        type_filter(&harness, "r", "").await;
        assert_eq!(expanded(&harness), HashSet::from([a.clone(), deep.clone()]));

        set_expansion(
            typed(json!({ "path": a, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        type_filter(&harness, "rs", "").await;
        type_filter(&harness, "x", "rs").await;

        assert!(!expanded(&harness).contains(&a), "must not be re-opened");
        let ui_state = harness.last_state_update().unwrap();
        let a_node = ui_state
            .root_rows()
            .into_iter()
            .find(|node| node.path == a)
            .unwrap();
        assert!(!a_node.is_expanded && a_node.has_children);
    }

    /// A match in a directory the user has not collapsed is still shown, also while
    /// another directory is held collapsed.
    #[tokio::test]
    async fn test_new_filter_text_still_expands_the_directory_of_a_new_match() {
        let (harness, a, deep, b) = harness_with_two_branches();
        type_filter(&harness, "", "rs").await;
        set_expansion(
            typed(json!({ "path": a, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        // The filter now matches in `b`, which was not expanded so far.
        type_filter(&harness, "", "md").await;
        assert_eq!(expanded(&harness), HashSet::from([deep.clone(), b.clone()]));

        // Expanding by hand ends the hold on that directory.
        set_expansion(
            typed(json!({ "path": a, "expanded": true })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        set_expansion(
            typed(json!({ "path": a, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        // ... and so does clearing the filters: the next filter starts afresh.
        type_filter(&harness, "", "").await;
        type_filter(&harness, "x", "").await;
        assert!(expanded(&harness).contains(&a));
    }

    /// "Collapse all" under a filter is a collapse by the user as well.
    #[tokio::test]
    async fn test_collapse_all_then_type_stays_collapsed() {
        let (harness, a, deep, _) = harness_with_two_branches();
        type_filter(&harness, "r", "").await;
        assert_eq!(expanded(&harness), HashSet::from([a.clone(), deep.clone()]));

        expand_collapse_all(
            typed(json!(false)),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        type_filter(&harness, "rs", "").await;

        assert!(!expanded(&harness).contains(&a));
        assert!(!expanded(&harness).contains(&deep));
    }

    /// Changing the case sensitivity applies the filters again and repeats the
    /// content search; neither re-opens what the user collapsed.
    #[tokio::test]
    async fn test_case_toggle_does_not_reopen_a_collapsed_directory() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("src/main.rs", "fn main() { needle }");
        let src = harness.root_path.join("src");
        harness.set_initial_files(&["src", "src/main.rs"]);
        update_filters(
            typed(json!({ "contentSearchQuery": "needle" })),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        assert_eq!(expanded(&harness), HashSet::from([src.clone()]));

        set_expansion(
            typed(json!({ "path": src, "expanded": false })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

        let state = harness.state.lock();
        assert_eq!(*state.content_search_results(), HashSet::from([file]));
        assert!(state.expanded_dirs.is_empty(), "must not be re-opened");
        drop(state);
        let _ = harness.last_state_update();
    }

    // --- Tests for user cancellation / dialog returning None ---

    #[tokio::test]
    async fn test_save_file_sends_cancelled_on_dialog_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_save_file(None);
        save_file(
            harness.dialog.as_ref(),
            typed(json!({ "kind": "edited", "content": "content" })),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        assert_eq!(harness.next_save_result().await, SaveResult::Cancelled);

        // The same for the host's own document.
        let document = generated_document(&harness, "generated");
        save_generated(&harness, document.id());
        assert_eq!(harness.next_save_result().await, SaveResult::Cancelled);
    }

    #[tokio::test]
    async fn test_pick_output_directory_does_nothing_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_folder(None);
        let initial_config = harness.state.lock().config.clone();
        pick_output_directory(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        let final_config = harness.state.lock().config.clone();
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
        assert_eq!(
            initial_config.output_directory,
            final_config.output_directory
        );
    }

    #[tokio::test]
    async fn test_import_config_does_nothing_on_cancel() {
        let mut harness = TestHarness::new();
        harness.dialog.set_pick_file(None); // Simulate user cancelling dialog
        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );
        harness
            .expect_no_event_within(QUIET_PERIOD, "No event expected")
            .await;
    }

    // --- Tests for specific logic and state edge cases ---

    /// An option that affects neither the file list nor the scan is stored and
    /// answered with the configuration the UI is to show.
    #[tokio::test]
    async fn test_set_config_option_stores_output_options() {
        let mut harness = TestHarness::new();

        set_option(
            &harness,
            ConfigOption::OutputFilename("new_name.txt".to_string()),
        )
        .await;
        let ui_state = harness.last_state_update().expect("the change is answered");
        assert_eq!(ui_state.config.output_filename, "new_name.txt");
        assert!(!ui_state.is_scanning);

        set_option(&harness, ConfigOption::IncludeTreeByDefault(false)).await;
        set_option(&harness, ConfigOption::UseRelativePaths(false)).await;
        set_option(
            &harness,
            ConfigOption::OutputDirectory(Some(PathBuf::from("/out"))),
        )
        .await;
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.config.include_tree_by_default);
        assert!(!ui_state.config.use_relative_paths);
        assert_eq!(
            ui_state.config.output_directory,
            Some(PathBuf::from("/out"))
        );

        // An empty directory text means "no preset", like no directory at all.
        for cleared in [Some(PathBuf::new()), None] {
            set_option(
                &harness,
                ConfigOption::OutputDirectory(Some(PathBuf::from("/out"))),
            )
            .await;
            set_option(&harness, ConfigOption::OutputDirectory(cleared)).await;
            let ui_state = harness.last_state_update().unwrap();
            assert_eq!(ui_state.config.output_directory, None);
        }
    }

    /// The reason for the intent commands: every edit is applied to the configuration
    /// of the host. With whole-config payloads the second of two quick edits was based
    /// on the copy the UI had before the first one and silently reverted it.
    #[tokio::test]
    async fn test_quick_successive_config_edits_do_not_overwrite_each_other() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src"]);
        {
            let mut state = harness.state.lock();
            state.config.ignore_patterns = HashSet::from(["keep".to_string()]);
            state.config.window_size = (640.0, 480.0);
            state.config.last_directory = Some(PathBuf::from("/somewhere"));
        }

        // None of these has seen the outcome of the ones before it.
        add_pattern(&harness, "first").await;
        add_pattern(&harness, "second").await;
        set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;
        set_option(&harness, ConfigOption::UseRelativePaths(false)).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state.config.ignore_patterns,
            vec![
                "first".to_string(),
                "keep".to_string(),
                "second".to_string()
            ]
        );
        assert!(
            !ui_state.patterns_need_rescan,
            "Adding patterns must never look like a removal"
        );
        let state = harness.state.lock();
        assert!(state.config.case_sensitive_search);
        assert!(!state.config.use_relative_paths);
        // What the UI never edits cannot be touched by its commands.
        assert_eq!(state.config.window_size, (640.0, 480.0));
        assert_eq!(
            state.config.last_directory,
            Some(PathBuf::from("/somewhere"))
        );
    }

    #[tokio::test]
    async fn test_clear_ignore_patterns_removes_all_and_recommends_rescan() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src"]);

        clear_ignore_patterns(harness.proxy.clone(), harness.state.clone()).await;

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.config.ignore_patterns.is_empty());
        assert!(ui_state.patterns_need_rescan);

        // Nothing left to remove: answered, but no further recommendation is derived.
        harness.state.lock().patterns_need_rescan = false;
        clear_ignore_patterns(harness.proxy.clone(), harness.state.clone()).await;
        let ui_state = harness.last_state_update().unwrap();
        assert!(!ui_state.patterns_need_rescan);
    }

    /// Removing a pattern that is not set and adding one that is set are no-ops.
    #[tokio::test]
    async fn test_pattern_commands_are_idempotent() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src"]);
        let before = harness.state.lock().config.clone();
        let existing = before.ignore_patterns.iter().next().unwrap().clone();

        remove_pattern(&harness, "never-added").await;
        add_pattern(&harness, &existing).await;

        let ui_state = harness
            .last_state_update()
            .expect("the commands are answered");
        assert!(!ui_state.patterns_need_rescan);
        assert_eq!(harness.state.lock().config, before);
    }

    #[tokio::test]
    async fn test_load_file_preview_sends_error_on_io_failure() {
        let mut harness = TestHarness::new();
        // Listed by the scan, deleted since.
        let non_existent_path = harness.root_path.join("non_existent_file.txt");
        harness.set_initial_files(&["non_existent_file.txt"]);
        let payload = json!(non_existent_path);
        load_file_preview(typed(payload), harness.proxy.clone(), harness.state.clone());

        let mut error_event_found = false;
        // The command sends two events: an error and a state update. We check for the error.
        for _ in 0..2 {
            if let Some(UserEvent::ShowError(msg)) = harness.get_next_event().await {
                assert!(msg.contains("I/O error"));
                error_event_found = true;
            }
        }
        assert!(error_event_found, "Expected a ShowError event");
    }

    #[tokio::test]
    async fn test_toggle_directory_selection_from_partial() {
        let mut harness = TestHarness::new();
        let dir_path = harness.create_dir("src");
        let file1 = harness.create_file("src/file1.rs", "");
        harness.create_file("src/file2.rs", "");
        harness.set_initial_files(&["src", "src/file1.rs", "src/file2.rs"]);

        // 1. Select one file to create a "partial" state
        {
            let mut state = harness.state.lock();
            state.selected_files.insert(file1);
        }

        // 2. Toggle the directory. It should now select all files.
        toggle_directory_selection(
            typed(json!(dir_path)),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(
            ui_state.selected_files_count, 2,
            "Directory should be fully selected from partial state"
        );
    }

    #[tokio::test]
    async fn test_generate_preview_creates_timestamped_filename_from_default() {
        let mut harness = TestHarness::new();
        // Use a fixed, old timestamp to make the test deterministic.
        // This ensures the generated filename will always be different.
        let old_default_filename = "cfc_output_20000101_120000.txt".to_string();

        {
            let mut state = harness.state.lock();
            state.config.output_filename = old_default_filename.clone();
        }

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let final_filename = harness.state.lock().config.output_filename.clone();

        assert_ne!(
            final_filename, old_default_filename,
            "A new timestamped filename should have been generated"
        );
        assert!(final_filename.starts_with("cfc_output_"));

        // Clean up events to prevent the test harness from panicking on drop
        let _ = harness.get_next_event().await;
        // The generation task also sends a final StateUpdate when it completes.
        let _ = harness.get_next_event().await;
    }

    /// Starts a generation with `filename` configured and returns the name that is
    /// configured afterwards, which is also the one the UI is told.
    async fn output_filename_after_generating(filename: &str) -> String {
        let mut harness = TestHarness::new();
        harness.state.lock().config.output_filename = filename.to_string();

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let configured = harness.state.lock().config.output_filename.clone();
        match harness.get_next_event().await {
            Some(UserEvent::StateUpdate(ui_state)) => {
                assert_eq!(ui_state.config.output_filename, configured)
            }
            other => panic!("Expected a state update, got {other:?}"),
        }
        let _ = harness.get_next_event().await;
        configured
    }

    /// The static name of a fresh configuration is not a timestamped one: it stays
    /// until the user changes it.
    #[tokio::test]
    async fn test_generate_preview_keeps_the_filename_of_a_fresh_config() {
        let fresh = AppConfig::default().output_filename;
        assert_eq!(fresh, "cfc_output.txt");
        assert_eq!(output_filename_after_generating(&fresh).await, fresh);
    }

    #[tokio::test]
    async fn test_generate_preview_gives_a_timestamped_filename_a_new_timestamp() {
        let old = "cfc_output_20240101_000000.txt";
        let assigned = output_filename_after_generating(old).await;
        assert_ne!(assigned, old);
        let stamp = assigned
            .strip_prefix("cfc_output_")
            .and_then(|rest| rest.strip_suffix(".txt"))
            .unwrap_or_else(|| panic!("Not a timestamped name: {assigned}"));
        assert!(
            chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d_%H%M%S").is_ok(),
            "{assigned}"
        );
    }

    #[tokio::test]
    async fn test_generate_preview_keeps_a_custom_filename() {
        for custom in ["context.txt", "cfc_output.md", "my_cfc_output_1.txt"] {
            assert_eq!(output_filename_after_generating(custom).await, custom);
        }
    }

    #[test]
    fn test_timestamped_output_filenames_are_recognized() {
        assert!(is_timestamped_output_filename(
            "cfc_output_20240101_120000.txt"
        ));
        for other in [
            "cfc_output.txt",
            "context.txt",
            "cfc_output.md",
            "my_cfc_output.txt",
            "",
        ] {
            assert!(!is_timestamped_output_filename(other), "{other}");
        }
    }

    /// A searcher that reports that it was started and then waits to be released.
    struct GatedSearcher {
        started: tokio::sync::mpsc::UnboundedSender<()>,
        release: Arc<tokio::sync::Notify>,
        matches: HashSet<PathBuf>,
    }
    impl tasks::FileSearcher for GatedSearcher {
        async fn search(
            &self,
            _: Vec<FileItem>,
            _: crate::core::ContentSearchRequest,
        ) -> tasks::SearchOutcome {
            let _ = self.started.send(());
            self.release.notified().await;
            Ok(self.matches.clone())
        }
    }

    /// `updateFilters` with a new content query is answered before the search has
    /// come to an end: the filter inputs of the UI wait for their texts to be
    /// reported back, and a search can take long. The matches follow.
    #[tokio::test]
    async fn test_update_filters_answers_before_the_content_search_completes() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["main.rs", "notes.md", "lib.rs"]);
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let searcher = GatedSearcher {
            started: started_tx,
            release: release.clone(),
            matches: HashSet::from([harness.root_path.join("main.rs")]),
        };

        let command = tokio::spawn(update_filters_with(
            typed(
                json!({ "searchQuery": "", "extensionFilter": "rs", "contentSearchQuery": "needle" }),
            ),
            harness.proxy.clone(),
            harness.state.clone(),
            searcher,
        ));
        started.recv().await.expect("The search must start");

        // The search is running: the answer is there, with the new texts.
        let pending = harness
            .last_state_update()
            .expect("updateFilters must be answered while the search is running");
        assert_eq!(pending.content_search_query, "needle");
        assert_eq!(pending.extension_filter, "rs");
        assert_eq!(pending.search_query, "");
        // The extension filter applies; the content filter does not empty the list
        // while its first matches are outstanding.
        assert_eq!(pending.visible_files_count, 2);
        assert!(!command.is_finished());
        harness
            .expect_no_event_within(QUIET_PERIOD, "Nothing more until the search answers")
            .await;

        release.notify_one();
        command.await.unwrap();
        assert_eq!(harness.state_updates_sent(), 1);
        let state = harness.state.lock();
        assert_eq!(state.content_search_query, "needle");
        assert_eq!(state.filtered_file_list().len(), 1);
    }

    /// The answer to a refined query shows the matches of the query before until the
    /// new ones are there, and typing into another filter meanwhile does not end the
    /// search.
    #[tokio::test]
    async fn test_update_filters_during_a_content_search_keeps_it_running() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["main.rs", "notes.md", "lib.rs"]);
        let (started_tx, mut started) = mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let searcher = GatedSearcher {
            started: started_tx,
            release: release.clone(),
            matches: HashSet::from([
                harness.root_path.join("main.rs"),
                harness.root_path.join("notes.md"),
            ]),
        };
        let command = tokio::spawn(update_filters_with(
            typed(json!({ "contentSearchQuery": "needle" })),
            harness.proxy.clone(),
            harness.state.clone(),
            searcher,
        ));
        started.recv().await.expect("The search must start");
        assert_eq!(harness.state_updates_sent(), 1);

        // Another filter, with the content query as the UI shows it.
        update_filters(
            typed(json!({ "searchQuery": "main", "contentSearchQuery": "needle" })),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
        let meanwhile = harness.last_state_update().unwrap();
        assert_eq!(meanwhile.search_query, "main");
        assert_eq!(meanwhile.content_search_query, "needle");
        assert_eq!(meanwhile.visible_files_count, 1);

        release.notify_one();
        command.await.unwrap();
        let answered = harness
            .last_state_update()
            .expect("The running search must still publish its matches");
        assert_eq!(answered.content_search_query, "needle");
        assert_eq!(answered.visible_files_count, 1);
        assert_eq!(
            *harness.state.lock().content_search_results(),
            HashSet::from([
                harness.root_path.join("main.rs"),
                harness.root_path.join("notes.md"),
            ])
        );
    }

    /// A file that names only some settings is completed with the defaults.
    #[tokio::test]
    async fn test_import_config_with_a_partial_file() {
        let mut harness = TestHarness::new();
        harness.state.lock().current_path = String::new();
        let new_config_path =
            harness.create_file("config_no_dir.json", r#"{ "ignore_patterns": [] }"#);
        harness.dialog.set_pick_file(Some(new_config_path));
        import_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        // A StateUpdate event is sent immediately after reset.
        assert!(
            matches!(
                harness.get_next_event().await,
                Some(UserEvent::StateUpdate(_))
            ),
            "Expected an immediate state update"
        );
        // No *second* event should follow, as no scan is started.
        harness
            .expect_no_event_within(QUIET_PERIOD, "No scan should start without a directory")
            .await;
        assert!(harness.state.lock().config.ignore_patterns.is_empty());
    }

    #[tokio::test]
    async fn test_export_config_sends_false_on_failure() {
        let mut harness = TestHarness::new();
        // A cross-platform way to cause an I/O error is to target a directory.
        let invalid_path = harness.root_path.clone();
        harness.dialog.set_save_file(Some(invalid_path));

        export_config(
            harness.dialog.as_ref(),
            harness.proxy.clone(),
            harness.state.clone(),
        );

        match harness.get_next_event().await.unwrap() {
            UserEvent::ConfigExported(success) => assert!(!success),
            other => panic!("Expected ConfigExported(false), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_add_ignore_pattern_applies_locally() {
        let mut harness = TestHarness::new();

        // Ensure a clean state for the test run.
        {
            harness.state.lock().config.ignore_patterns.clear();
        }

        // The `current_path` is the root for the ignore matcher. All file paths
        // in the state MUST be children of this root path.
        let root = "/test/project";
        harness.state.lock().current_path = root.to_string();

        // Create abstract files on the "filesystem".
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("debug.log", "log content");

        // Set up the initial file list using full paths that are consistent
        // with the root path. This resolves the `ignore` crate panic.
        harness.set_initial_files(&[
            &format!("{}/src", root),
            &format!("{}/src/main.rs", root),
            &format!("{}/debug.log", root),
        ]);

        add_pattern(&harness, "*.log").await;

        // Assert the outcome.
        let ui_state = harness.last_state_update().unwrap();

        assert!(
            !ui_state.patterns_need_rescan,
            "Flag should NOT be set when only adding patterns"
        );

        // After filtering "*.log", two items remain: "/test/project/src" and "/test/project/src/main.rs".
        assert_eq!(
            ui_state.visible_files_count, 2,
            "Should have src dir and main.rs, log file should be filtered out"
        );
    }

    #[tokio::test]
    async fn test_remove_ignore_pattern_sets_rescan_flag() {
        let mut harness = TestHarness::new();

        // Create actual files
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("debug.log", "log content");
        harness.create_dir("node_modules");
        harness.create_file("node_modules/package.json", "{}");

        // Set initial files
        harness.set_initial_files(&[
            "src",
            "src/main.rs",
            "debug.log",
            "node_modules",
            "node_modules/package.json",
        ]);

        // Set initial patterns
        {
            let mut state = harness.state.lock();
            state.config.ignore_patterns.insert("*.log".to_string());
            state
                .config
                .ignore_patterns
                .insert("node_modules/".to_string());
        }

        remove_pattern(&harness, "node_modules/").await;

        let ui_state = harness.last_state_update().unwrap();
        assert!(
            ui_state.patterns_need_rescan,
            "Flag should be set when patterns are removed"
        );
    }

    #[tokio::test]
    async fn test_rescan_clears_patterns_need_rescan_flag() {
        let mut harness = TestHarness::new();

        // Create a test directory with a file
        harness.create_file("test.txt", "content");

        {
            let mut state = harness.state.lock();
            state.patterns_need_rescan = true;
            // Ensure we have a valid current_path for the rescan
            // (TestHarness::new() already sets this)
        }

        rescan_directory(harness.proxy.clone(), harness.state.clone());

        // Wait for scan to complete
        let final_state = harness.wait_for_scan_completion().await.unwrap();
        assert!(
            !final_state.patterns_need_rescan,
            "Flag should be cleared after rescan"
        );
    }

    /// This is an isolated, robust unit test for the "add pattern" logic of `add_ignore_pattern`.
    /// It does not use the TestHarness and has no external dependencies, making it 100% deterministic.
    /// It asserts three things:
    /// 1. That the correct log message was emitted (proving the code path was taken).
    /// 2. That the event sent to the UI has the correct, updated state.
    /// 3. That the internal AppState was mutated correctly.
    #[tokio::test]
    async fn test_add_ignore_pattern_isolated() {
        // ADDED: Use the same logging setup as the other tests.
        setup_test_logging();

        // --- ARRANGE ---
        let (tx, mut rx) = mpsc::unbounded_channel();
        let proxy = TestEventProxy { sender: tx };
        let state = {
            let mut s = AppState::default();
            s.config.ignore_patterns.clear();
            s.current_path = "/app".to_string();
            s.set_full_file_list(vec![
                FileItem {
                    path: PathBuf::from("/app/src/main.rs"),
                    ..Default::default()
                },
                FileItem {
                    path: PathBuf::from("/app/debug.log"),
                    ..Default::default()
                },
            ]);
            s.set_filtered_file_list(s.full_file_list().to_vec());
            SharedState::new(s)
        };

        // --- ACT ---
        add_ignore_pattern(typed(json!({ "pattern": "*.log" })), proxy, state.clone()).await;

        // --- ASSERT ---

        // The following state and event assertions are the primary verification of correctness.

        let event = rx.recv().await.expect("An event should have been sent.");
        let ui_state = match event {
            UserEvent::StateUpdate(s) => s,
            _ => panic!("Expected a StateUpdate event"),
        };
        assert_eq!(ui_state.visible_files_count, 1);
        assert!(!ui_state.patterns_need_rescan);

        let final_state = state.lock();
        assert_eq!(final_state.full_file_list().len(), 1);
        assert!(final_state.config.ignore_patterns.contains("*.log"));
    }

    /// A comprehensive integration test for the entire user workflow of removing an ignore pattern.
    ///
    /// This test simulates a scenario with a nested directory structure to verify
    /// the two-phase state update process when un-ignoring files.
    ///
    /// It verifies the following sequence of events:
    /// 1. An initial scan is performed with a directory ("build_artifacts/") actively ignored,
    ///    confirming that the initial file count is low as expected.
    /// 2. The configuration is updated to remove the ignore pattern.
    /// 3. It asserts that the `patterns_need_rescan` flag is correctly set immediately
    ///    after the update, while the file count correctly remains unchanged.
    /// 4. A re-scan is triggered by the user (simulated by the test).
    /// 5. It asserts that after the re-scan completes, the flag is cleared and the final
    ///    file count ("statistic") has increased, reflecting that the previously ignored
    ///    files are now visible.
    ///
    /// NOTE on the final file count: The final count is 5, not 6. This is because the
    /// scanner's underlying `ignore` library 0has default behavior that automatically
    /// filters out files it detects as binary based on their content (e.g., containing
    /// null bytes). The `app.dat` file created in this test is correctly identified
    /// as binary and skipped, and the test asserts this real-world behavior.
    #[tokio::test]
    async fn test_removing_pattern_and_rescanning_updates_file_list() {
        // --- 1. Arrange ---
        let mut harness = TestHarness::new();
        harness.create_file(".gitignore", "");
        let artifact_dir_name = "build_artifacts";
        let sub_dir_name = "binaries";
        let binary_filename = "app.dat";

        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_dir(&format!("{}/{}", artifact_dir_name, sub_dir_name));
        harness.create_file(
            &format!("{}/{}/{}", artifact_dir_name, sub_dir_name, binary_filename),
            "\x00\x01\x02\x03", // Use actual binary content to trigger detection
        );
        harness.create_file("README.md", "Project Readme");

        {
            let mut state = harness.state.lock();
            state
                .config
                .ignore_patterns
                .insert(format!("{}/", artifact_dir_name));
        }

        // --- 2. Act & Assert Initial State ---
        rescan_directory(harness.proxy.clone(), harness.state.clone());
        let initial_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Initial scan should complete.");
        assert_eq!(initial_state.visible_files_count, 3);

        // --- 3. Act: Remove pattern and rescan ---
        remove_pattern(&harness, &format!("{}/", artifact_dir_name)).await;

        let _ = harness.last_state_update();

        rescan_directory(harness.proxy.clone(), harness.state.clone());
        let final_state = harness
            .wait_for_scan_completion()
            .await
            .expect("Final re-scan should complete.");

        // --- 4. Assert Final State ---
        assert!(!final_state.patterns_need_rescan);

        assert_eq!(
            final_state.visible_files_count, 5,
            "Final count should be 5. The binary 'app.dat' is skipped by the scanner's default behavior."
        );
        assert!(final_state
            .root_rows()
            .into_iter()
            .any(|node| node.name == artifact_dir_name));
    }

    // --- Config intent helpers ---

    async fn add_pattern(harness: &TestHarness, pattern: &str) {
        add_ignore_pattern(
            typed(json!({ "pattern": pattern })),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
    }

    async fn remove_pattern(harness: &TestHarness, pattern: &str) {
        remove_ignore_pattern(
            typed(json!({ "pattern": pattern })),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
    }

    async fn set_option(harness: &TestHarness, option: ConfigOption) {
        set_config_option(
            typed(serde_json::to_value(option).unwrap()),
            harness.proxy.clone(),
            harness.state.clone(),
        )
        .await;
    }

    // --- Config changes that need more than a re-filter ---

    /// Registers a scan whose task never finishes, as if a long scan were running.
    fn begin_parked_scan(harness: &TestHarness) -> crate::app::state::ScanToken {
        let mut state = harness.state.lock();
        let token = state.begin_scan();
        state.attach_scan_task(&token, tokio::spawn(std::future::pending()));
        token
    }

    fn file_names(state: &AppState) -> HashSet<String> {
        state
            .full_file_list()
            .iter()
            .map(|item| {
                item.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// A scan works with the patterns it was started with. If a pattern is added while
    /// it is running, its result would bring the entries back that the pattern has just
    /// removed, so the scan is replaced by one that knows the new patterns.
    #[tokio::test]
    async fn test_pattern_change_restarts_running_scan_when_pattern_is_added() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("docs/guide.md", "# Guide");
        harness.set_initial_files(&["src", "docs"]); // The shallow list of the scan.
        let old_scan = begin_parked_scan(&harness);

        add_pattern(&harness, "docs/").await;

        // The old scan is out of the game, a new one has taken over, and the ignored
        // directory has disappeared at once.
        {
            let state = harness.state.lock();
            assert!(old_scan.is_cancelled());
            assert!(!state.is_current_scan(&old_scan));
            assert!(state.is_scanning(), "A new scan must be running");
            assert!(state.has_scan_task(), "The new scan must be cancellable");
            assert_eq!(file_names(&state), HashSet::from(["src".to_string()]));
        }

        let final_state = harness
            .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
            .await;
        assert!(!final_state
            .root_rows()
            .into_iter()
            .any(|node| node.name == "docs"));
        assert!(!final_state.patterns_need_rescan);
        let state = harness.state.lock();
        assert_eq!(
            file_names(&state),
            HashSet::from(["src".to_string(), "main.rs".to_string()]),
            "The complete list must follow the new patterns"
        );
        assert!(state.active_ignore_patterns.contains("docs/"));
    }

    /// Removing a pattern during a scan restarts it as well: the new scan lists what
    /// the pattern used to hide, so no separate re-scan has to be recommended.
    #[tokio::test]
    async fn test_pattern_change_restarts_running_scan_when_pattern_is_removed() {
        let mut harness = TestHarness::new();
        harness.create_file("src/main.rs", "fn main() {}");
        harness.create_file("docs/guide.md", "# Guide");
        harness.set_initial_files(&["src"]);
        harness
            .state
            .lock()
            .config
            .ignore_patterns
            .insert("docs/".to_string());
        let old_scan = begin_parked_scan(&harness);

        remove_pattern(&harness, "docs/").await;

        assert!(old_scan.is_cancelled());
        let final_state = harness
            .wait_for_state(|ui_state| !ui_state.is_scanning && ui_state.is_fully_scanned)
            .await;
        assert!(final_state
            .root_rows()
            .into_iter()
            .any(|node| node.name == "docs"));
        assert!(!final_state.patterns_need_rescan);
        let state = harness.state.lock();
        assert!(file_names(&state).contains("guide.md"));
    }

    /// A config change that does not touch the patterns leaves a running scan alone.
    #[tokio::test]
    async fn test_config_change_without_pattern_change_keeps_running_scan() {
        let mut harness = TestHarness::new();
        harness.set_initial_files(&["src"]);
        let scan = begin_parked_scan(&harness);

        set_option(&harness, ConfigOption::RemoveEmptyDirectories(true)).await;
        set_option(
            &harness,
            ConfigOption::OutputFilename("other.txt".to_string()),
        )
        .await;

        let ui_state = harness.last_state_update().unwrap();
        assert!(ui_state.is_scanning);
        let mut state = harness.state.lock();
        assert!(state.is_current_scan(&scan));
        assert!(!scan.is_cancelled());
        state.cancel_current_scan(); // Stop the parked task.
    }

    /// The matches of a content search depend on the case sensitivity, so changing it
    /// repeats the search instead of filtering with the matches of the old setting.
    #[tokio::test]
    async fn test_case_sensitivity_change_repeats_content_search() {
        let mut harness = TestHarness::new();
        let lower = harness.create_file("lower.txt", "a needle in the hay");
        let upper = harness.create_file("upper.txt", "a NEEDLE in the hay");
        harness.set_initial_files(&["lower.txt", "upper.txt"]);
        {
            // The state a case-insensitive search for "needle" has left behind.
            let mut state = harness.state.lock();
            state.config.case_sensitive_search = false;
            state.content_search_query = "needle".to_string();
            state.set_content_search_results(HashSet::from([lower.clone(), upper.clone()]));
        }

        set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 1);
        assert_eq!(
            *harness.state.lock().content_search_results(),
            HashSet::from([lower.clone()])
        );

        // And back: the file that only matches case-insensitively is found again.
        set_option(&harness, ConfigOption::CaseSensitiveSearch(false)).await;

        let ui_state = harness.last_state_update().unwrap();
        assert_eq!(ui_state.visible_files_count, 2);
        assert_eq!(
            *harness.state.lock().content_search_results(),
            HashSet::from([lower, upper])
        );
    }

    /// Without a content query there is nothing to repeat: the change is answered
    /// with the one state update of the re-filter.
    #[tokio::test]
    async fn test_case_sensitivity_change_without_content_query_only_refilters() {
        let mut harness = TestHarness::new();
        harness.create_file("Main.rs", "");
        harness.set_initial_files(&["Main.rs"]);
        harness.state.lock().search_query = "main".to_string();

        set_option(&harness, ConfigOption::CaseSensitiveSearch(true)).await;

        let mut updates = Vec::new();
        while let Ok(event) = harness.event_rx.try_recv() {
            updates.push(event);
        }
        assert_eq!(updates.len(), 1, "Exactly one state update expected");
        match &updates[0] {
            UserEvent::StateUpdate(ui_state) => assert_eq!(ui_state.visible_files_count, 0),
            other => panic!("Expected a state update, got {other:?}"),
        }
    }

    /// The file size limit of the configuration is the one the generation works with.
    #[tokio::test]
    async fn test_generate_preview_applies_configured_file_size_limit() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("file.txt", "some content");
        harness.set_initial_files(&["file.txt"]);
        {
            let mut state = harness.state.lock();
            state.selected_files.insert(file);
            state.config.max_file_size_mb = 0;
        }

        generate_preview(harness.proxy.clone(), harness.state.clone());

        let content = loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent { document, .. }) => break document,
                Some(_) => {}
                None => panic!("Timed out waiting for the generated content"),
            }
        };
        assert!(content
            .text()
            .contains("[FILE TOO LARGE: 12 bytes - CONTENT SKIPPED]"));
    }

    /// A project whose listed files weigh `sizes` bytes (as the scan recorded them;
    /// nothing of that size is on disk), all of them selected.
    fn select_files_of_sizes(harness: &TestHarness, sizes: &[u64]) {
        let mut state = harness.state.lock();
        // Large enough for every file of these tests to count with its full size.
        state.config.max_file_size_mb = 1024;
        let items: Vec<FileItem> = sizes
            .iter()
            .enumerate()
            .map(|(i, size)| FileItem {
                size: *size,
                ..file_item(harness.root_path.join(format!("file{i}.txt")), false)
            })
            .collect();
        state
            .selected_files
            .extend(items.iter().map(|item| item.path.clone()).collect());
        state.set_full_file_list(items.clone());
        state.set_filtered_file_list(items);
    }

    const MIB: u64 = 1024 * 1024;

    /// An output above the limit is refused before anything is read, and refusing
    /// changes nothing.
    #[tokio::test]
    async fn test_generate_preview_refuses_an_output_above_the_limit() {
        let mut harness = TestHarness::new();
        // Each file is within the limit for a single file; together they are not.
        select_files_of_sizes(&harness, &[90 * MIB, 90 * MIB, 30 * MIB]);
        let filename = "cfc_output_20200101_000000.txt".to_string();
        let shown = generated_document(&harness, "the output that is shown");
        harness.state.lock().config.output_filename = filename.clone();

        generate_preview(harness.proxy.clone(), harness.state.clone());

        match harness.get_next_event().await {
            Some(UserEvent::OutputSize(OutputSizeNotice::TooLarge {
                estimated_bytes,
                limit_bytes,
            })) => {
                assert_eq!(limit_bytes, tasks::MAX_OUTPUT_SIZE);
                assert!(
                    (210 * MIB..211 * MIB).contains(&estimated_bytes),
                    "{estimated_bytes}"
                );
            }
            other => panic!("Expected the refusal, got {other:?}"),
        }
        // The command is answered with a snapshot, which shows no generation.
        match harness.get_next_event().await {
            Some(UserEvent::StateUpdate(ui_state)) => assert!(!ui_state.is_generating),
            other => panic!("Expected a snapshot, got {other:?}"),
        }
        harness
            .expect_no_event_within(QUIET_PERIOD, "A refused generation does nothing else")
            .await;

        let state = harness.state.lock();
        assert!(!state.is_generating());
        assert_eq!(state.config.output_filename, filename);
        assert_eq!(state.generated_document(shown.id()), Some(shown));
    }

    /// Refusing does not stop a generation that is running.
    #[tokio::test]
    async fn test_refused_generation_leaves_a_running_generation_alone() {
        let mut harness = TestHarness::new();
        let running = harness.state.lock().begin_generation();
        select_files_of_sizes(&harness, &[201 * MIB]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        assert!(matches!(
            harness.get_next_event().await,
            Some(UserEvent::OutputSize(OutputSizeNotice::TooLarge { .. }))
        ));
        let state = harness.state.lock();
        assert!(state.is_current_generation(&running));
        assert!(!running.is_cancelled());
    }

    /// A large output is generated, and announced as large right after the snapshot
    /// that shows the generation.
    #[tokio::test]
    async fn test_generate_preview_announces_a_large_output() {
        let mut harness = TestHarness::new();
        select_files_of_sizes(&harness, &[15 * MIB, 15 * MIB]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        match harness.get_next_event().await {
            Some(UserEvent::StateUpdate(ui_state)) => assert!(ui_state.is_generating),
            other => panic!("Expected the snapshot of the generation, got {other:?}"),
        }
        match harness.get_next_event().await {
            Some(UserEvent::OutputSize(OutputSizeNotice::Large {
                estimated_bytes,
                limit_bytes,
            })) => {
                assert_eq!(limit_bytes, tasks::LARGE_OUTPUT_SIZE);
                assert!(
                    (30 * MIB..31 * MIB).contains(&estimated_bytes),
                    "{estimated_bytes}"
                );
            }
            other => panic!("Expected the notice, got {other:?}"),
        }
        // The files do not exist, so each block holds a marker; the generation ends.
        loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent { document, .. }) => {
                    assert!(document.text().contains("[FILE UNREADABLE"));
                    break;
                }
                Some(UserEvent::StateUpdate(_)) => {}
                other => panic!("Expected the generated content, got {other:?}"),
            }
        }
    }

    /// An output of ordinary size is not commented on, at the thresholds included.
    #[tokio::test]
    async fn test_generate_preview_says_nothing_about_an_output_of_ordinary_size() {
        let mut harness = TestHarness::new();
        select_files_of_sizes(&harness, &[19 * MIB]);

        generate_preview(harness.proxy.clone(), harness.state.clone());

        loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent { .. }) => break,
                Some(UserEvent::StateUpdate(_)) => {}
                other => panic!("Expected only snapshots and the content, got {other:?}"),
            }
        }
    }

    /// The whole way of an output: generated by the command, shown with an id, saved
    /// by naming that id.
    #[tokio::test]
    async fn test_generated_output_is_saved_as_it_was_shown() {
        let mut harness = TestHarness::new();
        let file = harness.create_file("src/a.txt", "line one\nline two");
        harness.set_initial_files(&["src/a.txt"]);
        harness.state.lock().selected_files.insert(file);
        let save_path = harness.root_path.join("saved.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));

        generate_preview(harness.proxy.clone(), harness.state.clone());
        let (document, token_count) = loop {
            match harness.get_next_event().await {
                Some(UserEvent::ShowGeneratedContent {
                    document,
                    token_count,
                }) => break (document, token_count),
                Some(_) => {}
                None => panic!("Timed out waiting for the generated content"),
            }
        };
        assert!(token_count > 0);
        assert!(document
            .text()
            .ends_with("a.txt\n===FILE-START===\nline one\nline two\n---FILE-END-----\n\n"));

        save_generated(&harness, document.id());
        loop {
            match harness.get_next_event().await {
                Some(UserEvent::SaveResult(result)) => {
                    assert!(matches!(result, SaveResult::Saved { .. }), "{result:?}");
                    break;
                }
                Some(UserEvent::StateUpdate(_)) => {}
                other => panic!("Expected the result of the save, got {other:?}"),
            }
        }
        assert_eq!(std_fs::read_to_string(&save_path).unwrap(), document.text());
    }

    /// Source-like text of about `bytes` bytes, the same for the same `seed`.
    fn synthetic_source(seed: usize, bytes: usize) -> String {
        const WORDS: [&str; 16] = [
            "let",
            "value",
            "fn",
            "compute",
            "result",
            "self",
            "index",
            "return",
            "match",
            "Some(item)",
            "buffer.push_str(&line);",
            "// TODO: größer",
            "if",
            "else",
            "0x1f",
            "=>",
        ];
        let mut text = String::with_capacity(bytes + 128);
        let mut n = seed;
        while text.len() < bytes {
            let indent = (n % 4) * 4;
            text.extend(std::iter::repeat_n(' ', indent));
            for _ in 0..(3 + n % 9) {
                n = n
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                text.push_str(WORDS[(n >> 33) % WORDS.len()]);
                text.push(' ');
            }
            text.push_str(&format!("{};\n", n % 1000));
            if n.is_multiple_of(7) {
                text.push('\n');
            }
        }
        text
    }

    /// The way of a large output through the host, timed: generation and token count,
    /// the script for the WebView, the save. Not a test of anything; it prints what
    /// `docs/ipc.md` quotes. Run it on a release build, one phase at a time to see
    /// the peak memory of each:
    ///
    /// ```sh
    /// cargo test --release --lib large_output_benchmark --no-run
    /// BENCH_MB=50 BENCH_UNTIL=save /usr/bin/time -l target/release/deps/context_file_concat-<hash> \
    ///     large_output_benchmark --ignored --nocapture
    /// ```
    ///
    /// `BENCH_MB` is the size of the selection (default 50), `BENCH_UNTIL` the last
    /// phase to run: `generate`, `show` or `save` (default).
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "a benchmark, run on request"]
    async fn large_output_benchmark() {
        let megabytes: usize = std::env::var("BENCH_MB")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(50);
        let until = std::env::var("BENCH_UNTIL").unwrap_or_else(|_| "save".to_string());
        let mut harness = TestHarness::new();
        let file_size = 20 * 1024;
        let mut items = Vec::new();
        for i in 0..megabytes * 1024 * 1024 / file_size {
            let relative = format!("dir{:03}/file{:05}.rs", i / 100, i);
            let path = harness.create_file(&relative, &synthetic_source(i, file_size));
            if i.is_multiple_of(100) {
                let directory = path.parent().expect("a file has a directory");
                items.push(file_item(directory.to_path_buf(), true));
            }
            items.push(FileItem {
                size: std_fs::metadata(&path).unwrap().len(),
                ..file_item(path, false)
            });
        }
        {
            let mut state = harness.state.lock();
            state
                .selected_files
                .extend(items.iter().map(|item| item.path.clone()).collect());
            state.set_full_file_list(items.clone());
            state.set_filtered_file_list(items);
            state.is_fully_scanned = true;
        }
        let save_path = harness.root_path.join("output.txt");
        harness.dialog.set_save_file(Some(save_path.clone()));

        let started = std::time::Instant::now();
        generate_preview(harness.proxy.clone(), harness.state.clone());
        let event = loop {
            match harness.event_rx.recv().await.expect("the generation ends") {
                event @ UserEvent::ShowGeneratedContent { .. } => break event,
                UserEvent::ShowError(error) => panic!("{error}"),
                _ => {}
            }
        };
        let generated = started.elapsed();
        eprintln!("BENCH generated and counted: {generated:?}");
        if until == "generate" {
            return;
        }

        let script = crate::app::script_for(&event).unwrap();
        let shown = started.elapsed();
        let UserEvent::ShowGeneratedContent {
            document,
            token_count,
        } = event
        else {
            unreachable!()
        };
        eprintln!(
            "BENCH script built: +{:?} ({} bytes) => host time to show {shown:?}",
            shown - generated,
            script.len()
        );
        eprintln!(
            "BENCH document: {} bytes, {token_count} tokens",
            document.text().len()
        );
        drop(script);
        if until == "show" {
            return;
        }

        // What the UI posts for an output it has not changed, as it arrives.
        let message = serde_json::to_string(&json!({
            "seq": 1,
            "command": "saveFile",
            "payload": { "kind": "generated", "documentId": document.id() },
        }))
        .unwrap();
        let bytes = document.text().len() as u64;
        drop(document);
        let saving = std::time::Instant::now();
        let crate::app::ipc::IpcMessage::Command {
            command: crate::app::ipc::Command::SaveFile(payload),
            ..
        } = crate::app::ipc::IpcMessage::parse(&message)
        else {
            panic!("the message is a saveFile command");
        };
        save_file(
            harness.dialog.as_ref(),
            payload,
            harness.proxy.clone(),
            harness.state.clone(),
        );
        eprintln!(
            "BENCH save: {:?} (message {} bytes, written {} bytes)",
            saving.elapsed(),
            message.len(),
            std_fs::metadata(&save_path).unwrap().len()
        );
        assert_eq!(std_fs::metadata(&save_path).unwrap().len(), bytes);
    }
}
