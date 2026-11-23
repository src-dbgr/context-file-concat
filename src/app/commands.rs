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
        // The file is written in the background; neither the lock nor the lane waits.
        if let Some(snapshot) = s.edit_config(|config| config.last_directory = None) {
            tasks::save_config_in_background(snapshot);
        }
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

    let previous = state_guard.config().clone();
    let changed = state_guard.edit_config(change);
    let config = state_guard.config();

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

    if let Some(snapshot) = changed {
        tasks::save_config_in_background(snapshot);
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
    if is_timestamped_output_filename(&state_guard.config().output_filename) {
        let new_filename = format!(
            "cfc_output_{}.txt",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );
        if let Some(snapshot) =
            state_guard.edit_config(|config| config.output_filename = new_filename)
        {
            tasks::save_config_in_background(snapshot);
        }
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
        max_file_size: state_guard.config().max_file_size_bytes(),
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
        (content, state_guard.config().clone())
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
            if let Some(snapshot) = s.edit_config(|config| config.output_directory = Some(path)) {
                tasks::save_config_in_background(snapshot);
            }
        });
    }
}

/// Imports a configuration file.
///
/// The settings a user edits and shares are taken from the file (ignore patterns,
/// search and output options, the size limit, see [`AppConfig::adopt_user_settings`]).
/// A window geometry the file may hold is ignored: it describes the display of whoever
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
    let changed = state_guard.edit_config(|config| config.adopt_user_settings(imported));
    state_guard.current_config_filename =
        path.file_name().and_then(|n| n.to_str()).map(String::from);
    if let Some(snapshot) = changed {
        tasks::save_config_in_background(snapshot);
    }

    // The UI is wiped clean before the scan (if any) announces itself.
    send_snapshot(&proxy, &state_guard);
    if let Some(directory) = directory_to_open {
        tasks::open_path(&mut state_guard, directory, proxy, state.clone());
    }
}

/// Exports the settings that are meant to be shared to a JSON file: patterns, search
/// and output options, the size limit. The directories of this installation are not
/// part of the file (see [`config::SharedConfig`]).
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
        let settings = config::SharedConfig::from(state.lock().config());
        let result = write(&settings, &path).is_ok();
        proxy.send_event(UserEvent::ConfigExported(result));
    }
}

#[cfg(test)]
mod tests;
