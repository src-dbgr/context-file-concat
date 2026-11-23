use super::*;

// --- Serialization of events ---

fn ui_state_with_tree(paths: &[PathBuf]) -> view_model::UiState {
    let mut state = AppState::default();
    state.current_path = "/repo".to_string();
    state.set_full_file_list(
        paths
            .iter()
            .map(|path| crate::core::FileItem {
                path: path.clone(),
                parent: path.parent().map(Path::to_path_buf),
                depth: 1,
                ..Default::default()
            })
            .collect(),
    );
    filtering::apply_filters(&mut state);
    view_model::generate_ui_state(&state)
}

#[test]
fn every_event_becomes_one_call_with_its_arguments() {
    let script = |event| script_for(&event).unwrap();

    let render = script(UserEvent::StateUpdate(Snapshot::for_test(
        ui_state_with_tree(&[PathBuf::from("/repo/a.rs")]),
    )));
    assert!(
        render.starts_with("window.render({\"config\":{"),
        "{render}"
    );
    assert!(render.contains("\"path\":\"/repo/a.rs\""), "{render}");
    assert!(render.ends_with("});"), "{render}");

    assert_eq!(
        script(UserEvent::ShowFilePreview {
            content: "let a = \"b\";".to_string(),
            language: "rust".to_string(),
            search_term: None,
            path: PathBuf::from("/repo/a.rs"),
        }),
        r#"window.showPreviewContent("let a = \"b\";", "rust", null, "/repo/a.rs");"#
    );
    assert_eq!(
        script(UserEvent::ShowGeneratedContent {
            document: GeneratedDocument::for_test(7, "x \"y\"\n"),
            token_count: 3,
        }),
        r#"window.showGeneratedContent("x \"y\"\n", 3, 7);"#
    );
    assert_eq!(
        script(UserEvent::OutputSize(OutputSizeNotice::Large {
            estimated_bytes: 30,
            limit_bytes: 20,
        })),
        r#"window.outputSizeNotice({"status":"large","estimated_bytes":30,"limit_bytes":20});"#
    );
    assert_eq!(
        script(UserEvent::OutputSize(OutputSizeNotice::TooLarge {
            estimated_bytes: 300,
            limit_bytes: 200,
        })),
        r#"window.outputSizeNotice({"status":"too_large","estimated_bytes":300,"limit_bytes":200});"#
    );
    assert_eq!(
        script(UserEvent::ShowError("it's \"broken\"".to_string())),
        r#"window.showError("it's \"broken\"");"#
    );
    assert_eq!(
        script(UserEvent::SaveResult(SaveResult::Saved {
            path: "/out/a.txt".to_string()
        })),
        r#"window.saveResult({"status":"saved","path":"/out/a.txt"});"#
    );
    assert_eq!(
        script(UserEvent::SaveResult(SaveResult::Cancelled)),
        r#"window.saveResult({"status":"cancelled"});"#
    );
    assert_eq!(
        script(UserEvent::SaveResult(SaveResult::Failed {
            error: "disk \"full\"".to_string()
        })),
        r#"window.saveResult({"status":"failed","error":"disk \"full\""});"#
    );
    assert_eq!(
        script(UserEvent::ConfigExported(false)),
        "window.showStatus('Failed to export config.');"
    );
    assert_eq!(
        script(UserEvent::DragStateChanged(true)),
        "window.setDragState(true);"
    );
    let progress = script(UserEvent::ScanProgress(crate::core::ScanProgress {
        files_scanned: 4,
        large_files_skipped: 1,
        current_scanning_path: "/repo/src".to_string(),
    }));
    assert!(
        progress.starts_with("window.updateScanProgress({"),
        "{progress}"
    );
    assert!(progress.contains("\"files_scanned\":4"), "{progress}");
}

/// A file name that is not valid Unicode must not cost the UI its snapshot: serde
/// refuses such a path, so paths are sent lossily.
#[cfg(unix)]
#[test]
fn a_path_that_is_not_unicode_does_not_make_an_event_unserializable() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let odd = PathBuf::from("/repo").join(OsStr::from_bytes(b"caf\xe9.txt"));
    assert!(
        serde_json::to_string(&odd).is_err(),
        "what serde does on its own"
    );

    let mut ui_state = ui_state_with_tree(&[odd.clone(), PathBuf::from("/repo/b.txt")]);
    ui_state.config.output_directory = Some(odd.clone());
    let render = script_for(&UserEvent::StateUpdate(Snapshot::for_test(ui_state))).unwrap();
    assert!(
        render.contains("\"path\":\"/repo/caf\u{fffd}.txt\""),
        "{render}"
    );
    assert!(render.contains("\"path\":\"/repo/b.txt\""), "{render}");
    assert!(
        render.contains("\"output_directory\":\"/repo/caf\u{fffd}.txt\""),
        "{render}"
    );

    let preview = script_for(&UserEvent::ShowFilePreview {
        content: String::new(),
        language: "plaintext".to_string(),
        search_term: Some("x".to_string()),
        path: odd,
    })
    .unwrap();
    assert_eq!(
        preview,
        "window.showPreviewContent(\"\", \"plaintext\", \"x\", \"/repo/caf\u{fffd}.txt\");"
    );
}
