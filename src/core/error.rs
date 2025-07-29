//! Defines the custom error type for the `core` module.

use std::path::{PathBuf, StripPrefixError};
use thiserror::Error;

/// The primary error type for the `core` module.
///
/// This enum encapsulates all possible errors that can occur during
/// core operations like file scanning, content generation, and pattern matching.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Represents an I/O error, typically from file system operations.
    #[error("I/O error for path {1}: {0}")]
    Io(#[source] std::io::Error, PathBuf),

    /// Represents an error that occurred when a Tokio task was joined.
    /// This is often due to a task panicking or being cancelled.
    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    /// Represents an error during the parsing or building of a glob pattern.
    #[error("Invalid glob pattern: {0}")]
    GlobPattern(#[from] globset::Error),

    /// Represents a path that was expected to be a directory but was not.
    #[allow(dead_code)]
    #[error("Path is not a valid directory: {0}")]
    NotADirectory(PathBuf),

    /// Represents a failure to strip a path prefix.
    #[error("Failed to strip prefix from path: {0}")]
    PathStrip(#[from] StripPrefixError),

    /// Represents a user-initiated cancellation of an operation.
    #[error("Operation was cancelled by the user")]
    Cancelled,
}
