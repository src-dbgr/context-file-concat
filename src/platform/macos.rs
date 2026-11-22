//! macOS-specific helpers.
//!
//! AppKit is accessed through the `objc2` crates, the same bindings tao and wry use.
//! Keep that usage scoped to this module.

/// Public menu builder.
pub mod menu;
