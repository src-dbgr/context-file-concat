//! Serial execution of IPC commands.
//!
//! The WebView delivers commands one after the other, and the UI relies on them taking
//! effect in that order: `setExpansion(true)` followed by `setExpansion(false)` must end
//! collapsed, two selection toggles must cancel out, and of two filter updates the later
//! one must win. Spawning one task per command on a multi-threaded runtime does not
//! guarantee that, so every command goes through a [`CommandDispatcher`] instead.
//!
//! This module knows nothing about individual commands. It only decides *when* a unit
//! of work runs; *what* runs is decided by the caller (see `dispatch_ipc_message` and
//! `dispatch_dropped_path`).
//!
//! The order is only worth something if a command performs every state change it
//! stands for before it returns (or suspends). A command that starts background work
//! therefore registers that work in the state itself and spawns nothing but the IO;
//! and it must not block the lane with IO of its own. (No command of the ordered lane
//! touches the file system: what needs the disk runs in a task or on the blocking
//! thread pool.)
//!
//! # Lanes
//!
//! * **Ordered lane** ([`CommandDispatcher::dispatch`]): commands start strictly in
//!   arrival order, and a command runs without interleaving until it either finishes
//!   or suspends for the first time (its first `.await` that is not immediately ready).
//!   Almost every command never suspends, so for those "start order" is "execution
//!   order".
//! * **Continuation lane**: a command that does suspend (a content search waiting for
//!   its worker thread) is moved here, so the ordered lane is free for the next command
//!   right away. Continuations finish one at a time, in the order their commands were
//!   dispatched; the result of an older search can therefore never overwrite a newer one.
//! * **Blocking lane** ([`CommandDispatcher::dispatch_blocking`]): work that blocks a
//!   thread for as long as the user likes, i.e. native modal dialogs. It runs on the
//!   blocking thread pool, one job at a time and in arrival order, so that at most one
//!   dialog is open.
//!
//!   A blocking command **starts after the ordered commands that were dispatched
//!   before it**: a dialog that is opened right after an edit sees the edit (the save
//!   dialog proposes the file name that has just been entered). This is a barrier, not
//!   a lock. The job waits until the ordered lane has *started* everything that was
//!   queued ahead of it, i.e. until each of those commands has finished or suspended;
//!   a suspended command (a content search) has made its own state changes by then
//!   and is not waited for. In the other direction there is no order: commands
//!   arriving after the blocking command, also while its dialog is open, are executed
//!   immediately, and whatever the dialog command does after the dialog closes happens
//!   after them.
//!
//! # Panics
//!
//! A panicking command is logged and dropped; the lanes keep running. This holds in
//! every build: the release profile unwinds too (`panic = "unwind"` in `Cargo.toml`),
//! which is what `catch_unwind` below needs to have any effect. A command that
//! panicked while it held the state lock does not take the application down with it
//! either, see [`SharedState`](super::shared_state::SharedState).

use std::any::Any;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::pin::Pin;
use std::task::Poll;

use tokio::sync::{mpsc, oneshot};

type Job = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A command of the blocking lane together with its barrier.
struct BlockingJob {
    /// Resolves once the ordered lane has started everything dispatched before this
    /// job (or has shut down).
    ordered_lane_caught_up: oneshot::Receiver<()>,
    run: Box<dyn FnOnce() + Send + 'static>,
}

/// Executes commands in the order in which they were dispatched.
///
/// Cloning is cheap; all clones feed the same lanes. The lanes stop once every clone
/// has been dropped and the queued work has been executed.
#[derive(Clone)]
pub struct CommandDispatcher {
    ordered: mpsc::UnboundedSender<Job>,
    blocking: mpsc::UnboundedSender<BlockingJob>,
}

impl CommandDispatcher {
    /// Starts the lanes on the current Tokio runtime.
    ///
    /// # Panics
    /// Panics when called outside of a Tokio runtime.
    pub fn new() -> Self {
        let (ordered, ordered_rx) = mpsc::unbounded_channel();
        let (continuations, continuations_rx) = mpsc::unbounded_channel();
        let (blocking, blocking_rx) = mpsc::unbounded_channel();

        tokio::spawn(run_ordered_lane(ordered_rx, continuations));
        tokio::spawn(run_continuation_lane(continuations_rx));
        tokio::spawn(run_blocking_lane(blocking_rx));

        Self { ordered, blocking }
    }

    /// Queues a command on the ordered lane. Never blocks.
    pub fn dispatch(&self, command: impl Future<Output = ()> + Send + 'static) {
        if self.ordered.send(Box::pin(command)).is_err() {
            tracing::error!("Command dropped: the ordered lane is no longer running.");
        }
    }

    /// Queues a command that blocks its thread (a native dialog) on the blocking lane.
    /// Never blocks the caller.
    ///
    /// The command starts once the ordered commands dispatched before this call have
    /// been started, and once the blocking commands dispatched before it have ended.
    pub fn dispatch_blocking(&self, command: impl FnOnce() + Send + 'static) {
        // The barrier is a marker command: when the ordered lane gets to it, everything
        // ahead of it has run up to its end or its first suspension. If the lane is
        // gone, the sender is dropped, which releases the job as well.
        let (caught_up, ordered_lane_caught_up) = oneshot::channel();
        let _ = self.ordered.send(Box::pin(async move {
            let _ = caught_up.send(());
        }));

        let job = BlockingJob {
            ordered_lane_caught_up,
            run: Box::new(command),
        };
        if self.blocking.send(job).is_err() {
            tracing::error!("Command dropped: the blocking lane is no longer running.");
        }
    }
}

impl Default for CommandDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Starts the commands in arrival order. A command that suspends is handed to the
/// continuation lane instead of being awaited here.
async fn run_ordered_lane(
    mut commands: mpsc::UnboundedReceiver<Job>,
    continuations: mpsc::UnboundedSender<Job>,
) {
    while let Some(mut command) = commands.recv().await {
        if run_until_first_suspension(&mut command).await.is_pending()
            && continuations.send(command).is_err()
        {
            tracing::error!("Command abandoned: the continuation lane is no longer running.");
        }
    }
}

/// Polls the command exactly once. `Ready` also covers a command that panicked.
async fn run_until_first_suspension(command: &mut Job) -> Poll<()> {
    std::future::poll_fn(|cx| {
        let first_poll = catch_unwind(AssertUnwindSafe(|| command.as_mut().poll(cx)));
        Poll::Ready(first_poll.unwrap_or_else(|panic| {
            log_panic(panic.as_ref());
            Poll::Ready(())
        }))
    })
    .await
}

/// Completes suspended commands one at a time.
async fn run_continuation_lane(mut continuations: mpsc::UnboundedReceiver<Job>) {
    while let Some(mut continuation) = continuations.recv().await {
        std::future::poll_fn(|cx| {
            catch_unwind(AssertUnwindSafe(|| continuation.as_mut().poll(cx))).unwrap_or_else(
                |panic| {
                    log_panic(panic.as_ref());
                    Poll::Ready(())
                },
            )
        })
        .await;
    }
}

/// Runs blocking commands one at a time on the blocking thread pool, each one after
/// its barrier on the ordered lane.
async fn run_blocking_lane(mut commands: mpsc::UnboundedReceiver<BlockingJob>) {
    while let Some(job) = commands.recv().await {
        // An error means that the ordered lane has shut down: nothing to wait for.
        let _ = job.ordered_lane_caught_up.await;
        if let Err(e) = tokio::task::spawn_blocking(job.run).await {
            tracing::error!("A blocking command failed: {e}");
        }
    }
}

fn log_panic(panic: &(dyn Any + Send)) {
    let message = panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string());
    tracing::error!("A command panicked: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// A log the commands of a test append to.
    #[derive(Clone, Default)]
    struct Journal(Arc<Mutex<Vec<String>>>);

    impl Journal {
        fn record(&self, entry: impl Into<String>) {
            self.0.lock().unwrap().push(entry.into());
        }

        fn entries(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    /// Dispatches a marker on the ordered lane and waits until it has run, i.e. until
    /// everything dispatched before it on that lane has been started.
    async fn ordered_lane_drained(dispatcher: &CommandDispatcher) {
        let (done, wait) = oneshot::channel();
        dispatcher.dispatch(async move {
            let _ = done.send(());
        });
        tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .expect("the ordered lane stalled")
            .expect("the marker command was dropped");
    }

    async fn eventually(journal: &Journal, expected_len: usize) -> Vec<String> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let entries = journal.entries();
                if entries.len() >= expected_len {
                    return entries;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .expect("the expected commands did not run")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn commands_sent_in_quick_succession_run_in_arrival_order() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();

        for i in 0..2_000 {
            let journal = journal.clone();
            dispatcher.dispatch(async move { journal.record(i.to_string()) });
        }
        ordered_lane_drained(&dispatcher).await;

        let expected: Vec<String> = (0..2_000).map(|i| i.to_string()).collect();
        assert_eq!(journal.entries(), expected);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 8)]
    async fn commands_never_overlap() {
        let dispatcher = CommandDispatcher::new();

        // A non-atomic read-modify-write with a widened race window: with one task per
        // command, updates would be lost.
        let counter = Arc::new(Mutex::new(0_u32));
        for _ in 0..200 {
            let counter = counter.clone();
            dispatcher.dispatch(async move {
                let seen = *counter.lock().unwrap();
                std::thread::yield_now();
                *counter.lock().unwrap() = seen + 1;
            });
        }
        ordered_lane_drained(&dispatcher).await;

        assert_eq!(*counter.lock().unwrap(), 200);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_slow_synchronous_command_does_not_reorder_later_ones() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();

        let slow = journal.clone();
        dispatcher.dispatch(async move {
            std::thread::sleep(Duration::from_millis(100));
            slow.record("slow");
        });
        for name in ["second", "third"] {
            let journal = journal.clone();
            dispatcher.dispatch(async move { journal.record(name) });
        }
        ordered_lane_drained(&dispatcher).await;

        assert_eq!(journal.entries(), ["slow", "second", "third"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_suspended_command_does_not_stall_the_ordered_lane() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (finish_search, search_finished) = oneshot::channel::<()>();

        let search = journal.clone();
        dispatcher.dispatch(async move {
            search.record("search: started");
            let _ = search_finished.await;
            search.record("search: finished");
        });
        let next = journal.clone();
        dispatcher.dispatch(async move { next.record("next") });
        ordered_lane_drained(&dispatcher).await;

        // The part before the suspension ran in order, the later command did not wait.
        assert_eq!(journal.entries(), ["search: started", "next"]);

        finish_search.send(()).unwrap();
        assert_eq!(
            eventually(&journal, 3).await,
            ["search: started", "next", "search: finished"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn suspended_commands_finish_in_dispatch_order() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (finish_first, first_finished) = oneshot::channel::<()>();
        let (finish_second, second_finished) = oneshot::channel::<()>();

        for (name, finished) in [("first", first_finished), ("second", second_finished)] {
            let journal = journal.clone();
            dispatcher.dispatch(async move {
                let _ = finished.await;
                journal.record(name);
            });
        }
        ordered_lane_drained(&dispatcher).await;

        // The newer one becomes ready first, but must not overtake the older one.
        finish_second.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(journal.entries().is_empty());

        finish_first.send(()).unwrap();
        assert_eq!(eventually(&journal, 2).await, ["first", "second"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_blocking_command_does_not_stall_the_ordered_lane() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (close_dialog, dialog_closed) = std::sync::mpsc::channel::<()>();

        let dialog = journal.clone();
        dispatcher.dispatch_blocking(move || {
            dialog_closed.recv().unwrap();
            dialog.record("dialog closed");
        });
        let next = journal.clone();
        dispatcher.dispatch(async move { next.record("command") });
        ordered_lane_drained(&dispatcher).await;
        assert_eq!(journal.entries(), ["command"]);

        close_dialog.send(()).unwrap();
        assert_eq!(eventually(&journal, 2).await, ["command", "dialog closed"]);
    }

    /// "Rename the output file, then save": the dialog must see the rename.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_blocking_command_starts_after_the_ordered_commands_before_it() {
        for _ in 0..20 {
            let dispatcher = CommandDispatcher::new();
            let journal = Journal::default();

            let slow = journal.clone();
            dispatcher.dispatch(async move {
                std::thread::sleep(Duration::from_millis(20));
                slow.record("edit");
            });
            let second = journal.clone();
            dispatcher.dispatch(async move { second.record("second edit") });
            let dialog = journal.clone();
            dispatcher.dispatch_blocking(move || dialog.record("dialog"));
            let later = journal.clone();
            dispatcher.dispatch(async move { later.record("later") });

            let entries = eventually(&journal, 4).await;
            assert_eq!(entries[..2], ["edit", "second edit"]);
            let position = |name: &str| entries.iter().position(|e| e == name).unwrap();
            assert!(position("dialog") > position("second edit"), "{entries:?}");
        }
    }

    /// The barrier waits for the commands before it to *start*: one that is suspended
    /// (a running content search) does not keep a dialog from opening.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_suspended_command_does_not_hold_back_a_blocking_command() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (finish_search, search_finished) = oneshot::channel::<()>();

        let search = journal.clone();
        dispatcher.dispatch(async move {
            search.record("search: started");
            let _ = search_finished.await;
            search.record("search: finished");
        });
        let dialog = journal.clone();
        dispatcher.dispatch_blocking(move || dialog.record("dialog"));

        assert_eq!(eventually(&journal, 2).await, ["search: started", "dialog"]);
        finish_search.send(()).unwrap();
        assert_eq!(eventually(&journal, 3).await[2], "search: finished");
    }

    /// The barrier of a second dialog command is reached while the first dialog is
    /// still open; the ordered lane is not held up by either of them.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn queued_blocking_commands_do_not_stall_the_ordered_lane() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (close_first, first_closed) = std::sync::mpsc::channel::<()>();

        let first = journal.clone();
        dispatcher.dispatch_blocking(move || {
            first.record("first: open");
            first_closed.recv().unwrap();
        });
        let second = journal.clone();
        dispatcher.dispatch_blocking(move || second.record("second: open"));
        assert_eq!(eventually(&journal, 1).await, ["first: open"]);

        let command = journal.clone();
        dispatcher.dispatch(async move { command.record("command") });
        ordered_lane_drained(&dispatcher).await;
        assert_eq!(journal.entries(), ["first: open", "command"]);

        close_first.send(()).unwrap();
        assert_eq!(
            eventually(&journal, 3).await,
            ["first: open", "command", "second: open"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn blocking_commands_run_one_at_a_time_in_arrival_order() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();
        let (close_first, first_closed) = std::sync::mpsc::channel::<()>();

        let first = journal.clone();
        dispatcher.dispatch_blocking(move || {
            first.record("first: open");
            first_closed.recv().unwrap();
            first.record("first: closed");
        });
        let second = journal.clone();
        dispatcher.dispatch_blocking(move || second.record("second: open"));

        assert_eq!(eventually(&journal, 1).await, ["first: open"]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(journal.entries(), ["first: open"]);

        close_first.send(()).unwrap();
        assert_eq!(
            eventually(&journal, 3).await,
            ["first: open", "first: closed", "second: open"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_panicking_command_does_not_stop_any_lane() {
        let dispatcher = CommandDispatcher::new();
        let journal = Journal::default();

        dispatcher.dispatch(async { panic!("panic before the first suspension") });
        dispatcher.dispatch(async {
            tokio::task::yield_now().await;
            panic!("panic after a suspension");
        });
        dispatcher.dispatch_blocking(|| panic!("panic in a blocking command"));

        let ordered = journal.clone();
        dispatcher.dispatch(async move { ordered.record("ordered") });
        ordered_lane_drained(&dispatcher).await;
        assert_eq!(journal.entries(), ["ordered"]);

        let suspended = journal.clone();
        dispatcher.dispatch(async move {
            tokio::task::yield_now().await;
            suspended.record("continuation");
        });
        assert_eq!(eventually(&journal, 2).await, ["ordered", "continuation"]);

        let blocking = journal.clone();
        dispatcher.dispatch_blocking(move || blocking.record("blocking"));
        assert_eq!(
            eventually(&journal, 3).await,
            ["ordered", "continuation", "blocking"]
        );
    }
}
