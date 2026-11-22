//! Platform-specific integration helpers.
//!
//! Keep OS quirks here to avoid leaking them into the app's core logic.

#[cfg(target_os = "macos")]
pub mod macos;

// The navigation policy is only wired up for the embedded UI (release builds); the
// dev build loads the Vite server without one. Tests always compile it.
#[cfg(any(test, not(debug_assertions)))]
pub mod navigation;
