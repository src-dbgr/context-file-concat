//! Cross-language contract test for the host calls of the output (host → UI).
//!
//! The counterpart of `command_contract_tests.rs` for the events around generating
//! and saving (`showGeneratedContent`, `outputSizeNotice`, `saveResult`) and for the
//! notice of the ignore button (`ignorePathRefused`). For every
//! form such an event can take, this test builds the event, turns it into the script
//! the WebView evaluates ([`script_for`]) and writes the arguments of that call into
//! a JSON fixture that is committed inside the UI's test directory. The UI validates
//! each example with the schema of its handler
//! (`src/ui/src/tests/contracts/rust-fixture.test.ts`), so neither side can change
//! the shape of these calls without the other one noticing.
//!
//! After an intentional change, regenerate the fixture with
//! `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture` and adapt the UI schema.

use super::events::{IgnorePathRefusal, OutputSizeNotice, SaveResult, UserEvent};
use super::script_for;
use super::state::GeneratedDocument;
use super::tasks::{LARGE_OUTPUT_SIZE, MAX_OUTPUT_SIZE};
use serde_json::Value;
use std::collections::BTreeMap;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/output_events.json"
);
const UPDATE_ENV: &str = "UPDATE_CONTRACT_FIXTURES";

/// One example per form of each event. The `match`es do not compile once a variant
/// is added without being listed here.
fn example_events() -> Vec<UserEvent> {
    let save_results = vec![
        SaveResult::Saved {
            path: "/home/user/out/cfc_output.txt".to_string(),
        },
        SaveResult::Cancelled,
        SaveResult::Failed {
            error: "Permission denied (os error 13)".to_string(),
        },
    ];
    for result in &save_results {
        match result {
            SaveResult::Saved { .. } | SaveResult::Cancelled | SaveResult::Failed { .. } => {}
        }
    }

    let notices = vec![
        OutputSizeNotice::Large {
            estimated_bytes: LARGE_OUTPUT_SIZE + 1,
            limit_bytes: LARGE_OUTPUT_SIZE,
        },
        OutputSizeNotice::TooLarge {
            estimated_bytes: MAX_OUTPUT_SIZE + 1,
            limit_bytes: MAX_OUTPUT_SIZE,
        },
    ];
    for notice in &notices {
        match notice {
            OutputSizeNotice::Large { .. } | OutputSizeNotice::TooLarge { .. } => {}
        }
    }

    let refusals = vec![
        IgnorePathRefusal::TrailingWhitespace {
            name: "notes\u{a0}".to_string(),
        },
        IgnorePathRefusal::NotUnicode {
            name: "bad\u{fffd}.txt".to_string(),
        },
    ];
    for refusal in &refusals {
        match refusal {
            IgnorePathRefusal::TrailingWhitespace { .. } | IgnorePathRefusal::NotUnicode { .. } => {
            }
        }
    }

    let documents = vec![
        UserEvent::ShowGeneratedContent {
            document: GeneratedDocument::for_test(
                7,
                "# CFC Output\n\nsrc/a.rs\n===FILE-START===\nlet s = \"ä\";\n---FILE-END-----\n\n",
            ),
            token_count: 21,
        },
        UserEvent::ShowGeneratedContent {
            document: GeneratedDocument::for_test(8, ""),
            token_count: 0,
        },
    ];

    documents
        .into_iter()
        .chain(notices.into_iter().map(UserEvent::OutputSize))
        .chain(save_results.into_iter().map(UserEvent::SaveResult))
        .chain(refusals.into_iter().map(UserEvent::IgnorePathRefused))
        .collect()
}

/// The handler a script calls and the arguments it passes, read back from the
/// script itself: `window.<handler>(<arguments>);`.
fn call_of(script: &str) -> (String, Value) {
    let call = script
        .strip_prefix("window.")
        .and_then(|rest| rest.strip_suffix(");"))
        .unwrap_or_else(|| panic!("not a call of a window handler: {script}"));
    let (handler, arguments) = call.split_once('(').expect("a call has arguments");
    let arguments = serde_json::from_str(&format!("[{arguments}]"))
        .unwrap_or_else(|e| panic!("the arguments of {handler} are not JSON: {e}"));
    (handler.to_string(), arguments)
}

fn fixture_json() -> String {
    let mut calls: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for event in example_events() {
        let script = script_for(&event).expect("the event is serializable");
        let (handler, arguments) = call_of(&script);
        calls.entry(handler).or_default().push(arguments);
    }
    let mut json = serde_json::to_string_pretty(&calls).expect("fixture is serializable");
    json.push('\n');
    json
}

#[test]
fn contract_fixture_matches_the_host_calls_of_the_output() {
    let actual = fixture_json();

    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(FIXTURE_PATH, &actual).expect("fixture is writable");
        return;
    }

    let expected = std::fs::read_to_string(FIXTURE_PATH)
        .unwrap_or_else(|e| panic!("cannot read {FIXTURE_PATH}: {e}"))
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "The host calls of the output no longer match the fixture shared with the UI. \
         If the change is intended, run `{UPDATE_ENV}=1 cargo test contract_fixture`, \
         commit the fixture and update src/ui/src/lib/ipc/schema.ts."
    );
}

#[test]
fn examples_cover_every_handler_of_the_output() {
    let handlers: Vec<String> = serde_json::from_str::<BTreeMap<String, Value>>(&fixture_json())
        .unwrap()
        .into_keys()
        .collect();
    assert_eq!(
        handlers,
        [
            "ignorePathRefused",
            "outputSizeNotice",
            "saveResult",
            "showGeneratedContent"
        ]
    );
}
