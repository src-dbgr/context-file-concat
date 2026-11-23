//! Contains helper functions to reduce boilerplate code in other `app` modules.

use super::proxy::EventProxy;
use super::shared_state::SharedState;
use super::snapshot::send_snapshot;
use super::state::AppState;

/// A helper function that locks the `AppState`, performs a mutation,
/// and then sends the snapshot of the state to the UI, all under one lock.
///
/// This significantly reduces boilerplate in the command handlers.
pub fn with_state_and_notify<F, P: EventProxy>(state: &SharedState, proxy: &P, update_fn: F)
where
    F: FnOnce(&mut AppState),
{
    let mut state_guard = state.lock();

    // Execute the specific mutation logic
    update_fn(&mut state_guard);

    send_snapshot(proxy, &state_guard);
}
