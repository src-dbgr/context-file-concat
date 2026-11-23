//! The snapshot of the state as it is sent to the UI, and the one way to send one.
//!
//! Two parts of the transport rely on the order in which snapshots are sent being
//! the order in which they were made: the coalescing (`transport.rs`: the snapshot
//! sent last is the newest, so an undelivered older one can be dropped) and the
//! correlation of commands and snapshots (`correlation.rs`: a snapshot sent after
//! the answer to a command shows what that command has done). That holds if a
//! snapshot is made and sent while the state is locked, and this module is what
//! makes it hold:
//!
//! * A [`Snapshot`] cannot be built anywhere else; [`UserEvent::StateUpdate`] takes
//!   nothing but a `Snapshot`.
//! * [`send_snapshot`] makes it from the state and sends it in one step. It takes
//!   the state by reference, and outside of tests a reference to the state of the
//!   application exists only while a guard of `SharedState` is alive.
//!
//! So there is no way to make a snapshot under the lock and send it later.

use std::ops::Deref;

use serde::Serialize;

use super::correlation::CommandId;
use super::events::UserEvent;
use super::proxy::EventProxy;
use super::state::AppState;
use super::view_model::{generate_ui_state, UiState};

/// A complete snapshot of what the UI shows, on its way to the UI.
#[derive(Debug, Serialize)]
#[serde(transparent)]
pub struct Snapshot(Box<UiState>);

impl Snapshot {
    /// Writes into the snapshot which command of the UI it accounts for. This is
    /// the transport's to say (see `correlation.rs`), on the way out.
    pub(super) fn account_for(&mut self, processed: CommandId) {
        self.0.last_processed_session = processed.session;
        self.0.last_processed_seq = processed.seq;
    }

    /// The command of the UI the snapshot accounts for.
    pub fn accounts_for(&self) -> CommandId {
        CommandId {
            session: self.0.last_processed_session,
            seq: self.0.last_processed_seq,
        }
    }

    /// What the snapshot holds.
    pub fn into_ui_state(self) -> Box<UiState> {
        self.0
    }

    /// A snapshot that holds `ui_state`, for tests of what happens to a snapshot
    /// after it has been sent.
    #[cfg(test)]
    pub(crate) fn for_test(ui_state: UiState) -> Self {
        Self(Box::new(ui_state))
    }
}

impl Deref for Snapshot {
    type Target = UiState;

    fn deref(&self) -> &UiState {
        &self.0
    }
}

/// Makes the snapshot of `state` and sends it to the UI.
///
/// This is the only way a snapshot leaves the host. `state` is the locked state of
/// the application, so the snapshot is sent before anything else can change the
/// state or send a snapshot of its own.
pub fn send_snapshot<P: EventProxy>(proxy: &P, state: &AppState) {
    proxy.send_event(UserEvent::StateUpdate(Snapshot(Box::new(
        generate_ui_state(state),
    ))));
}
