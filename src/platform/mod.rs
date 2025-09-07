//! Platform-specific integration helpers.
//!
//! Keep OS quirks here to avoid leaking them into the app's core logic.

#[cfg(target_os = "macos")]
pub mod macos;
