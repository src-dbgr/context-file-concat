//! Words the status of the application for the status bar.
//!
//! The state only records *what happened* ([`StatusNotice`]) and how far a scan has got
//! ([`ScanProgress`], as the running scan last reported it); how that reads is decided
//! here.

use crate::app::state::{AppState, StatusNotice};
use crate::core::ScanProgress;

/// The text for a notice.
pub(super) fn notice_text(notice: &StatusNotice) -> String {
    match notice {
        StatusNotice::Ready => "Ready.".to_string(),
        StatusNotice::ScanCancelled => "Scan cancelled.".to_string(),
        StatusNotice::IndexingComplete { visible_items } => {
            format!("Indexing complete. Found {visible_items} visible items.")
        }
        StatusNotice::ScanFailed { reason } => format!("Scan failed: {reason}"),
        StatusNotice::GenerationCancelled => "Generation cancelled.".to_string(),
    }
}

/// What the application is busy with or, failing that, what happened last: the path a
/// scan is working on if it has reported one, the latest notice otherwise.
fn activity_text(state: &AppState) -> String {
    let scanning_path = &state.scan_progress.current_scanning_path;
    if scanning_path.is_empty() {
        notice_text(&state.status)
    } else {
        scanning_path.clone()
    }
}

/// The scan progress as the UI receives it.
///
/// The UI contract has a single text field next to the counters,
/// `current_scanning_path`, which carries the activity text.
pub(super) fn scan_progress_for_ui(state: &AppState) -> ScanProgress {
    ScanProgress {
        files_scanned: state.scan_progress.files_scanned,
        large_files_skipped: state.scan_progress.large_files_skipped,
        current_scanning_path: activity_text(state),
    }
}

/// The line shown in the status bar: while a scan is running how far it has got (as of
/// this snapshot; the live counters are sent as `ScanProgress` events), otherwise what
/// happened last.
pub(super) fn status_message(state: &AppState) -> String {
    if !state.is_scanning() {
        return activity_text(state);
    }
    let progress = &state.scan_progress;
    let counters = format!(
        "Scanning... {} files processed. {} large files skipped",
        progress.files_scanned, progress.large_files_skipped
    );
    // A scan that has not reported anything yet has no path to show; the notice of
    // whatever happened before it does not belong to it.
    if progress.current_scanning_path.is_empty() {
        format!("{counters}.")
    } else {
        format!("{counters} ({})", progress.current_scanning_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_notice_has_its_text() {
        let cases = [
            (StatusNotice::Ready, "Ready."),
            (StatusNotice::ScanCancelled, "Scan cancelled."),
            (
                StatusNotice::IndexingComplete { visible_items: 12 },
                "Indexing complete. Found 12 visible items.",
            ),
            (
                StatusNotice::ScanFailed {
                    reason: "disk on fire".to_string(),
                },
                "Scan failed: disk on fire",
            ),
            (StatusNotice::GenerationCancelled, "Generation cancelled."),
        ];

        for (notice, expected) in cases {
            assert_eq!(notice_text(&notice), expected);
        }
    }

    #[test]
    fn idle_state_shows_the_latest_notice() {
        let mut state = AppState::default();
        assert_eq!(status_message(&state), "Ready.");
        assert_eq!(scan_progress_for_ui(&state).current_scanning_path, "Ready.");

        state.status = StatusNotice::IndexingComplete { visible_items: 3 };
        state.scan_progress.files_scanned = 7;
        state.scan_progress.large_files_skipped = 1;

        let expected = "Indexing complete. Found 3 visible items.";
        assert_eq!(status_message(&state), expected);
        let progress = scan_progress_for_ui(&state);
        assert_eq!(progress.current_scanning_path, expected);
        assert_eq!(
            (progress.files_scanned, progress.large_files_skipped),
            (7, 1)
        );
    }

    #[test]
    fn running_scan_shows_the_progress_it_has_reported() {
        let mut state = AppState::default();
        state.status = StatusNotice::ScanCancelled;
        let scan = state.begin_scan();

        // Nothing reported yet: no path, and no notice from before the scan.
        assert_eq!(
            status_message(&state),
            "Scanning... 0 files processed. 0 large files skipped."
        );

        state.record_scan_progress(
            &scan,
            ScanProgress {
                files_scanned: 40,
                large_files_skipped: 2,
                current_scanning_path: "/project/src/main.rs".to_string(),
            },
        );

        assert_eq!(
            status_message(&state),
            "Scanning... 40 files processed. 2 large files skipped (/project/src/main.rs)"
        );
        let progress = scan_progress_for_ui(&state);
        assert_eq!(progress.current_scanning_path, "/project/src/main.rs");
        assert_eq!(
            (progress.files_scanned, progress.large_files_skipped),
            (40, 2)
        );
    }

    #[test]
    fn finished_scan_shows_its_outcome_and_keeps_its_counters() {
        let mut state = AppState::default();
        let scan = state.begin_scan();
        state.record_scan_progress(
            &scan,
            ScanProgress {
                files_scanned: 40,
                large_files_skipped: 2,
                current_scanning_path: "/project/src/main.rs".to_string(),
            },
        );
        state.finish_scan(&scan);
        state.status = StatusNotice::IndexingComplete { visible_items: 38 };

        let expected = "Indexing complete. Found 38 visible items.";
        assert_eq!(status_message(&state), expected);
        let progress = scan_progress_for_ui(&state);
        assert_eq!(progress.current_scanning_path, expected);
        assert_eq!(
            (progress.files_scanned, progress.large_files_skipped),
            (40, 2)
        );
    }
}
