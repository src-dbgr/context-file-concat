//! Defines the custom error type for the `core` module.

use std::path::{PathBuf, StripPrefixError};
use thiserror::Error;

/// The primary error type for the `core` module.
///
/// This enum encapsulates all possible errors that can occur during
/// core operations like file scanning, content generation, and pattern matching.
#[derive(Debug, Error, Clone)]
pub enum CoreError {
    /// Represents an I/O error, typically from file system operations.
    #[error("I/O error for path {1}: {0}")]
    Io(String, PathBuf),

    /// Represents an error that occurred when a Tokio task was joined.
    #[error("Task join error: {0}")]
    Join(String),

    /// Represents a failure to strip a path prefix.
    #[error("Failed to strip prefix from path: {0}")]
    PathStrip(String),

    /// Represents a user-initiated cancellation of an operation.
    #[error("Operation was cancelled by the user")]
    Cancelled,

    /// The output document would be larger than the caller allows (`limit` bytes).
    #[error(
        "The output would be larger than {} MiB. Select fewer files.",
        .limit / (1024 * 1024)
    )]
    OutputTooLarge { limit: u64 },
}

// Manual From implementations because the source errors are not Clone
impl From<tokio::task::JoinError> for CoreError {
    fn from(e: tokio::task::JoinError) -> Self {
        CoreError::Join(e.to_string())
    }
}

impl From<StripPrefixError> for CoreError {
    fn from(e: StripPrefixError) -> Self {
        CoreError::PathStrip(e.to_string())
    }
}
