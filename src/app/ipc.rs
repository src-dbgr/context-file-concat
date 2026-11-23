//! The wire format of what the UI posts: the envelope and the commands.
//!
//! A message is
//! `{"session": <number>, "seq": <number>, "command": "<name>", "payload": <payload | null>}`.
//!
//! * `session` and `seq` belong to the transport: the UI names the commands it
//!   posts (who it is, and which of its commands this is), and the snapshots tell
//!   it how far the host has got (see [`super::correlation`]). They are not part of
//!   any payload, and no command sees them.
//! * `command` and `payload` are one [`Command`]. That type is the contract: reading
//!   a message into it is the validation of the message and decides which command
//!   runs with which payload, in one step. There is no list of command names beside
//!   it and no handler that looks at JSON.
//!
//! # Messages that are refused
//!
//! Every message that is not a named command is treated the same way, whatever is
//! wrong with it (not JSON, no `session` or `seq`, an unknown command, a payload that
//! is missing a field, has one too many or one of the wrong type, an extra field in
//! the envelope): it is logged, it changes nothing, and it is answered with the
//! unchanged snapshot. If the message has a name, the snapshot accounts for it.
//!
//! What is logged is short, whatever the size of the message: the reason is cut
//! ([`excerpt`]), because the reason serde gives quotes the value it stumbled over,
//! and a payload can be as large as an output.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::commands::{
    ConfigOption, IgnorePatternPayload, LocateRowPayload, SaveFilePayload, SetExpansionPayload,
    SetViewportPayload, UpdateFiltersPayload,
};
use super::correlation::CommandId;

/// A command of the IPC contract with its payload, on the wire
/// `{"command": "<name>", "payload": <payload>}`. A command without a payload is
/// sent with `"payload": null`.
///
/// What each command does is described at its handler in [`super::commands`]; on
/// which lane it runs is decided where it is routed (`Command::into_work`).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(
    tag = "command",
    content = "payload",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Command {
    Initialize,
    SelectDirectory,
    RescanDirectory,
    ClearDirectory,
    CancelScan,
    LoadDirectoryLevel(PathBuf),
    SetExpansion(SetExpansionPayload),
    /// `true` expands, `false` collapses.
    ExpandCollapseAll(bool),
    SetViewport(SetViewportPayload),
    LocateRow(LocateRowPayload),
    ToggleSelection(PathBuf),
    ToggleDirectorySelection(PathBuf),
    SelectAll,
    DeselectAll,
    UpdateFilters(UpdateFiltersPayload),
    LoadFilePreview(PathBuf),
    ClearPreviewState,
    GeneratePreview,
    CancelGeneration,
    SaveFile(SaveFilePayload),
    PickOutputDirectory,
    AddIgnorePattern(IgnorePatternPayload),
    RemoveIgnorePattern(IgnorePatternPayload),
    ClearIgnorePatterns,
    AddIgnorePath(PathBuf),
    SetConfigOption(ConfigOption),
    ImportConfig,
    ExportConfig,
}

/// A message of the UI, as far as it could be read.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum IpcMessage {
    /// A named command.
    Command { id: CommandId, command: Command },
    /// A named message that is no command; `reason` says why (in short).
    Refused { id: CommandId, reason: String },
    /// Not a message at all: it has no name; `reason` says why (in short).
    Unreadable { reason: String },
}

/// The beginning of `text`, for the log: a message can be as large as an output,
/// and so can what is said about it.
pub(crate) fn excerpt(text: &str) -> String {
    const MAX_CHARS: usize = 200;
    let mut excerpt: String = text.chars().take(MAX_CHARS).collect();
    if excerpt.len() < text.len() {
        excerpt.push('…');
    }
    excerpt
}

impl IpcMessage {
    /// Reads a message the WebView has posted.
    pub(crate) fn parse(message: &str) -> Self {
        let mut envelope = match serde_json::from_str::<Value>(message) {
            Ok(Value::Object(envelope)) => envelope,
            Ok(_) => {
                return Self::Unreadable {
                    reason: "the message is not an object".to_string(),
                }
            }
            Err(e) => {
                return Self::Unreadable {
                    reason: excerpt(&e.to_string()),
                }
            }
        };
        // The name is taken off; what is left has to be the command, and nothing else.
        let mut number = |field: &str| envelope.remove(field).as_ref().and_then(Value::as_u64);
        let (Some(session), Some(seq)) = (number("session"), number("seq")) else {
            return Self::Unreadable {
                reason: "the message has no `session` and `seq` (non-negative integers)"
                    .to_string(),
            };
        };
        let id = CommandId { session, seq };
        match Command::deserialize(Value::Object(envelope)) {
            Ok(command) => Self::Command { id, command },
            Err(e) => Self::Refused {
                id,
                reason: excerpt(&e.to_string()),
            },
        }
    }
}

#[cfg(test)]
impl Command {
    /// The name of every command on the wire, in the order of the enum.
    ///
    /// Taken from the enum itself: it is the list serde states when it meets a name
    /// that is none of them, so it cannot miss a command or name one that is not
    /// routed.
    pub(crate) fn wire_names() -> &'static [&'static str] {
        use serde::de::value::MapDeserializer;

        #[derive(Debug)]
        struct Names(&'static [&'static str]);
        impl std::fmt::Display for Names {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{:?}", self.0)
            }
        }
        impl std::error::Error for Names {}
        impl serde::de::Error for Names {
            fn custom<T: std::fmt::Display>(_: T) -> Self {
                Names(&[])
            }
            fn unknown_variant(_: &str, expected: &'static [&'static str]) -> Self {
                Names(expected)
            }
        }

        let no_such_command =
            MapDeserializer::<_, Names>::new(std::iter::once(("command", "\u{0}no such command")));
        let Err(Names(names)) = Command::deserialize(no_such_command) else {
            panic!("a name that is no command was read as one");
        };
        assert!(!names.is_empty(), "serde did not state the commands");
        names
    }

    /// The name of this command on the wire.
    pub(crate) fn wire_name(&self) -> String {
        let message = serde_json::to_value(self).expect("a command is serializable");
        message["command"]
            .as_str()
            .expect("a command has a name")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::commands::RowTarget;
    use serde_json::json;

    use crate::app::correlation::CommandSeq;

    /// The UI of these tests.
    const UI: u64 = 4242;

    fn id(seq: CommandSeq) -> CommandId {
        CommandId { session: UI, seq }
    }

    /// Reads `message` as one of the UI of these tests.
    fn parse(mut message: Value) -> IpcMessage {
        message["session"] = json!(UI);
        IpcMessage::parse(&message.to_string())
    }

    fn command(seq: CommandSeq, command: Command) -> IpcMessage {
        IpcMessage::Command {
            id: id(seq),
            command,
        }
    }

    fn is_refused(message: Value, seq: CommandSeq) -> bool {
        matches!(parse(message), IpcMessage::Refused { id: refused, .. } if refused == id(seq))
    }

    #[test]
    fn a_message_is_a_number_and_a_command() {
        assert_eq!(
            parse(json!({ "seq": 1, "command": "initialize", "payload": null })),
            command(1, Command::Initialize)
        );
        assert_eq!(
            parse(json!({ "seq": 2, "command": "toggleSelection", "payload": "/repo/a.rs" })),
            command(2, Command::ToggleSelection(PathBuf::from("/repo/a.rs")))
        );
        assert_eq!(
            parse(json!({ "seq": 3, "command": "expandCollapseAll", "payload": false })),
            command(3, Command::ExpandCollapseAll(false))
        );
        assert_eq!(
            parse(json!({
                "seq": 4,
                "command": "locateRow",
                "payload": { "request": 9, "target": { "kind": "typeahead", "prefix": "ma", "from": 3 } },
            })),
            command(
                4,
                Command::LocateRow(LocateRowPayload {
                    request: 9,
                    target: RowTarget::Typeahead {
                        prefix: "ma".to_string(),
                        from: 3
                    },
                })
            )
        );
        assert_eq!(
            parse(json!({
                "seq": 5,
                "command": "setConfigOption",
                "payload": { "option": "output_directory", "value": null },
            })),
            command(
                5,
                Command::SetConfigOption(ConfigOption::OutputDirectory(None))
            )
        );
    }

    /// The order of the fields does not matter, and the number is not a payload.
    #[test]
    fn the_number_is_part_of_the_envelope_only() {
        let message = r#"{"payload":{"path":"/repo/src","expanded":true},"command":"setExpansion","seq":12,"session":4242}"#;
        assert_eq!(
            IpcMessage::parse(message),
            command(
                12,
                Command::SetExpansion(SetExpansionPayload {
                    path: PathBuf::from("/repo/src"),
                    expanded: true,
                })
            )
        );
        // Inside a payload it is an unknown field like any other.
        assert!(is_refused(
            json!({
                "seq": 13,
                "command": "setExpansion",
                "payload": { "path": "/repo/src", "expanded": true, "seq": 13 },
            }),
            13
        ));
    }

    #[test]
    fn a_command_without_a_payload_takes_null_or_none() {
        assert_eq!(
            parse(json!({ "seq": 1, "command": "selectAll", "payload": null })),
            command(1, Command::SelectAll)
        );
        assert_eq!(
            parse(json!({ "seq": 2, "command": "selectAll" })),
            command(2, Command::SelectAll)
        );
        assert!(is_refused(
            json!({ "seq": 3, "command": "selectAll", "payload": { "all": true } }),
            3
        ));
        assert!(is_refused(
            json!({ "seq": 4, "command": "selectAll", "payload": "/repo" }),
            4
        ));
    }

    #[test]
    fn a_numbered_message_that_is_no_command_is_refused_under_its_number() {
        let refused = [
            // No such command (also the one that used to replace the configuration).
            json!({ "seq": 7, "command": "noSuchCommand", "payload": null }),
            json!({ "seq": 7, "command": "updateConfig", "payload": {} }),
            json!({ "seq": 7, "command": 3, "payload": null }),
            json!({ "seq": 7, "payload": null }),
            json!({ "seq": 7 }),
            // A payload that is missing, of the wrong type, incomplete or too rich.
            json!({ "seq": 7, "command": "toggleSelection", "payload": null }),
            json!({ "seq": 7, "command": "toggleSelection" }),
            json!({ "seq": 7, "command": "toggleSelection", "payload": 3 }),
            json!({ "seq": 7, "command": "loadFilePreview", "payload": { "path": "/a" } }),
            json!({ "seq": 7, "command": "expandCollapseAll", "payload": "yes" }),
            json!({ "seq": 7, "command": "setExpansion", "payload": { "path": "/a" } }),
            json!({ "seq": 7, "command": "setViewport", "payload": { "start": 5 } }),
            json!({ "seq": 7, "command": "setViewport", "payload": { "start": 5, "count": 0 } }),
            json!({ "seq": 7, "command": "setViewport", "payload": { "start": -1, "count": 5 } }),
            json!({
                "seq": 7,
                "command": "locateRow",
                "payload": { "request": 1, "target": { "kind": "sideways", "path": "/a" } },
            }),
            json!({ "seq": 7, "command": "updateFilters", "payload": { "query": "x" } }),
            json!({ "seq": 7, "command": "updateFilters", "payload": { "searchQuery": 1 } }),
            json!({ "seq": 7, "command": "addIgnorePattern", "payload": "*.log" }),
            json!({
                "seq": 7,
                "command": "setConfigOption",
                "payload": { "option": "window_size", "value": [1, 2] },
            }),
            json!({
                "seq": 7,
                "command": "setConfigOption",
                "payload": { "option": "case_sensitive_search", "value": "yes" },
            }),
            json!({ "seq": 7, "command": "saveFile", "payload": { "kind": "generated" } }),
            json!({ "seq": 7, "command": "saveFile", "payload": "content" }),
            // More than a command in the envelope.
            json!({ "seq": 7, "command": "selectAll", "payload": null, "extra": 1 }),
        ];
        for message in refused {
            assert!(is_refused(message.clone(), 7), "{message}");
        }
    }

    #[test]
    fn a_message_without_a_number_is_unreadable() {
        let unreadable = [
            "not json".to_string(),
            String::new(),
            "null".to_string(),
            "[1, 2]".to_string(),
            "\"initialize\"".to_string(),
            json!({ "command": "initialize", "payload": null }).to_string(),
            // Half a name is none.
            json!({ "seq": 1, "command": "initialize", "payload": null }).to_string(),
            json!({ "session": 9, "command": "initialize", "payload": null }).to_string(),
        ]
        .into_iter()
        .chain(
            [json!("1"), json!(-1), json!(1.5), json!(null)]
                .into_iter()
                .flat_map(|bad| {
                    [
                        json!({ "session": 9, "seq": bad, "command": "initialize", "payload": null }),
                        json!({ "session": bad, "seq": 1, "command": "initialize", "payload": null }),
                    ]
                })
                .map(|message| message.to_string()),
        );
        for message in unreadable {
            assert!(
                matches!(IpcMessage::parse(&message), IpcMessage::Unreadable { .. }),
                "{message}"
            );
        }
    }

    /// The filter texts are optional, each of them: a text that is left out is empty.
    #[test]
    fn filter_texts_that_are_left_out_are_empty() {
        assert_eq!(
            parse(
                json!({ "seq": 1, "command": "updateFilters", "payload": { "searchQuery": "a" } })
            ),
            command(
                1,
                Command::UpdateFilters(UpdateFiltersPayload {
                    search_query: "a".to_string(),
                    ..Default::default()
                })
            )
        );
    }

    #[test]
    fn the_names_of_the_commands_are_taken_from_the_enum() {
        let names = Command::wire_names();
        assert_eq!(names.len(), 28);
        assert!(names.contains(&"initialize"));
        assert!(names.contains(&"toggleDirectorySelection"));
        assert!(!names.contains(&"updateConfig"));
        assert_eq!(Command::Initialize.wire_name(), "initialize");
        assert_eq!(
            Command::ExpandCollapseAll(true).wire_name(),
            "expandCollapseAll"
        );
    }

    /// The reason serde gives for a refusal quotes the value that does not fit. What
    /// is kept of it (and logged) stays short, however large the payload is.
    #[test]
    fn the_reason_of_a_refusal_is_short_whatever_the_size_of_the_payload() {
        let large = "x".repeat(700_000);
        let refused = [
            json!({ "seq": 1, "command": "expandCollapseAll", "payload": large }),
            json!({ "seq": 1, "command": large, "payload": null }),
            json!({ "seq": 1, "command": "setConfigOption", "payload": { "option": large, "value": 1 } }),
            json!({
                "seq": 1,
                "command": "setConfigOption",
                "payload": { "option": "case_sensitive_search", "value": large },
            }),
            json!({ "seq": 1, "command": "selectAll", "payload": null, large.clone(): 1 }),
        ];
        for message in refused {
            let IpcMessage::Refused { reason, .. } = parse(message) else {
                panic!("the message is refused");
            };
            assert!(reason.chars().count() <= 201, "{} characters", reason.len());
            assert!(!reason.is_empty());
        }

        let IpcMessage::Unreadable { reason } = IpcMessage::parse(&large) else {
            panic!("the message cannot be read");
        };
        assert!(reason.chars().count() <= 201);
    }

    #[test]
    fn an_excerpt_is_the_beginning_of_a_text() {
        assert_eq!(excerpt("not json"), "not json");
        let long = "ä".repeat(10_000);
        let shown = excerpt(&long);
        assert_eq!(shown.chars().count(), 201);
        assert!(shown.ends_with('…'));
    }
}
