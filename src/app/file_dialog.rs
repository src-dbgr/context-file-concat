//! An abstraction layer for native file dialogs to enable testing.

use crate::config::AppConfig;
use std::path::PathBuf;
use std::sync::OnceLock;
use tao::rwh_06::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};

/// Defines a common interface for file and folder selection dialogs.
/// This allows for a mock implementation during tests, avoiding the need
/// to interact with actual OS dialog windows.
pub trait DialogService: Send + Sync {
    /// Opens a dialog to select a single directory.
    fn pick_directory(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a single file for config import.
    fn pick_config_to_import(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a save location for a config export.
    fn export_config_path(&self) -> Option<PathBuf>;

    /// Opens a dialog to select a save location for the final output file.
    /// It uses the provided config to suggest a default name and directory.
    fn save_output_file_path(&self, config: &AppConfig) -> Option<PathBuf>;
}

/// The window the native dialogs belong to, as raw handles.
///
/// The handles are taken once, by the thread that owns the window: on macOS a window
/// only hands out its handle on the main thread, while the dialogs are requested from
/// worker threads.
#[derive(Clone, Copy)]
struct ParentWindow {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// SAFETY: the handles are plain identifiers of a window (a pointer or an id that is
// never dereferenced here). They are only passed on to the dialog backend, which uses
// them on the thread the platform requires: AppKit dialogs are run on the main
// thread, and a Win32 window handle may name the owner of a dialog from any thread.
unsafe impl Send for ParentWindow {}
unsafe impl Sync for ParentWindow {}

impl ParentWindow {
    /// The handles of `window`, or `None` if it cannot provide them (e.g. when asked
    /// on the wrong thread).
    fn of(window: &(impl HasWindowHandle + HasDisplayHandle)) -> Option<Self> {
        Some(Self {
            window: window.window_handle().ok()?.as_raw(),
            display: window.display_handle().ok()?.as_raw(),
        })
    }
}

impl HasWindowHandle for ParentWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: see `NativeDialogService::parented_to`: the window outlives the
        // service, so the handle is valid whenever a dialog is opened.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for ParentWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: as for the window handle.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

/// The production implementation that uses the `rfd` crate to show native OS dialogs.
///
/// The dialogs are children of the application window once the service has been
/// given its handles ([`NativeDialogService::set_parent`]): they stay in front of it
/// and block it while they are open, instead of being windows of their own that can
/// end up behind it. Until then, and for a [`Default`] service, they are free-standing.
///
/// The parent is set after construction because the right handle only exists once
/// the web view has been built (see `platform::dialog_parent_handles`), while the
/// service is already needed to build the IPC handler of that web view.
#[derive(Default)]
pub struct NativeDialogService {
    parent: OnceLock<ParentWindow>,
}

impl NativeDialogService {
    /// A service whose dialogs belong to `window`.
    ///
    /// To be called on the thread that owns the window (the main thread). The window
    /// must stay open for as long as the service is used; the application window
    /// lives until the process ends. If the window does not hand out its handles, the
    /// dialogs are free-standing.
    pub fn parented_to(window: &(impl HasWindowHandle + HasDisplayHandle)) -> Self {
        let service = Self::default();
        match ParentWindow::of(window) {
            Some(parent) => service.adopt(parent),
            None => tracing::warn!("No window handle available; dialogs will not be parented."),
        }
        service
    }

    /// Attaches the dialogs to the window named by these handles, taken on the thread
    /// that owns the window. The window must stay open for as long as the service is
    /// used. Only the first parent counts.
    pub fn set_parent(&self, window: RawWindowHandle, display: RawDisplayHandle) {
        self.adopt(ParentWindow { window, display });
    }

    fn adopt(&self, parent: ParentWindow) {
        if self.parent.set(parent).is_err() {
            tracing::warn!("The dialogs already have a parent window; keeping it.");
        }
    }

    /// A dialog that belongs to the parent window, if there is one.
    fn dialog(&self) -> rfd::FileDialog {
        let dialog = rfd::FileDialog::new();
        match self.parent.get() {
            Some(parent) => dialog.set_parent(parent),
            None => dialog,
        }
    }
}

impl DialogService for NativeDialogService {
    fn pick_directory(&self) -> Option<PathBuf> {
        self.dialog().pick_folder()
    }

    fn pick_config_to_import(&self) -> Option<PathBuf> {
        self.dialog().add_filter("JSON", &["json"]).pick_file()
    }

    fn export_config_path(&self) -> Option<PathBuf> {
        self.dialog()
            .add_filter("JSON", &["json"])
            .set_file_name("cfc-config.json")
            .save_file()
    }

    fn save_output_file_path(&self, config: &AppConfig) -> Option<PathBuf> {
        let mut dialog = self
            .dialog()
            .add_filter("Text File", &["txt"])
            .set_file_name(&config.output_filename);
        if let Some(dir) = &config.output_directory {
            dialog = dialog.set_directory(dir);
        }
        dialog.save_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window that hands out fixed handles, or none (as a window does when asked on
    /// the wrong thread).
    struct FakeWindow {
        available: bool,
    }

    impl HasWindowHandle for FakeWindow {
        fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
            if !self.available {
                return Err(HandleError::Unavailable);
            }
            let raw = RawWindowHandle::Web(tao::rwh_06::WebWindowHandle::new(7));
            // SAFETY: a web handle is a plain id; nothing is dereferenced.
            Ok(unsafe { WindowHandle::borrow_raw(raw) })
        }
    }

    impl HasDisplayHandle for FakeWindow {
        fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
            Ok(DisplayHandle::web())
        }
    }

    #[test]
    fn service_keeps_the_handles_of_its_window() {
        let service = NativeDialogService::parented_to(&FakeWindow { available: true });

        let parent = service.parent.get().expect("parent");
        assert_eq!(
            parent.window_handle().unwrap().as_raw(),
            RawWindowHandle::Web(tao::rwh_06::WebWindowHandle::new(7))
        );
        assert_eq!(
            parent.display_handle().unwrap().as_raw(),
            DisplayHandle::web().as_raw()
        );
    }

    #[test]
    fn service_without_a_window_handle_shows_free_standing_dialogs() {
        let service = NativeDialogService::parented_to(&FakeWindow { available: false });
        assert!(service.parent.get().is_none());
        assert!(NativeDialogService::default().parent.get().is_none());
    }

    #[test]
    fn a_parent_set_later_is_used_and_the_first_one_wins() {
        let service = NativeDialogService::default();
        let first = RawWindowHandle::Web(tao::rwh_06::WebWindowHandle::new(1));
        let second = RawWindowHandle::Web(tao::rwh_06::WebWindowHandle::new(2));

        service.set_parent(first, DisplayHandle::web().as_raw());
        service.set_parent(second, DisplayHandle::web().as_raw());

        let parent = service.parent.get().expect("parent");
        assert_eq!(parent.window_handle().unwrap().as_raw(), first);
    }

    /// The service is shared with the worker threads the dialogs are requested from.
    #[test]
    fn service_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NativeDialogService>();
    }
}
