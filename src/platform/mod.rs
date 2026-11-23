//! Platform-specific integration helpers.
//!
//! Keep OS quirks here to avoid leaking them into the app's core logic.

#[cfg(target_os = "macos")]
pub mod macos;

// The origin of the UI and the gate for IPC messages apply to every build.
pub mod ipc;
pub mod origin;

// These policies are only wired up for the embedded UI (release builds); the dev
// build loads the Vite server without them. Tests always compile them.
#[cfg(any(test, not(debug_assertions)))]
pub mod asset_protocol;
#[cfg(any(test, not(debug_assertions)))]
pub mod csp;
#[cfg(any(test, not(debug_assertions)))]
pub mod navigation;

/// The raw handles native dialogs are to be attached to, taken on the main thread
/// once the web view has been built (on macOS the web view replaces the window's
/// original content view, see [`macos::dialog_parent`]).
pub fn dialog_parent_handles(
    window: &tao::window::Window,
) -> Option<(tao::rwh_06::RawWindowHandle, tao::rwh_06::RawDisplayHandle)> {
    use tao::rwh_06::HasDisplayHandle;
    #[cfg(not(target_os = "macos"))]
    use tao::rwh_06::HasWindowHandle;
    let display = window.display_handle().ok()?.as_raw();
    #[cfg(target_os = "macos")]
    let window = macos::dialog_parent::content_view_handle(window)?;
    #[cfg(not(target_os = "macos"))]
    let window = window.window_handle().ok()?.as_raw();
    Some((window, display))
}
