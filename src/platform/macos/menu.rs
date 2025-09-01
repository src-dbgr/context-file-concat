#![allow(deprecated)] // Keep Cocoa warnings localized
#![allow(unexpected_cfgs)] // Suppress warnings from the `sel!` macro in older `objc` crates

use cocoa::appkit::{NSApp, NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use cocoa::base::{id, nil, YES}; // FIX: Removed unused `NO` import
use cocoa::foundation::NSString;
use objc::runtime::Sel;
use objc::{sel, sel_impl};

fn ns(s: &str) -> id {
    unsafe { NSString::alloc(nil).init_str(s) }
}

// Helper function to create a menu item with an optional shortcut.
unsafe fn add_item(
    menu: id,
    title: &str,
    action: Sel,
    key: Option<&str>,
    mask: Option<NSEventModifierFlags>,
) -> id {
    // Note: The method name in this version of the cocoa crate includes a trailing underscore.
    let item = NSMenuItem::alloc(nil).initWithTitle_action_keyEquivalent_(
        ns(title),
        action,
        ns(key.unwrap_or("")),
    );
    if let Some(m) = mask {
        item.setKeyEquivalentModifierMask_(m);
    }
    menu.addItem_(item);
    item
}

unsafe fn add_separator(menu: id) {
    let sep = NSMenuItem::separatorItem(nil);
    menu.addItem_(sep);
}

// Builds the main application menu (About, Services, Hide, Quit) and returns the menubar.
unsafe fn build_app_menu(app_name: &str) -> id {
    // Install the menubar
    let menubar = NSMenu::new(nil);
    NSApp().setMainMenu_(menubar);

    // Create the first menu item (Application) - title is set by the system.
    let app_menu_item = NSMenuItem::new(nil);
    menubar.addItem_(app_menu_item);

    let app_menu = NSMenu::new(nil);
    app_menu_item.setSubmenu_(app_menu);

    // About
    add_item(
        app_menu,
        &format!("About {}", app_name),
        sel!(orderFrontStandardAboutPanel:),
        None,
        None,
    );

    add_separator(app_menu);

    // Services (placeholder for macOS to populate)
    let services_menu = NSMenu::alloc(nil).initWithTitle_(ns("Services"));
    let services_item = add_item(app_menu, "Services", sel!(performMiniaturize:), None, None);
    services_item.setSubmenu_(services_menu);
    NSApp().setServicesMenu_(services_menu);

    add_separator(app_menu);

    // Hide / Hide Others / Show All
    add_item(
        app_menu,
        &format!("Hide {}", app_name),
        sel!(hide:),
        Some("h"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(
        app_menu,
        "Hide Others",
        sel!(hideOtherApplications:),
        Some("h"),
        Some(NSEventModifierFlags::NSCommandKeyMask | NSEventModifierFlags::NSAlternateKeyMask),
    );
    add_item(
        app_menu,
        "Show All",
        sel!(unhideAllApplications:),
        None,
        None,
    );

    add_separator(app_menu);

    // Quit
    add_item(
        app_menu,
        &format!("Quit {}", app_name),
        sel!(terminate:),
        Some("q"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );

    menubar
}

// Builds the "Edit" menu.
unsafe fn build_edit_menu(menubar: id) {
    let edit_menu = NSMenu::alloc(nil).initWithTitle_(ns("Edit"));
    let edit_item = NSMenuItem::new(nil);
    edit_item.setSubmenu_(edit_menu);
    menubar.addItem_(edit_item);

    add_item(
        edit_menu,
        "Undo",
        sel!(undo:),
        Some("z"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(
        edit_menu,
        "Redo",
        sel!(redo:),
        Some("Z"),
        Some(NSEventModifierFlags::NSCommandKeyMask | NSEventModifierFlags::NSShiftKeyMask),
    );
    add_separator(edit_menu);

    add_item(
        edit_menu,
        "Cut",
        sel!(cut:),
        Some("x"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(
        edit_menu,
        "Copy",
        sel!(copy:),
        Some("c"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(
        edit_menu,
        "Paste",
        sel!(paste:),
        Some("v"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(edit_menu, "Delete", sel!(delete:), None, None);
    add_item(
        edit_menu,
        "Select All",
        sel!(selectAll:),
        Some("a"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );

    add_separator(edit_menu);
    add_item(
        edit_menu,
        "Start Dictation…",
        sel!(startDictation:),
        None,
        None,
    );
    add_item(
        edit_menu,
        "Emoji & Symbols",
        sel!(orderFrontCharacterPalette:),
        None,
        None,
    );
}

// Builds the "View" menu (fullscreen only).
unsafe fn build_view_menu(menubar: id) {
    let view_menu = NSMenu::alloc(nil).initWithTitle_(ns("View"));
    let view_item = NSMenuItem::new(nil);
    view_item.setSubmenu_(view_menu);
    menubar.addItem_(view_item);

    add_item(
        view_menu,
        "Enter Full Screen",
        sel!(toggleFullScreen:),
        Some("f"),
        Some(NSEventModifierFlags::NSCommandKeyMask | NSEventModifierFlags::NSControlKeyMask),
    );
}

// Builds the "Window" menu (managed by macOS).
unsafe fn build_window_menu(menubar: id) {
    let window_menu = NSMenu::alloc(nil).initWithTitle_(ns("Window"));
    let window_item = NSMenuItem::new(nil);
    window_item.setSubmenu_(window_menu);
    menubar.addItem_(window_item);

    add_item(
        window_menu,
        "Minimize",
        sel!(performMiniaturize:),
        Some("m"),
        Some(NSEventModifierFlags::NSCommandKeyMask),
    );
    add_item(window_menu, "Zoom", sel!(performZoom:), None, None);
    add_separator(window_menu);
    add_item(
        window_menu,
        "Bring All to Front",
        sel!(arrangeInFront:),
        None,
        None,
    );

    NSApp().setWindowsMenu_(window_menu);
}

// Builds the "Help" menu (placeholder).
unsafe fn build_help_menu(menubar: id, app_name: &str) {
    let help_menu = NSMenu::alloc(nil).initWithTitle_(ns("Help"));
    let help_item = NSMenuItem::new(nil);
    help_item.setSubmenu_(help_menu);
    menubar.addItem_(help_item);

    add_item(
        help_menu,
        &format!("{} Help", app_name),
        sel!(showHelp:),
        None,
        None,
    );
}

/// Installs a standard set of menus (App, Edit, View, Window, Help).
/// Recommended for a user-friendly application.
pub fn install_standard_menus(app_name: &str) {
    unsafe {
        let menubar = build_app_menu(app_name);
        build_edit_menu(menubar);
        build_view_menu(menubar);
        build_window_menu(menubar);
        build_help_menu(menubar, app_name);

        menubar.setAutoenablesItems(YES);
    }
}
