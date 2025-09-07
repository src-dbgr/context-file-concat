//! macOS-specific helpers.
//!
//! Cocoa is marked deprecated in favor of objc2 in newer ecosystems, but
//! wry/tao still integrate via Cocoa under the hood. We keep this usage

//! strictly scoped to this module to avoid leaking deprecations elsewhere.

#![allow(deprecated)] // Silence Cocoa deprecation warnings in this isolated module.

// The function `ensure_main_menu` was here, but it was unused (`dead_code` warning).
// It has been removed as the application directly calls `install_standard_menus`.

/// Public menu builder.
pub mod menu;
