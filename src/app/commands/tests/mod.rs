//! Tests of the command handlers. The harness is here; the tests are grouped by topic
//! in the files beside it.

use super::*;
use crate::app::file_dialog::DialogService;
use crate::app::state::AppState;
use crate::app::test_support::{
    config_on_disk, file_item, TestEventProxy, TestHarness, QUIET_PERIOD,
};
use crate::app::view_model::UiState;
use crate::config::ConfigStore;
use crate::core::FileItem;
use crate::utils::test_helpers::setup_test_logging;
use serde_json::json;
use std::fs as std_fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

mod configuration;
mod directory;
mod filters;
mod generation;
mod ignore_path;
mod preview;
mod tree;

/// The payload of a command, read the way a message of the UI is: as the type
/// the command carries. A test of a handler starts where the handler does, with
/// a payload that fits; what does not fit is refused before (see `app::ipc`).
fn typed<T: serde::de::DeserializeOwned>(payload: serde_json::Value) -> T {
    serde_json::from_value(payload).expect("the payload fits its command")
}

async fn add_pattern(harness: &TestHarness, pattern: &str) {
    add_ignore_pattern(
        typed(json!({ "pattern": pattern })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}

async fn remove_pattern(harness: &TestHarness, pattern: &str) {
    remove_ignore_pattern(
        typed(json!({ "pattern": pattern })),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}

async fn set_option(harness: &TestHarness, option: ConfigOption) {
    set_config_option(
        typed(serde_json::to_value(option).unwrap()),
        harness.proxy.clone(),
        harness.state.clone(),
    )
    .await;
}
