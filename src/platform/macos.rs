//! macOS-specific helpers.

use cocoa::appkit::{NSApp, NSApplication, NSMenu};
use cocoa::base::{id, nil};

/// Ensure that a main menu exists before creating the WebView.
///
/// wry (0.37) installs a parent NSView that forwards `keyDown:` to
/// `NSApp.mainMenu.performKeyEquivalent(_)`. If `mainMenu` is `nil`,
/// WebKit gets a null deref. Installing an *empty* menu is sufficient.
///
/// This is intentionally minimal and side-effect free; your UI keeps
/// handling shortcuts, but the crash path disappears.
pub fn ensure_main_menu() {
    unsafe {
        let app = NSApp();
        let current: id = app.mainMenu();
        if current == nil {
            let menubar: id = NSMenu::new(nil);
            // No submenus required for safety; presence is enough.
            app.setMainMenu_(menubar);
        }
    }
}
