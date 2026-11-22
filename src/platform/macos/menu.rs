//! The application's main menu (NSMenu).
//!
//! The WebView only receives the standard editing shortcuts (Cmd+C/V/X/A/Z, ...) if a
//! menu item with the matching key equivalent and first-responder action exists.

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{sel, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::NSString;

const CMD: NSEventModifierFlags = NSEventModifierFlags::Command;

/// Key equivalent of a menu item: the key and its modifier mask.
type Shortcut = (&'static str, NSEventModifierFlags);

/// Appends an item to `menu`.
///
/// `action` is sent along the responder chain (the items have no target), so it must be
/// a standard AppKit action taking the sender as its only argument.
fn add_item(
    mtm: MainThreadMarker,
    menu: &NSMenu,
    title: &str,
    action: Option<Sel>,
    shortcut: Option<Shortcut>,
) -> Retained<NSMenuItem> {
    let (key, mask) = match shortcut {
        Some((key, mask)) => (key, Some(mask)),
        None => ("", None),
    };
    // SAFETY: `action` is either `None` or one of the `-(void)action:(id)sender`
    // selectors passed by the builders below, which is the signature AppKit expects.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc(),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    if let Some(mask) = mask {
        item.setKeyEquivalentModifierMask(mask);
    }
    menu.addItem(&item);
    item
}

fn add_separator(mtm: MainThreadMarker, menu: &NSMenu) {
    menu.addItem(&NSMenuItem::separatorItem(mtm));
}

/// Appends an empty, titled submenu to the menubar and returns it.
fn add_submenu(mtm: MainThreadMarker, menubar: &NSMenu, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(title));
    let item = NSMenuItem::new(mtm);
    item.setSubmenu(Some(&menu));
    menubar.addItem(&item);
    menu
}

// Builds the application menu (About, Services, Hide, Quit).
fn build_app_menu(mtm: MainThreadMarker, app: &NSApplication, menubar: &NSMenu, app_name: &str) {
    // The first menubar item is the application menu; its title is set by the system.
    let app_menu_item = NSMenuItem::new(mtm);
    menubar.addItem(&app_menu_item);

    let app_menu = NSMenu::new(mtm);
    app_menu_item.setSubmenu(Some(&app_menu));

    add_item(
        mtm,
        &app_menu,
        &format!("About {app_name}"),
        Some(sel!(orderFrontStandardAboutPanel:)),
        None,
    );

    add_separator(mtm, &app_menu);

    // Services: the item has no action of its own, it only owns the submenu that macOS
    // populates.
    let services_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Services"));
    let services_item = add_item(mtm, &app_menu, "Services", None, None);
    services_item.setSubmenu(Some(&services_menu));
    app.setServicesMenu(Some(&services_menu));

    add_separator(mtm, &app_menu);

    add_item(
        mtm,
        &app_menu,
        &format!("Hide {app_name}"),
        Some(sel!(hide:)),
        Some(("h", CMD)),
    );
    add_item(
        mtm,
        &app_menu,
        "Hide Others",
        Some(sel!(hideOtherApplications:)),
        Some(("h", CMD.union(NSEventModifierFlags::Option))),
    );
    add_item(
        mtm,
        &app_menu,
        "Show All",
        Some(sel!(unhideAllApplications:)),
        None,
    );

    add_separator(mtm, &app_menu);

    add_item(
        mtm,
        &app_menu,
        &format!("Quit {app_name}"),
        Some(sel!(terminate:)),
        Some(("q", CMD)),
    );
}

// Builds the "Edit" menu.
fn build_edit_menu(mtm: MainThreadMarker, menubar: &NSMenu) {
    let menu = add_submenu(mtm, menubar, "Edit");

    add_item(mtm, &menu, "Undo", Some(sel!(undo:)), Some(("z", CMD)));
    add_item(
        mtm,
        &menu,
        "Redo",
        Some(sel!(redo:)),
        Some(("Z", CMD.union(NSEventModifierFlags::Shift))),
    );
    add_separator(mtm, &menu);

    add_item(mtm, &menu, "Cut", Some(sel!(cut:)), Some(("x", CMD)));
    add_item(mtm, &menu, "Copy", Some(sel!(copy:)), Some(("c", CMD)));
    add_item(mtm, &menu, "Paste", Some(sel!(paste:)), Some(("v", CMD)));
    add_item(mtm, &menu, "Delete", Some(sel!(delete:)), None);
    add_item(
        mtm,
        &menu,
        "Select All",
        Some(sel!(selectAll:)),
        Some(("a", CMD)),
    );

    add_separator(mtm, &menu);
    add_item(
        mtm,
        &menu,
        "Start Dictation…",
        Some(sel!(startDictation:)),
        None,
    );
    add_item(
        mtm,
        &menu,
        "Emoji & Symbols",
        Some(sel!(orderFrontCharacterPalette:)),
        None,
    );
}

// Builds the "View" menu (fullscreen only).
fn build_view_menu(mtm: MainThreadMarker, menubar: &NSMenu) {
    let menu = add_submenu(mtm, menubar, "View");

    add_item(
        mtm,
        &menu,
        "Enter Full Screen",
        Some(sel!(toggleFullScreen:)),
        Some(("f", CMD.union(NSEventModifierFlags::Control))),
    );
}

// Builds the "Window" menu (managed by macOS).
fn build_window_menu(mtm: MainThreadMarker, app: &NSApplication, menubar: &NSMenu) {
    let menu = add_submenu(mtm, menubar, "Window");

    add_item(
        mtm,
        &menu,
        "Minimize",
        Some(sel!(performMiniaturize:)),
        Some(("m", CMD)),
    );
    add_item(mtm, &menu, "Zoom", Some(sel!(performZoom:)), None);
    add_separator(mtm, &menu);
    add_item(
        mtm,
        &menu,
        "Bring All to Front",
        Some(sel!(arrangeInFront:)),
        None,
    );

    app.setWindowsMenu(Some(&menu));
}

// Builds the "Help" menu and registers it, so macOS adds its search field.
fn build_help_menu(mtm: MainThreadMarker, app: &NSApplication, menubar: &NSMenu, app_name: &str) {
    let menu = add_submenu(mtm, menubar, "Help");

    add_item(
        mtm,
        &menu,
        &format!("{app_name} Help"),
        Some(sel!(showHelp:)),
        None,
    );

    app.setHelpMenu(Some(&menu));
}

/// Installs a standard set of menus (App, Edit, View, Window, Help).
///
/// Must be called on the main thread; otherwise nothing is installed and an error is
/// logged.
pub fn install_standard_menus(app_name: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        tracing::error!("The macOS menu can only be installed from the main thread.");
        return;
    };
    let app = NSApplication::sharedApplication(mtm);

    let menubar = NSMenu::new(mtm);
    app.setMainMenu(Some(&menubar));

    build_app_menu(mtm, &app, &menubar, app_name);
    build_edit_menu(mtm, &menubar);
    build_view_menu(mtm, &menubar);
    build_window_menu(mtm, &app, &menubar);
    build_help_menu(mtm, &app, &menubar, app_name);

    menubar.setAutoenablesItems(true);
}
