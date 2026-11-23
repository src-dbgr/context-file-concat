//! Platform-specific integration helpers.
//!
//! Keep OS quirks here to avoid leaking them into the app's core logic.

#[cfg(target_os = "macos")]
pub mod macos;

// The origin of the UI and the check of where an IPC message comes from apply to
// every build.
pub mod origin;

// These policies are only wired up for the embedded UI (release builds); the dev
// build loads the Vite server without them. Tests always compile them.
#[cfg(any(test, not(debug_assertions)))]
pub mod asset_protocol;
#[cfg(any(test, not(debug_assertions)))]
pub mod csp;
#[cfg(any(test, not(debug_assertions)))]
pub mod navigation;
