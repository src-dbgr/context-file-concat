//! Correlation of commands and snapshots.
//!
//! A command returns nothing; its effect arrives with a snapshot. So that the UI can
//! tell *which* snapshot that is, it numbers the commands it posts (the `seq` of the
//! envelope, see [`super::ipc`]), and every snapshot states the number of the last
//! command it accounts for: `last_processed_seq`.
//!
//! # What the number means
//!
//! A snapshot carries `last_processed_seq = n` if and only if
//!
//! * it was sent by command `n` itself while that command ran on the ordered lane
//!   (it is, or belongs to, the answer to `n`), or
//! * it was sent after that, or after command `n` had finished or suspended on the
//!   ordered lane without sending one,
//!
//! and no later command has got that far yet. Commands pass the ordered lane in the
//! order they were posted, so a snapshot with `last_processed_seq >= n` shows what
//! command `n` and every command before it have done there: everything a command
//! does before it finishes or suspends for the first time (see [`super::dispatch`]).
//!
//! What a command does *later* is not covered by its number, and comes with later
//! snapshots (which carry the number that is current then):
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
//!
//! A message that is refused (a malformed payload, an unknown command) is numbered
//! like any other and is processed by being answered with the unchanged snapshot. A
//! message without a number changes none.
//!
//! The number is the one of the *last* command, not the highest one ever seen: a UI
//! that starts anew (a reload) counts from 1 again, and the snapshots follow it.
//!
//! # How it is kept
//!
//! This is a concern of the transport, and no command knows about it. The ordered
//! lane tells a [`CommandProgress`] which numbered command it is running
//! ([`CommandProgress::while_answering`]), and the proxy the commands send their
//! events through ([`Correlated`]) writes the number into every snapshot on its way
//! out. A snapshot sent on the thread of the lane while a command runs there is
//! sent by that command.
//!
//! The number is written when a snapshot is *sent*. That is as good as when it is
//! made, because every snapshot is made and sent while the state is locked (which
//! the coalescing of the transport relies on as well: the snapshot sent last is the
//! newest). Of two snapshots the later one therefore never carries the smaller
//! number, and dropping the earlier one loses nothing.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::events::UserEvent;
use super::proxy::EventProxy;

/// The number the UI gives a command it posts. The UI counts from 1; 0 stands for
/// "no command yet".
pub type CommandSeq = u64;

thread_local! {
    /// The numbered command that is running on this thread, with the progress it
    /// reports to (its address: several dispatchers can exist side by side).
    static ANSWERING: Cell<Option<(usize, CommandSeq)>> = const { Cell::new(None) };
}

/// How far the ordered lane has got with the numbered commands.
#[derive(Debug, Default)]
pub struct CommandProgress {
    last_processed: AtomicU64,
}

impl CommandProgress {
    /// The number of the last command that was processed.
    pub fn last_processed(&self) -> CommandSeq {
        self.last_processed.load(Ordering::SeqCst)
    }

    /// Runs the part of command `seq` that the ordered lane executes: up to its end
    /// or its first suspension. Snapshots sent from within `run` are answers to the
    /// command; when `run` returns (or panics), the command is processed.
    pub fn while_answering<R>(&self, seq: CommandSeq, run: impl FnOnce() -> R) -> R {
        let _answering = Answering::begin(self, seq);
        run()
    }

    fn id(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// The number a snapshot that is sent now, on this thread, has to carry.
    fn seq_of_snapshot(&self) -> CommandSeq {
        if let Some((progress, seq)) = ANSWERING.get() {
            if progress == self.id() {
                // The command answers: from here on it counts as processed.
                self.last_processed.store(seq, Ordering::SeqCst);
            }
        }
        self.last_processed()
    }
}

/// Marks the thread as running a numbered command for as long as it lives.
struct Answering<'a> {
    progress: &'a CommandProgress,
    seq: CommandSeq,
    outer: Option<(usize, CommandSeq)>,
}

impl<'a> Answering<'a> {
    fn begin(progress: &'a CommandProgress, seq: CommandSeq) -> Self {
        let outer = ANSWERING.replace(Some((progress.id(), seq)));
        Self {
            progress,
            seq,
            outer,
        }
    }
}

impl Drop for Answering<'_> {
    fn drop(&mut self) {
        ANSWERING.set(self.outer);
        self.progress
            .last_processed
            .store(self.seq, Ordering::SeqCst);
    }
}

/// An [`EventProxy`] that writes `last_processed_seq` into every snapshot it sends.
/// Every other event passes unchanged.
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
            snapshot.last_processed_seq = self.progress.seq_of_snapshot();
        }
        self.proxy.send_event(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::app::view_model::generate_ui_state;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Sent(Arc<Mutex<Vec<UserEvent>>>);

    impl EventProxy for Sent {
        fn send_event(&self, event: UserEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    impl Sent {
        fn seqs(&self) -> Vec<CommandSeq> {
            let sent = self.0.lock().unwrap();
            sent.iter()
                .filter_map(|event| match event {
                    UserEvent::StateUpdate(snapshot) => Some(snapshot.last_processed_seq),
                    _ => None,
                })
                .collect()
        }
    }

    fn snapshot() -> UserEvent {
        UserEvent::StateUpdate(Box::new(generate_ui_state(&AppState::default())))
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
    fn a_snapshot_carries_zero_before_any_command() {
        let (progress, proxy, sent) = correlated();
        proxy.send_event(snapshot());
        assert_eq!(sent.seqs(), [0]);
        assert_eq!(progress.last_processed(), 0);
    }

    #[test]
    fn the_snapshots_of_a_command_carry_its_number_and_so_do_all_after_it() {
        let (progress, proxy, sent) = correlated();

        progress.while_answering(7, || {
            proxy.send_event(snapshot());
            proxy.send_event(snapshot());
        });
        proxy.send_event(snapshot());

        assert_eq!(sent.seqs(), [7, 7, 7]);
    }

    /// A background task sends a snapshot while a command is running on the lane:
    /// what it shows was made before the command changed anything.
    #[test]
    fn a_snapshot_of_another_thread_does_not_carry_the_number_of_a_running_command() {
        let (progress, proxy, sent) = correlated();
        progress.while_answering(1, || {});

        progress.while_answering(2, || {
            let task = proxy.clone();
            std::thread::spawn(move || task.send_event(snapshot()))
                .join()
                .unwrap();
            assert_eq!(sent.seqs(), [1]);

            // The command answers; a snapshot the task sends from now on follows
            // that answer.
            proxy.send_event(snapshot());
            let task = proxy.clone();
            std::thread::spawn(move || task.send_event(snapshot()))
                .join()
                .unwrap();
        });

        assert_eq!(sent.seqs(), [1, 2, 2]);
    }

    #[test]
    fn a_command_that_sends_nothing_is_processed_when_it_returns() {
        let (progress, proxy, sent) = correlated();

        progress.while_answering(3, || assert_eq!(progress.last_processed(), 0));
        assert_eq!(progress.last_processed(), 3);
        proxy.send_event(snapshot());

        assert_eq!(sent.seqs(), [3]);
    }

    #[test]
    fn a_command_that_panics_is_processed_and_leaves_no_mark_on_the_thread() {
        let (progress, proxy, sent) = correlated();

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            progress.while_answering(4, || panic!("deliberate panic in a command"));
        }));
        assert!(panicked.is_err());
        assert_eq!(progress.last_processed(), 4);

        // Nothing is running on this thread any more: the number stays.
        progress.last_processed.store(9, Ordering::SeqCst);
        proxy.send_event(snapshot());
        assert_eq!(sent.seqs(), [9]);
    }

    /// The number is the one of the last command, so a UI that was reloaded and
    /// counts from 1 again is followed.
    #[test]
    fn the_number_follows_a_ui_that_starts_counting_anew() {
        let (progress, proxy, sent) = correlated();

        progress.while_answering(500, || proxy.send_event(snapshot()));
        progress.while_answering(1, || proxy.send_event(snapshot()));
        proxy.send_event(snapshot());

        assert_eq!(sent.seqs(), [500, 1, 1]);
    }

    #[test]
    fn a_command_of_one_dispatcher_does_not_number_the_snapshots_of_another() {
        let (progress, _, _) = correlated();
        let (_, other_proxy, other_sent) = correlated();

        progress.while_answering(5, || other_proxy.send_event(snapshot()));

        assert_eq!(other_sent.seqs(), [0]);
    }

    #[test]
    fn other_events_pass_unchanged() {
        let (progress, proxy, sent) = correlated();
        progress.while_answering(2, || proxy.send_event(UserEvent::ShowError("no".into())));

        let sent = sent.0.lock().unwrap();
        assert!(matches!(&sent[0], UserEvent::ShowError(message) if message == "no"));
    }
}
