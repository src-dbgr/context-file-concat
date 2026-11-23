//! Correlation of commands and snapshots.
//!
//! A command returns nothing; its effect arrives with a snapshot. So that the UI can
//! tell *which* snapshot that is, it names the commands it posts (the `session` and
//! `seq` of the envelope, see [`super::ipc`]): `session` identifies the UI that
//! posts (a number it draws when it is loaded), `seq` counts its commands from 1.
//! Every snapshot states the last command it accounts for:
//! `last_processed_session` and `last_processed_seq`.
//!
//! # What the snapshot says
//!
//! A snapshot names command `n` of a session if and only if
//!
//! * it was sent by command `n` itself while that command ran on the ordered lane
//!   (it is, or belongs to, the answer to `n`), or
//! * it was sent after that, or after command `n` had finished or suspended on the
//!   ordered lane without sending one,
//!
//! and no later command has got that far yet. Commands pass the ordered lane in the
//! order they were posted. So for the UI: **from the first snapshot on that names
//! its session and a `seq >= n`, it is looking at the answer to command `n` or at
//! something newer.** What the host reports from then on is its decision about the
//! command, not a state from before it.
//!
//! This is a statement about the answer, not about everything a command sets in
//! motion. What a command does after its answer comes with later snapshots (which
//! name the command that is current then):
//!
//! * **A command that suspends** (`updateFilters` while its content search runs) is
//!   processed when it suspends. It has stored the filter texts and sent its first
//!   answer by then; the matches follow from the continuation lane.
//! * **A dialog command** is processed when the ordered lane reaches its place in
//!   the order, which is the moment it is handed to the blocking lane (every command
//!   posted before it has finished or suspended). Opening the dialog and whatever
//!   follows its closing happen after that, like the results of background work.
//! * **Background work** a command starts (a scan, a generation) is registered by
//!   the command; its results are not waited for.
//! * A command may also go on, on the lane, after it has sent its answer.
//!   `setConfigOption` for the case sensitivity answers with the new setting and
//!   then, if a content query is active, starts the search again; a snapshot a
//!   background task sends in between names the command and shows the setting, but
//!   not yet that a search is running.
//!
//! A message that is refused (a malformed payload, an unknown command) is named
//! like any other and is processed by being answered with the unchanged snapshot. A
//! message without a name changes nothing about what the snapshots say.
//!
//! The snapshot names the *last* command. A UI that is loaded anew (a reload) has
//! another `session` and counts from 1 again; snapshots that were made for the UI
//! before it name the session of that one, so the new UI does not take them for
//! answers to its own commands.
//!
//! # How it is kept
//!
//! This is a concern of the transport, and no command knows about it. The ordered
//! lane tells a [`CommandProgress`] which named command it is running
//! ([`CommandProgress::while_answering`]), and the proxy the commands send their
//! events through ([`Correlated`]) writes the name into every snapshot on its way
//! out. A snapshot sent on the thread of the lane while a command runs there is
//! sent by that command.
//!
//! The name is written when a snapshot is *sent*. That is as good as when it is
//! made: a snapshot can only be made and sent in one step, from the locked state
//! (see [`super::snapshot`]), so snapshots are sent in the order they are made. Of
//! two snapshots the later one therefore never names the earlier command, and the
//! transport dropping the earlier one loses nothing.

use std::cell::Cell;
use std::sync::{Arc, Mutex, PoisonError};

use super::events::UserEvent;
use super::proxy::EventProxy;

/// The number the UI gives a command it posts. The UI counts from 1; 0 stands for
/// "no command yet".
pub type CommandSeq = u64;

/// Identifies the UI that posts: a number it draws when it is loaded. 0 stands for
/// "no UI yet".
pub type SessionId = u64;

/// The name of a command: who posted it, and which of its commands it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandId {
    pub session: SessionId,
    pub seq: CommandSeq,
}

thread_local! {
    /// The named command that is running on this thread, with the progress it
    /// reports to (its address: several dispatchers can exist side by side).
    static ANSWERING: Cell<Option<(usize, CommandId)>> = const { Cell::new(None) };
}

/// How far the ordered lane has got with the named commands.
#[derive(Debug, Default)]
pub struct CommandProgress {
    last_processed: Mutex<CommandId>,
}

impl CommandProgress {
    /// The last command that was processed.
    pub fn last_processed(&self) -> CommandId {
        *self
            .last_processed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs the part of command `id` that the ordered lane executes: up to its end
    /// or its first suspension. Snapshots sent from within `run` are answers to the
    /// command; when `run` returns (or panics), the command is processed.
    pub fn while_answering<R>(&self, id: CommandId, run: impl FnOnce() -> R) -> R {
        let _answering = Answering::begin(self, id);
        run()
    }

    fn address(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    fn processed(&self, id: CommandId) {
        *self
            .last_processed
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = id;
    }

    /// The command a snapshot that is sent now, on this thread, has to name.
    fn command_of_snapshot(&self) -> CommandId {
        if let Some((progress, id)) = ANSWERING.get() {
            if progress == self.address() {
                // The command answers: from here on it counts as processed.
                self.processed(id);
            }
        }
        self.last_processed()
    }
}

/// Marks the thread as running a named command for as long as it lives.
struct Answering<'a> {
    progress: &'a CommandProgress,
    id: CommandId,
    outer: Option<(usize, CommandId)>,
}

impl<'a> Answering<'a> {
    fn begin(progress: &'a CommandProgress, id: CommandId) -> Self {
        let outer = ANSWERING.replace(Some((progress.address(), id)));
        Self {
            progress,
            id,
            outer,
        }
    }
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        ANSWERING.set(self.outer);
        self.progress.processed(self.id);
    }
}

/// An [`EventProxy`] that writes into every snapshot it sends which command the
/// snapshot accounts for. Every other event passes unchanged.
#[derive(Clone)]
pub struct Correlated<P> {
    proxy: P,
    progress: Arc<CommandProgress>,
}

impl<P> Correlated<P> {
    pub fn new(proxy: P, progress: Arc<CommandProgress>) -> Self {
        Self { proxy, progress }
    }
}

impl<P: EventProxy> EventProxy for Correlated<P> {
    fn send_event(&self, mut event: UserEvent) {
        if let UserEvent::StateUpdate(snapshot) = &mut event {
            snapshot.account_for(self.progress.command_of_snapshot());
        }
        self.proxy.send_event(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::snapshot::send_snapshot;
    use crate::app::state::AppState;

    #[derive(Clone, Default)]
    struct Sent(Arc<Mutex<Vec<UserEvent>>>);

    impl EventProxy for Sent {
        fn send_event(&self, event: UserEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Sent {
        /// The commands the snapshots that were sent name, as (session, seq).
        fn named(&self) -> Vec<(SessionId, CommandSeq)> {
            let sent = self.0.lock().unwrap();
            sent.iter()
                .filter_map(|event| match event {
                    UserEvent::StateUpdate(snapshot) => {
                        let id = snapshot.accounts_for();
                        Some((id.session, id.seq))
                    }
                    _ => None,
                })
                .collect()
        }
    }

    const UI: SessionId = 77;

    fn command(seq: CommandSeq) -> CommandId {
        CommandId { session: UI, seq }
    }

    fn send(proxy: &Correlated<Sent>) {
        send_snapshot(proxy, &AppState::default());
    }

    fn correlated() -> (Arc<CommandProgress>, Correlated<Sent>, Sent) {
        let progress = Arc::new(CommandProgress::default());
        let sent = Sent::default();
        (
            progress.clone(),
            Correlated::new(sent.clone(), progress),
            sent,
        )
    }

    #[test]
    fn a_snapshot_names_no_command_before_the_first_one() {
        let (progress, proxy, sent) = correlated();
        send(&proxy);
        assert_eq!(sent.named(), [(0, 0)]);
        assert_eq!(progress.last_processed(), CommandId::default());
    }

    #[test]
    fn the_snapshots_of_a_command_name_it_and_so_do_all_after_it() {
        let (progress, proxy, sent) = correlated();

        progress.while_answering(command(7), || {
            send(&proxy);
            send(&proxy);
        });
        send(&proxy);

        assert_eq!(sent.named(), [(UI, 7), (UI, 7), (UI, 7)]);
    }

    /// A background task sends a snapshot while a command is running on the lane:
    /// what it shows was made before the command changed anything.
    #[test]
    fn a_snapshot_of_another_thread_does_not_name_a_running_command() {
        let (progress, proxy, sent) = correlated();
        progress.while_answering(command(1), || {});

        progress.while_answering(command(2), || {
            let task = proxy.clone();
            std::thread::spawn(move || send(&task)).join().unwrap();
            assert_eq!(sent.named(), [(UI, 1)]);

            // The command answers; a snapshot the task sends from now on follows
            // that answer.
            send(&proxy);
            let task = proxy.clone();
            std::thread::spawn(move || send(&task)).join().unwrap();
        });

        assert_eq!(sent.named(), [(UI, 1), (UI, 2), (UI, 2)]);
    }

    #[test]
    fn a_command_that_sends_nothing_is_processed_when_it_returns() {
        let (progress, proxy, sent) = correlated();

        progress.while_answering(command(3), || {
            assert_eq!(progress.last_processed(), CommandId::default());
        });
        assert_eq!(progress.last_processed(), command(3));
        send(&proxy);

        assert_eq!(sent.named(), [(UI, 3)]);
    }

    #[test]
    fn a_command_that_panics_is_processed_and_leaves_no_mark_on_the_thread() {
        let (progress, proxy, sent) = correlated();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            progress.while_answering(command(4), || panic!("deliberate panic in a command"));
        }));
        assert!(panicked.is_err());
        assert_eq!(progress.last_processed(), command(4));

        // Nothing is running on this thread any more: what is processed stays.
        progress.processed(command(9));
        send(&proxy);
        assert_eq!(sent.named(), [(UI, 9)]);
    }

    /// The snapshots name the last command, whoever posted it: a UI that was
    /// loaded anew has another session and counts from 1 again.
    #[test]
    fn the_snapshots_follow_a_ui_that_was_loaded_anew() {
        let (progress, proxy, sent) = correlated();
        let reloaded = CommandId {
            session: UI + 1,
            seq: 1,
        };

        progress.while_answering(command(500), || send(&proxy));
        // Still on its way when the new UI starts: it names the old one.
        send(&proxy);
        progress.while_answering(reloaded, || send(&proxy));
        send(&proxy);

        assert_eq!(
            sent.named(),
            [(UI, 500), (UI, 500), (UI + 1, 1), (UI + 1, 1)]
        );
    }

    #[test]
    fn a_command_of_one_dispatcher_does_not_name_the_snapshots_of_another() {
        let (progress, _, _) = correlated();
        let (_, other_proxy, other_sent) = correlated();

        progress.while_answering(command(5), || send(&other_proxy));

        assert_eq!(other_sent.named(), [(0, 0)]);
    }

    #[test]
    fn other_events_pass_unchanged() {
        let (progress, proxy, sent) = correlated();
        progress.while_answering(command(2), || {
            proxy.send_event(UserEvent::ShowError("no".into()));
        });

        let sent = sent.0.lock().unwrap();
        assert!(matches!(&sent[0], UserEvent::ShowError(message) if message == "no"));
    }
}
