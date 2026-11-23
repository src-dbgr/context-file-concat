use super::*;

#[tokio::test]
async fn test_begin_generation_marks_state_as_generating() {
    let mut state = AppState::default();
    assert!(!state.is_generating());

    let token = state.begin_generation();

    assert!(state.is_generating());
    assert!(state.is_current_generation(&token));
    assert!(!token.is_cancelled());
    assert!(
        !state.is_scanning(),
        "Generating is independent of scanning"
    );
}

#[tokio::test]
async fn test_begin_generation_supersedes_previous_generation() {
    let mut state = AppState::default();
    let old_token = state.begin_generation();
    let old_handle = create_dummy_task();
    let old_abort = old_handle.abort_handle();
    state.attach_generation_task(&old_token, old_handle);

    let new_token = state.begin_generation();
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(
        old_token.is_cancelled(),
        "Old generation must be told to stop"
    );
    assert!(
        old_abort.is_finished(),
        "Old generation task must be aborted"
    );
    assert!(!state.is_current_generation(&old_token));
    assert!(state.is_current_generation(&new_token));
    assert!(!new_token.is_cancelled());
    assert!(state.is_generating());
}

#[tokio::test]
async fn test_finish_generation_with_stale_token_leaves_new_generation_untouched() {
    let mut state = AppState::default();
    let old_token = state.begin_generation();
    let new_token = state.begin_generation();
    state.attach_generation_task(&new_token, create_dummy_task());

    let finished = state.finish_generation(&old_token);

    assert!(!finished);
    assert!(
        state.is_generating(),
        "New generation must still be running"
    );
    assert!(
        state.has_generation_task(),
        "New generation must stay cancellable"
    );
    assert!(state.is_current_generation(&new_token));
    state.cancel_current_generation();
}

#[tokio::test]
async fn test_finish_generation_with_current_token_releases_generating_state() {
    let mut state = AppState::default();
    let token = state.begin_generation();
    state.attach_generation_task(&token, create_dummy_task());

    assert!(state.finish_generation(&token));

    assert!(!state.is_generating());
    assert!(!state.has_generation_task());
    assert!(!token.is_cancelled(), "Finishing is not cancelling");
    assert!(
        !state.finish_generation(&token),
        "Finishing twice is a no-op"
    );
}

#[tokio::test]
async fn test_completed_generation_keeps_its_document_under_the_id_of_the_run() {
    let mut state = AppState::default();
    let token = state.begin_generation();

    let document = state
        .complete_generation(&token, Arc::new("output".to_string()))
        .expect("the current generation may publish");

    assert!(!state.is_generating());
    assert_eq!(document.text(), "output");
    assert_eq!(
        state.generated_document(document.id()),
        Some(document.clone())
    );
    assert_eq!(state.generated_document(document.id() + 1), None);
    assert!(
        state
            .complete_generation(&token, Arc::new("again".to_string()))
            .is_none(),
        "a generation ends once"
    );
    assert_eq!(state.generated_document(document.id()), Some(document));
}

#[tokio::test]
async fn test_document_of_a_stale_generation_is_dropped() {
    let mut state = AppState::default();
    let first = state.begin_generation();
    let shown = state
        .complete_generation(&first, Arc::new("first".to_string()))
        .unwrap();

    // The document stays while the next generation is running ...
    let superseded = state.begin_generation();
    assert_eq!(state.generated_document(shown.id()), Some(shown.clone()));
    // ... and when that generation is cancelled or superseded.
    let current = state.begin_generation();
    assert!(state
        .complete_generation(&superseded, Arc::new("stale".to_string()))
        .is_none());
    assert_eq!(state.generated_document(shown.id()), Some(shown.clone()));
    state.cancel_current_generation();
    assert!(state
        .complete_generation(&current, Arc::new("cancelled".to_string()))
        .is_none());
    assert_eq!(state.generated_document(shown.id()), Some(shown.clone()));

    // The next document takes its place, under an id of its own.
    let next = state.begin_generation();
    let replaced = state
        .complete_generation(&next, Arc::new("next".to_string()))
        .unwrap();
    assert_ne!(replaced.id(), shown.id());
    assert_eq!(state.generated_document(shown.id()), None);
    assert_eq!(state.generated_document(replaced.id()), Some(replaced));
}

#[tokio::test]
async fn test_generated_document_outlives_the_directory_it_was_generated_from() {
    let mut state = AppState::default();
    let token = state.begin_generation();
    let document = state
        .complete_generation(&token, Arc::new("output".to_string()))
        .unwrap();

    // Another directory is opened, or the state is reset by an import: the UI
    // still shows the output, so it has to stay available for saving.
    state.reset_directory_state();
    assert_eq!(
        state.generated_document(document.id()),
        Some(document.clone())
    );
    let scan = state.begin_open(Path::new("/other"));
    assert!(state.enter_opened_directory(&scan, Path::new("/other")));
    assert_eq!(
        state.generated_document(document.id()),
        Some(document.clone())
    );

    // It is given up when it is told to.
    state.discard_generated_document();
    assert_eq!(state.generated_document(document.id()), None);
}

#[test]
fn test_debug_output_of_a_generated_document_does_not_contain_its_text() {
    let document = GeneratedDocument::for_test(7, "secret text");
    assert_eq!(
        format!("{document:?}"),
        "GeneratedDocument { id: 7, bytes: 11 }"
    );
}

#[tokio::test]
async fn test_cancelled_generation_cannot_be_finished_or_resurrected() {
    let mut state = AppState::default();
    let token = state.begin_generation();

    state.cancel_current_generation();

    assert!(!state.is_generating());
    assert!(token.is_cancelled());
    assert!(!state.finish_generation(&token));

    // The task handle arriving late must not resurrect the cancelled generation.
    let late_handle = create_dummy_task();
    let late_abort = late_handle.abort_handle();
    state.attach_generation_task(&token, late_handle);
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!state.has_generation_task());
    assert!(late_abort.is_finished());
}

#[tokio::test]
async fn test_reset_directory_state_cancels_scan_and_generation() {
    let mut state = AppState::default();
    let scan = state.begin_scan();
    let generation = state.begin_generation();

    state.reset_directory_state();

    assert!(scan.is_cancelled());
    assert!(generation.is_cancelled());
    assert!(!state.is_scanning());
    assert!(!state.is_generating());
}

#[tokio::test]
async fn test_cancel_generation_with_active_task() {
    // Arrange
    let mut state = AppState::default();
    let token = state.begin_generation();
    state.attach_generation_task(&token, create_dummy_task());

    // Act
    state.cancel_current_generation();
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Assert
    assert!(
        !state.has_generation_task(),
        "Generation task should be cleared"
    );
    assert!(!state.is_generating(), "is_generating should be false");
    assert!(
        token.is_cancelled(),
        "Generation cancellation flag should be set"
    );
}

#[tokio::test]
async fn test_cancel_generation_with_no_task_does_not_panic() {
    // Arrange
    let mut state = AppState::default();
    assert!(!state.is_generating(), "precondition");
    assert!(!state.has_generation_task(), "precondition");

    // Act & Assert
    // The test passes if this call does not panic.
    state.cancel_current_generation();
    assert!(!state.is_generating());
}
