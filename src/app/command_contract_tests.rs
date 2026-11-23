//! Cross-language contract test for the commands (UI → host).
//!
//! The counterpart of `view_model/contract_tests.rs`, which guards the other direction.
//! For every command of the routing table ([`COMMANDS`]) this test serializes example
//! payloads, wherever a command has a payload type from that very type, and compares
//! the result with a JSON fixture that is committed inside the UI's test directory.
//! The UI validates each example with its `CommandSchemas` and checks that it knows
//! exactly the same command names (`src/ui/src/tests/contracts/rust-fixture.test.ts`).
//!
//! After an intentional change, regenerate the fixture with
//! `UPDATE_CONTRACT_FIXTURES=1 cargo test contract_fixture` and adapt the UI schema.

use super::commands::{
    ConfigOption, IgnorePatternPayload, LocateRowPayload, RowTarget, SaveFilePayload,
    SetExpansionPayload, SetViewportPayload, UpdateFiltersPayload,
};
use super::{Command, DialogCommand, OrderedCommand, COMMANDS};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;

const FIXTURE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/ui/src/tests/contracts/fixtures/commands.json"
);
const UPDATE_ENV: &str = "UPDATE_CONTRACT_FIXTURES";

fn to_json<T: serde::Serialize>(payload: T) -> Value {
    serde_json::to_value(payload).expect("payload is serializable")
}

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

/// Example payloads of a command, as the UI has to send them.
fn example_payloads(command: Command) -> Vec<Value> {
    use OrderedCommand as C;
    let path = || to_json(PathBuf::from("/project/src"));
    let pattern = || {
        to_json(IgnorePatternPayload {
            pattern: "*.log".to_string(),
        })
    };

    match command {
        Command::Dialog(command) => match command {
            DialogCommand::SelectDirectory
            | DialogCommand::PickOutputDirectory
            | DialogCommand::ImportConfig
            | DialogCommand::ExportConfig => vec![Value::Null],
            DialogCommand::SaveFile => save_file_examples().into_iter().map(to_json).collect(),
        },
        Command::Ordered(command) => match command {
            C::Initialize
            | C::RescanDirectory
            | C::ClearDirectory
            | C::CancelScan
            | C::SelectAll
            | C::DeselectAll
            | C::ClearPreviewState
            | C::GeneratePreview
            | C::CancelGeneration
            | C::ClearIgnorePatterns => vec![Value::Null],
            C::LoadDirectoryLevel
            | C::ToggleSelection
            | C::ToggleDirectorySelection
            | C::LoadFilePreview
            | C::AddIgnorePath => vec![path()],
            C::SetExpansion => vec![to_json(SetExpansionPayload {
                path: PathBuf::from("/project/src"),
                expanded: true,
            })],
            C::ExpandCollapseAll => vec![json!(true), json!(false)],
            C::SetViewport => vec![to_json(SetViewportPayload {
                start: 120,
                count: std::num::NonZeroUsize::new(90).expect("not zero"),
            })],
            C::LocateRow => row_target_examples()
                .into_iter()
                .enumerate()
                .map(|(request, target)| {
                    to_json(LocateRowPayload {
                        request: request as u32 + 1,
                        target,
                    })
                })
                .collect(),
            C::UpdateFilters => vec![
                to_json(UpdateFiltersPayload {
                    search_query: "main".to_string(),
                    extension_filter: "rs".to_string(),
                    content_search_query: "needle".to_string(),
                }),
                to_json(UpdateFiltersPayload::default()),
            ],
            C::AddIgnorePattern | C::RemoveIgnorePattern => vec![pattern()],
            C::SetConfigOption => config_option_examples().into_iter().map(to_json).collect(),
        },
    }
}

fn fixture_json() -> String {
    let commands: serde_json::Map<String, Value> = COMMANDS
        .iter()
        .map(|(name, command)| (name.to_string(), Value::Array(example_payloads(*command))))
        .collect();
    let mut json =
        serde_json::to_string_pretty(&Value::Object(commands)).expect("fixture is serializable");
    json.push('\n');
    json
}

#[test]
fn contract_fixture_matches_the_commands_of_the_routing_table() {
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
        "The commands or their payloads no longer match the fixture shared with the UI. \
         If the change is intended, run `{UPDATE_ENV}=1 cargo test contract_fixture`, \
         commit the fixture and update src/ui/src/lib/ipc/schema.ts."
    );
}

#[test]
fn routing_table_lists_every_command_once() {
    let names: BTreeSet<&str> = COMMANDS.iter().map(|(name, _)| *name).collect();
    assert_eq!(names.len(), COMMANDS.len(), "duplicate command name");

    let mut targets = Vec::new();
    for (name, command) in COMMANDS {
        assert!(!targets.contains(command), "{name} shares its target");
        targets.push(*command);
        assert_eq!(Command::from_name(name), Some(*command));
        assert!(!example_payloads(*command).is_empty(), "{name}");
    }
    assert_eq!(Command::from_name("updateConfig"), None);
    assert_eq!(Command::from_name("nope"), None);
}

/// The typed examples are what the handlers accept: they survive the way back.
#[test]
fn typed_example_payloads_are_accepted_by_the_host() {
    for option in config_option_examples() {
        let parsed: ConfigOption = serde_json::from_value(to_json(&option)).unwrap();
        assert_eq!(parsed, option);
    }
    for example in save_file_examples() {
        let parsed: SaveFilePayload = serde_json::from_value(to_json(&example)).unwrap();
        assert_eq!(parsed, example);
    }
    for payload in example_payloads(Command::Ordered(OrderedCommand::AddIgnorePattern)) {
        serde_json::from_value::<IgnorePatternPayload>(payload).unwrap();
    }
    for payload in example_payloads(Command::Ordered(OrderedCommand::SetExpansion)) {
        serde_json::from_value::<SetExpansionPayload>(payload).unwrap();
    }
    for payload in example_payloads(Command::Ordered(OrderedCommand::SetViewport)) {
        let parsed: SetViewportPayload = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(to_json(parsed), payload);
    }
    for payload in example_payloads(Command::Ordered(OrderedCommand::LocateRow)) {
        let parsed: LocateRowPayload = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(to_json(parsed), payload);
    }
    for payload in example_payloads(Command::Ordered(OrderedCommand::UpdateFilters)) {
        let parsed: UpdateFiltersPayload = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(to_json(parsed), payload);
    }
}
