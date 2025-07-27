//! Contains helper functions to reduce boilerplate code in other `app` modules.

use std::sync::{Arc, Mutex};

use super::events::UserEvent;
use super::proxy::EventProxy;
use super::state::AppState;
use super::view_model::generate_ui_state;

/// A helper function that locks the `AppState`, performs a mutation,
/// and then automatically sends a `StateUpdate` event to the UI.
///
/// This significantly reduces boilerplate in the command handlers.
pub fn with_state_and_notify<F, P: EventProxy>(
    state: &Arc<Mutex<AppState>>,
    proxy: &P,
    update_fn: F,
) where
    F: FnOnce(&mut AppState),
{
    let mut state_guard = state
        .lock()
        .expect("Mutex was poisoned. This should not happen.");

    // Execute the specific mutation logic
    update_fn(&mut state_guard);

    // Generate the new UI state and send the event
    let ui_state = generate_ui_state(&state_guard);
    let event = UserEvent::StateUpdate(Box::new(ui_state));

    proxy.send_event(event);
}
