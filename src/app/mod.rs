//! The `app` module orchestrates the interaction between the `core` logic and the `ui`.
//!
//! It manages the application state, handles events from the WebView (IPC messages),
//! and sends updates back to the UI. It acts as the "controller" in an MVC-like pattern.

#[cfg(test)]
mod command_contract_tests;
pub mod commands;
pub mod correlation;
pub mod dispatch;
#[cfg(test)]
mod event_contract_tests;
pub mod events;
pub mod file_dialog;
pub mod filtering;
mod helpers;
pub(crate) mod ipc;
pub mod proxy;
pub mod selection;
pub mod shared_state;
pub mod snapshot;
mod staged;
pub mod state;
pub mod tasks;
pub mod tracked;
pub mod transport;
pub mod tree_index;
pub mod view_model;
#[cfg(test)]
mod window_contract_tests;
pub mod window_geometry;

use crate::app::file_dialog::DialogService;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use wry::WebView;

use crate::app::proxy::EventProxy;
use correlation::Correlated;
use dispatch::CommandDispatcher;
use events::UserEvent;
use ipc::{excerpt, Command, IpcMessage};
use shared_state::SharedState;

/// The main handler for IPC messages from the WebView: reads the message and queues
/// its command on the matching lane of `dispatcher`, which executes commands in arrival
/// order (see [`dispatch`]). Returns immediately, so the event loop is never blocked.
///
/// The dispatcher is owned by the caller (the composition root in `main.rs`), so there
/// is no process-wide state and tests can use their own instance.
///
/// - Commands that open a native dialog block for as long as the user likes. They run
///   on the blocking lane: they start after the commands that arrived before them, and
///   they neither hold up nor are ordered relative to the commands that arrive later,
///   while the dialog is open.
/// - Every other command runs on the ordered lane, in arrival order.
/// - A message that is no command is refused: it is logged and answered with the
///   unchanged snapshot, at its place in the order (see [`ipc`]).
///
/// The name of the message is handed to the dispatcher, and the events of the
/// command are sent through a proxy that writes into every snapshot how far the
/// commands have got (see [`correlation`]). The commands know nothing of either.
pub fn dispatch_ipc_message<P: EventProxy, D: DialogService + 'static>(
    dispatcher: &CommandDispatcher,
    message: String,
    dialog_service: Arc<D>,
    proxy: P,
    state: SharedState,
) {
    let proxy = Correlated::new(proxy, dispatcher.progress().clone());

    // What is logged of a message is short, whatever its size (see [`ipc`]).
    let (id, command) = match IpcMessage::parse(&message) {
        IpcMessage::Command { id, command } => (Some(id), Some(command)),
        IpcMessage::Refused { id, reason } => {
            tracing::warn!(
                "Refused IPC message {} of UI {}: {reason}",
                id.seq,
                id.session
            );
            (Some(id), None)
        }
        IpcMessage::Unreadable { reason } => {
            tracing::error!(
                "Refused an IPC message that cannot be read ({reason}): {}",
                excerpt(&message)
            );
            (None, None)
        }
    };

    let work = match command {
        Some(command) => command.into_work(dialog_service, proxy, state),
        None => at_once(move || commands::answer_unchanged(&proxy, &state)),
    };
    match work {
        Work::Ordered(command) => dispatcher.dispatch(id, command),
        Work::Blocking(command) => dispatcher.dispatch_blocking(id, command),
    }
}

/// Queues the opening of a path that was dropped onto the window.
///
/// A drop is a command like any other: it replaces the loaded directory, so it has to
/// be ordered relative to the IPC commands (and to other drops). It therefore runs on
/// the ordered lane of the same `dispatcher`. It has no number (the UI did not post
/// it); the snapshots it leads to carry the number of the last command of the UI.
pub fn dispatch_dropped_path<P: EventProxy>(
    dispatcher: &CommandDispatcher,
    path: PathBuf,
    proxy: P,
    state: SharedState,
) {
    let proxy = Correlated::new(proxy, dispatcher.progress().clone());
    dispatcher.dispatch(None, async move {
        commands::open_dropped_path(path, proxy, state)
    });
}

/// What a command does, on the lane it is done on.
enum Work {
    /// Runs on the ordered lane.
    Ordered(Pin<Box<dyn Future<Output = ()> + Send>>),
    /// Waits for a native dialog; runs on the blocking lane.
    Blocking(Box<dyn FnOnce() + Send>),
}

/// A command of the ordered lane that can suspend.
fn ordered(command: impl Future<Output = ()> + Send + 'static) -> Work {
    Work::Ordered(Box::pin(command))
}

/// A command of the ordered lane that runs to its end at once.
fn at_once(command: impl FnOnce() + Send + 'static) -> Work {
    ordered(async move { command() })
}

/// A command that blocks its thread while a dialog is open, so it must run on a
/// thread that is allowed to block (the dispatcher's blocking lane).
fn blocking(command: impl FnOnce() + Send + 'static) -> Work {
    Work::Blocking(Box::new(command))
}

impl Command {
    /// The handler of the command, bound to its payload, on the lane it belongs to.
    ///
    /// This is the one place that says what a command runs and where. A command that
    /// is added to [`Command`] does not compile until it is listed here.
    ///
    /// Most commands are synchronous: they mutate the state, possibly launch a
    /// background task (scan, lazy load, generation, file preview) and return.
    /// Whatever such a command means for the state (the scan is registered, the
    /// directory is expanded, the file is the previewed one) is done by the command
    /// itself, not by the task, so that the next command finds it done. The commands
    /// that are awaited can suspend while they wait for a content search
    /// (`updateFilters`, and a change of the case sensitivity while a content search
    /// is active); the dispatcher then completes them on its continuation lane.
    fn into_work<P: EventProxy, D: DialogService + 'static>(
        self,
        dialog: Arc<D>,
        proxy: P,
        state: SharedState,
    ) -> Work {
        use commands as c;
        match self {
            // --- Commands that wait for a native dialog ---
            Self::SelectDirectory => {
                blocking(move || c::select_directory(dialog.as_ref(), proxy, state))
            }
            Self::SaveFile(payload) => {
                blocking(move || c::save_file(dialog.as_ref(), payload, proxy, state))
            }
            Self::PickOutputDirectory => {
                blocking(move || c::pick_output_directory(dialog.as_ref(), proxy, state))
            }
            Self::ImportConfig => blocking(move || c::import_config(dialog.as_ref(), proxy, state)),
            Self::ExportConfig => blocking(move || c::export_config(dialog.as_ref(), proxy, state)),

            // --- Commands that can suspend ---
            Self::UpdateFilters(filters) => ordered(c::update_filters(filters, proxy, state)),
            Self::AddIgnorePattern(pattern) => {
                ordered(c::add_ignore_pattern(pattern, proxy, state))
            }
            Self::RemoveIgnorePattern(pattern) => {
                ordered(c::remove_ignore_pattern(pattern, proxy, state))
            }
            Self::ClearIgnorePatterns => ordered(c::clear_ignore_patterns(proxy, state)),
            Self::AddIgnorePath(path) => ordered(c::add_ignore_path(path, proxy, state)),
            Self::SetConfigOption(option) => ordered(c::set_config_option(option, proxy, state)),

            // --- Synchronous commands & task launchers ---
            Self::Initialize => at_once(move || c::initialize(proxy, state)),
            Self::RescanDirectory => at_once(move || c::rescan_directory(proxy, state)),
            Self::ClearDirectory => at_once(move || c::clear_directory(proxy, state)),
            Self::CancelScan => at_once(move || c::cancel_scan(proxy, state)),
            Self::LoadDirectoryLevel(path) => {
                at_once(move || c::load_directory_level(path, proxy, state))
            }
            Self::SetExpansion(payload) => at_once(move || c::set_expansion(payload, proxy, state)),
            Self::ExpandCollapseAll(expand) => {
                at_once(move || c::expand_collapse_all(expand, proxy, state))
            }
            Self::SetViewport(viewport) => at_once(move || c::set_viewport(viewport, proxy, state)),
            Self::LocateRow(lookup) => at_once(move || c::locate_row(lookup, proxy, state)),
            Self::ToggleSelection(path) => at_once(move || c::toggle_selection(path, proxy, state)),
            Self::ToggleDirectorySelection(path) => {
                at_once(move || c::toggle_directory_selection(path, proxy, state))
            }
            Self::SelectAll => at_once(move || c::select_all(proxy, state)),
            Self::DeselectAll => at_once(move || c::deselect_all(proxy, state)),
            Self::LoadFilePreview(path) => {
                at_once(move || c::load_file_preview(path, proxy, state))
            }
            Self::ClearPreviewState => at_once(move || c::clear_preview_state(proxy, state)),
            Self::GeneratePreview => at_once(move || c::generate_preview(proxy, state)),
            Self::CancelGeneration => at_once(move || c::cancel_generation(proxy, state)),
        }
    }
}

/// Processes events sent from the backend to the UI thread.
///
/// It translates each `UserEvent` into a JavaScript call in the WebView to update the UI.
/// Events reach this function via [`transport::EventOutbox::next_event`], which has
/// already dropped superseded `StateUpdate`s, so no stale snapshot is serialized here.
///
/// An event that cannot be serialized is logged and skipped: a call without its
/// argument (`window.render();`) would only make the UI report an invalid payload.
pub fn handle_user_event(event: UserEvent, webview: &WebView) {
    let script = match script_for(&event) {
        Ok(script) => script,
        Err(e) => {
            tracing::error!(
                "Failed to serialize a {} event for the UI: {e}. The event is skipped.",
                event_name(&event)
            );
            return;
        }
    };
    if let Err(e) = webview.evaluate_script(&script) {
        tracing::error!("Failed to evaluate script: {}", e);
    }
}

/// The `window.*` call that delivers `event` to the UI.
fn script_for(event: &UserEvent) -> serde_json::Result<String> {
    use serde_json::to_string as json;
    Ok(match event {
        UserEvent::StateUpdate(state) => format!("window.render({});", json(state)?),
        UserEvent::ShowFilePreview {
            content,
            language,
            search_term,
            path,
        } => format!(
            "window.showPreviewContent({}, {}, {}, {});",
            json(content)?,
            json(language)?,
            json(search_term)?,
            // Lossy like the paths of the tree, see `view_model::lossy_path`.
            json(&path.to_string_lossy())?,
        ),
        UserEvent::ShowGeneratedContent {
            document,
            token_count,
        } => generated_content_script(document.text(), *token_count, document.id())?,
        UserEvent::OutputSize(notice) => {
            format!("window.outputSizeNotice({});", json(notice)?)
        }
        UserEvent::IgnorePathRefused(refusal) => {
            format!("window.ignorePathRefused({});", json(refusal)?)
        }
        UserEvent::ShowError(msg) => format!("window.showError({});", json(msg)?),
        UserEvent::SaveResult(result) => format!("window.saveResult({});", json(result)?),
        UserEvent::ConfigExported(success) => format!(
            "window.showStatus('{}');",
            if *success {
                "Config exported successfully."
            } else {
                "Failed to export config."
            }
        ),
        UserEvent::ScanProgress(progress) => {
            format!("window.updateScanProgress({});", json(progress)?)
        }
        UserEvent::DragStateChanged(is_dragging) => {
            format!("window.setDragState({is_dragging});")
        }
    })
}

/// The call that shows a generated document.
///
/// A document can weigh many megabytes, so the script is written into one buffer of
/// about its final size: the text is escaped straight into it and exists once more,
/// not three times (as a JSON string, as the formatted script and while that grows).
fn generated_content_script(
    text: &str,
    token_count: usize,
    document_id: u64,
) -> serde_json::Result<String> {
    use std::io::Write;

    // Room for the escapes of a typical text: a line end takes two bytes instead of one.
    let mut script = Vec::with_capacity(text.len() + text.len() / 8 + 96);
    script.extend_from_slice(b"window.showGeneratedContent(");
    serde_json::to_writer(&mut script, text)?;
    // Writing into a `Vec` cannot fail.
    let _ = write!(script, ", {token_count}, {document_id});");
    // JSON is UTF-8, and so is everything else that was written.
    String::from_utf8(script).map_err(serde::ser::Error::custom)
}

fn event_name(event: &UserEvent) -> &'static str {
    match event {
        UserEvent::StateUpdate(_) => "StateUpdate",
        UserEvent::ShowFilePreview { .. } => "ShowFilePreview",
        UserEvent::ShowGeneratedContent { .. } => "ShowGeneratedContent",
        UserEvent::IgnorePathRefused(_) => "IgnorePathRefused",
        UserEvent::ShowError(_) => "ShowError",
        UserEvent::OutputSize(_) => "OutputSize",
        UserEvent::SaveResult(_) => "SaveResult",
        UserEvent::ConfigExported(_) => "ConfigExported",
        UserEvent::ScanProgress(_) => "ScanProgress",
        UserEvent::DragStateChanged(_) => "DragStateChanged",
    }
}

#[cfg(test)]
mod tests;
