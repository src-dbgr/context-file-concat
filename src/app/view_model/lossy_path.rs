//! Serialization of paths for the UI.
//!
//! Serde refuses to serialize a path that is not valid Unicode, and one such file name
//! in a tree would make the whole snapshot unserializable: the UI would get nothing at
//! all. Paths that go to the UI are therefore written lossily (invalid sequences become
//! U+FFFD). For every valid path that is exactly what serde would have written.
//!
//! The price is paid by the odd file only: its row is shown, but its path does not
//! survive the round trip, so commands the UI sends for it name a path the host does
//! not know and have no effect.

use serde::Serializer;
use std::path::{Path, PathBuf};

/// For `#[serde(serialize_with = "lossy_path::serialize")]` on a `PathBuf`.
pub(crate) fn serialize<S: Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// For `#[serde(serialize_with = "lossy_path::serialize_option")]` on an
/// `Option<PathBuf>`.
pub(crate) fn serialize_option<S: Serializer>(
    path: &Option<PathBuf>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match path {
        Some(path) => serializer.serialize_some(&*path.to_string_lossy()),
        None => serializer.serialize_none(),
    }
}
