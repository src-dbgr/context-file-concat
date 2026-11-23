//! The view native dialogs are attached to.
//!
//! wry installs its web view as the content view of the window. The view tao hands
//! out as the window's handle is the one it created, which is no longer inside the
//! window afterwards; rfd looks up the window of the view it is given and aborts the
//! process when there is none. The dialogs are therefore attached to the window's
//! current content view, taken after the web view has been built.

use std::ptr::NonNull;

use objc2::MainThreadMarker;
use objc2_app_kit::NSWindow;
use tao::platform::macos::WindowExtMacOS;
use tao::rwh_06::{AppKitWindowHandle, RawWindowHandle};

/// The handle of the content view of `window`, or `None` off the main thread or for a
/// window without content view.
pub fn content_view_handle(window: &tao::window::Window) -> Option<RawWindowHandle> {
    MainThreadMarker::new()?;
    let ns_window = NonNull::new(window.ns_window().cast::<NSWindow>())?;
    // SAFETY: tao returns the pointer of the live NSWindow it owns; it is only used
    // here, on the main thread, while `window` is borrowed.
    let ns_window = unsafe { ns_window.as_ref() };
    let view = ns_window.contentView()?;
    // The window keeps its content view alive for as long as it shows it, which is
    // the lifetime of the application window.
    let view = NonNull::from(&*view).cast();
    Some(RawWindowHandle::AppKit(AppKitWindowHandle::new(view)))
}
