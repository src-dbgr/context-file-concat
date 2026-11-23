//! Tests of the application state, grouped by topic in the files beside this one.

use super::*;
use crate::core::FileItem;
use std::sync::atomic::Ordering;
use std::time::Duration;

mod configuration;
mod content_search;
mod directory;
mod expansion;
mod file_list;
mod generation;
mod lazy_load;
mod preview;
mod scan;

/// Creates a dummy tokio JoinHandle for testing purposes.
fn create_dummy_task() -> JoinHandle<()> {
    tokio::spawn(async {
        // The task can sleep for a bit to simulate work,
        // though it's not strictly necessary as we abort it immediately.
        tokio::time::sleep(Duration::from_millis(50)).await;
    })
}

/// Creates a simple FileItem for testing.
fn create_test_file_item(path_str: &str, is_dir: bool) -> FileItem {
    FileItem {
        path: PathBuf::from(path_str),
        is_directory: is_dir,
        ..Default::default()
    }
}
