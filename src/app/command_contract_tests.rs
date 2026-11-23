//! Cross-language contract test for the commands (UI → host).
//!
//! The counterpart of `view_model/contract_tests.rs`, which guards the other direction.
//! The commands of the host are one type, [`Command`]: reading a message into it is
//! what validates and routes the message. This test writes examples of every command
//! down, as values of that type, and serializes them into two JSON fixtures that are
//! committed inside the UI's test directory:
//!
//! * `commands.json`: every command name with example payloads. The UI validates
//!   each example with its `CommandSchemas` and checks that it knows exactly the
//!   same command names (`src/ui/src/tests/contracts/rust-fixture.test.ts`).
//! * `command_messages.json`: the same examples as complete messages, with the
//!   session and the number of their envelope, as the host reads them. The UI checks
//!   that `post()` writes exactly these (with a session of its own).
//!
//! The list of names is not kept by hand: it is the one serde states for the enum
//! ([`Command::wire_names`]), and the examples have to cover it.
//!
//! After an intentional change, regenerate the fixtures with
//! `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture` and adapt the UI schema.

use super::commands::{
    ConfigOption, IgnorePatternPayload, LocateRowPayload, RowTarget, SaveFilePayload,
    SetExpansionPayload, SetViewportPayload, UpdateFiltersPayload,
};
use super::correlation::{CommandId, CommandSeq, SessionId};
use super::ipc::{Command, IpcMessage};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/commands.json"
);
const MESSAGES_FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/command_messages.json"
);
const UPDATE_ENV: &str = "UPDATE_CONTRACT_FIXTURES";

/// One example per option. The `match` does not compile once an option is added
/// without being listed here.
fn config_option_examples() -> Vec<ConfigOption> {
    let examples = vec![
        ConfigOption::CaseSensitiveSearch(true),
        ConfigOption::RemoveEmptyDirectories(true),
        ConfigOption::IncludeTreeByDefault(false),
        ConfigOption::UseRelativePaths(false),
        ConfigOption::OutputFilename("context.txt".to_string()),
        ConfigOption::OutputDirectory(Some(PathBuf::from("/home/user/out"))),
        ConfigOption::OutputDirectory(None),
    ];
    for example in &examples {
        match example {
            ConfigOption::CaseSensitiveSearch(_)
            | ConfigOption::RemoveEmptyDirectories(_)
            | ConfigOption::IncludeTreeByDefault(_)
            | ConfigOption::UseRelativePaths(_)
            | ConfigOption::OutputFilename(_)
            | ConfigOption::OutputDirectory(_) => {}
        }
    }
    examples
}

/// One example per kind of target. The `match` does not compile once a kind is added
/// without being listed here.
fn row_target_examples() -> Vec<RowTarget> {
    let examples = vec![
        RowTarget::Path {
            path: PathBuf::from("/project/src/main.rs"),
        },
        RowTarget::ParentOf {
            path: PathBuf::from("/project/src/main.rs"),
        },
        RowTarget::Typeahead {
            prefix: "ma".to_string(),
            from: 3,
        },
    ];
    for example in &examples {
        match example {
            RowTarget::Path { .. } | RowTarget::ParentOf { .. } | RowTarget::Typeahead { .. } => {}
        }
    }
    examples
}

/// One example per kind of save. The `match` does not compile once a kind is added
/// without being listed here.
fn save_file_examples() -> Vec<SaveFilePayload> {
    let examples = vec![
        SaveFilePayload::Generated { document_id: 7 },
        SaveFilePayload::Edited {
            content: "generated content, edited".to_string(),
        },
    ];
    for example in &examples {
        match example {
            SaveFilePayload::Generated { .. } | SaveFilePayload::Edited { .. } => {}
        }
    }
    examples
}

/// Examples of every command, as the UI has to send them: at least one per command,
/// and one per kind where a payload has kinds.
///
/// The `match` does not compile once a command is added without being listed here,
/// and `the_examples_cover_every_command` fails until it has an example.
pub(super) fn example_commands() -> Vec<Command> {
    use Command as C;
    let path = || PathBuf::from("/project/src");
    let pattern = || IgnorePatternPayload {
        pattern: "*.log".to_string(),
    };

    let mut examples = vec![
        C::Initialize,
        C::SelectDirectory,
        C::RescanDirectory,
        C::ClearDirectory,
        C::CancelScan,
        C::LoadDirectoryLevel(path()),
        C::SetExpansion(SetExpansionPayload {
            path: path(),
            expanded: true,
        }),
        C::ExpandCollapseAll(true),
        C::ExpandCollapseAll(false),
        C::SetViewport(SetViewportPayload {
            start: 120,
            count: std::num::NonZeroUsize::new(90).expect("not zero"),
        }),
        C::ToggleSelection(path()),
        C::ToggleDirectorySelection(path()),
        C::SelectAll,
        C::DeselectAll,
        C::UpdateFilters(UpdateFiltersPayload {
            search_query: "main".to_string(),
            extension_filter: "rs".to_string(),
            content_search_query: "needle".to_string(),
        }),
        C::UpdateFilters(UpdateFiltersPayload::default()),
        C::LoadFilePreview(path()),
        C::ClearPreviewState,
        C::GeneratePreview,
        C::CancelGeneration,
        C::PickOutputDirectory,
        C::AddIgnorePattern(pattern()),
        C::RemoveIgnorePattern(pattern()),
        C::ClearIgnorePatterns,
        C::AddIgnorePath(path()),
        C::ImportConfig,
        C::ExportConfig,
    ];
    examples.extend(
        row_target_examples()
            .into_iter()
            .enumerate()
            .map(|(request, target)| {
                C::LocateRow(LocateRowPayload {
                    request: request as u32 + 1,
                    target,
                })
            }),
    );
    examples.extend(save_file_examples().into_iter().map(C::SaveFile));
    examples.extend(config_option_examples().into_iter().map(C::SetConfigOption));

    for example in &examples {
        match example {
            C::Initialize
            | C::SelectDirectory
            | C::RescanDirectory
            | C::ClearDirectory
            | C::CancelScan
            | C::LoadDirectoryLevel(_)
            | C::SetExpansion(_)
            | C::ExpandCollapseAll(_)
            | C::SetViewport(_)
            | C::LocateRow(_)
            | C::ToggleSelection(_)
            | C::ToggleDirectorySelection(_)
            | C::SelectAll
            | C::DeselectAll
            | C::UpdateFilters(_)
            | C::LoadFilePreview(_)
            | C::ClearPreviewState
            | C::GeneratePreview
            | C::CancelGeneration
            | C::SaveFile(_)
            | C::PickOutputDirectory
            | C::AddIgnorePattern(_)
            | C::RemoveIgnorePattern(_)
            | C::ClearIgnorePatterns
            | C::AddIgnorePath(_)
            | C::SetConfigOption(_)
            | C::ImportConfig
            | C::ExportConfig => {}
        }
    }
    examples
}

/// The UI the example messages are posted by.
const EXAMPLE_SESSION: SessionId = 1_234_567;

fn example_id(seq: CommandSeq) -> CommandId {
    CommandId {
        session: EXAMPLE_SESSION,
        seq,
    }
}

/// A command as the UI posts it: `{"session", "seq", "command", "payload"}`. The UI
/// always sends a payload, `null` for a command that has none.
fn message(id: CommandId, command: &Command) -> Value {
    let mut message = serde_json::to_value(command).expect("a command is serializable");
    let fields = message.as_object_mut().expect("a command is an object");
    fields.entry("payload").or_insert(Value::Null);
    fields.insert("session".to_string(), json!(id.session));
    fields.insert("seq".to_string(), json!(id.seq));
    message
}

/// The examples as numbered messages, in the order they are posted.
fn example_messages() -> Vec<Value> {
    example_commands()
        .iter()
        .zip(1..)
        .map(|(command, seq)| message(example_id(seq), command))
        .collect()
}

fn pretty(fixture: &impl serde::Serialize) -> String {
    let mut json = serde_json::to_string_pretty(fixture).expect("fixture is serializable");
    json.push('\n');
    json
}

/// Every command name with its example payloads.
fn fixture_json() -> String {
    let mut commands: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for message in example_messages() {
        let name = message["command"].as_str().expect("a name").to_string();
        commands
            .entry(name)
            .or_default()
            .push(message["payload"].clone());
    }
    pretty(&commands)
}

fn assert_fixture(path: &str, actual: &str) {
    if std::env::var_os(UPDATE_ENV).is_some() {
        std::fs::write(path, actual).expect("fixture is writable");
        return;
    }

    let expected = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "The commands or their payloads no longer match the fixture shared with the UI. \
         If the change is intended, run `{UPDATE_ENV}=1 cargo test contract_fixture`, \
         commit the fixture and update src/ui/src/lib/ipc/schema.ts."
    );
}

#[test]
fn contract_fixture_matches_the_commands_of_the_host() {
    assert_fixture(FIXTURE_PATH, &fixture_json());
}

#[test]
fn contract_fixture_matches_the_messages_the_host_reads() {
    assert_fixture(MESSAGES_FIXTURE_PATH, &pretty(&example_messages()));
}

/// The names are those of the enum, each command has an example, and no example
/// names anything else.
#[test]
fn the_examples_cover_every_command() {
    let names: BTreeSet<&str> = Command::wire_names().iter().copied().collect();
    assert_eq!(names.len(), Command::wire_names().len(), "duplicate name");

    let with_example: BTreeSet<String> =
        example_commands().iter().map(Command::wire_name).collect();
    assert_eq!(
        with_example.iter().map(String::as_str).collect::<Vec<_>>(),
        names.into_iter().collect::<Vec<_>>()
    );
}

/// What is written into the fixtures is what the host accepts: every message is read
/// back as the command it was made of, under its number.
#[test]
fn the_example_messages_are_read_as_the_commands_they_were_made_of() {
    for (command, seq) in example_commands().into_iter().zip(1..) {
        let id = example_id(seq);
        let message = message(id, &command);
        assert_eq!(
            IpcMessage::parse(&message.to_string()),
            IpcMessage::Command { id, command },
            "{message}"
        );
    }
}

/// A payload has one way to be written: serializing what was read gives the message
/// back. (The UI compares what it posts with these very messages.)
#[test]
fn the_example_messages_are_written_one_way() {
    for example in example_messages() {
        let IpcMessage::Command { id, command } = IpcMessage::parse(&example.to_string()) else {
            panic!("not a command: {example}");
        };
        assert_eq!(message(id, &command), example);
    }
}
