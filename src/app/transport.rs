//! The production transport between backend tasks and the UI thread.
//!
//! Business code only knows the [`EventProxy`] trait and "fires" [`UserEvent`]s at it.
//! This module decides *how* those events travel to the tao event loop, and it is the
//! only place that knows about coalescing:
//!
//! ```text
//!  tokio workers / blocking threads            tao main thread
//!  --------------------------------            ---------------
//!  CoalescingProxy::send_event(event)
//!      |- outbox (FIFO, latest-wins tail) ---> EventOutbox::next_event()
//!      '- WakeSink::wake()  --- Wake --------> Event::UserEvent(Wake)
//! ```
//!
//! # Why coalesce
//!
//! A [`UserEvent::StateUpdate`] carries a full snapshot of the UI state, which the UI
//! thread serializes to JSON (potentially many megabytes) and evaluates in the WebView.
//! A snapshot fully supersedes every earlier snapshot, so when the backend produces
//! snapshots faster than the UI thread drains them, delivering the stale ones is pure
//! waste. The outbox therefore keeps at most one *undelivered* snapshot per run of
//! consecutive state updates: a newer snapshot replaces the pending one in place.
//!
//! # Ordering invariant
//!
//! The UI observes events in exactly the order they were sent, except that a snapshot
//! may be dropped when a newer snapshot was sent before any other event. Precisely:
//!
//! 1. The outbox is a FIFO queue; every mutation happens under one mutex, so the queue
//!    order is a linearization of the `send_event` calls.
//! 2. A snapshot only ever replaces the *tail* of the queue, and only if that tail is
//!    itself a snapshot. Nothing was sent between the two, so dropping the older one
//!    cannot reorder anything: the newer snapshot takes over the older one's position,
//!    which is still behind everything sent before either of them.
//! 3. A non-state event is always appended. It thereby "seals" a pending snapshot in
//!    front of it: later snapshots find a non-snapshot tail and are appended behind it.
//!
//! Hence a snapshot can never overtake, or be overtaken by, a non-state event, and an
//! older snapshot can never follow a newer one. For the sequence
//! `StateUpdate(is_generating = true)`, `ShowGeneratedContent`,
//! `StateUpdate(is_generating = false)` nothing is coalesced and all three arrive in
//! order; coalescing only kicks in for back-to-back snapshots.
//!
//! # Wake accounting
//!
//! The event loop is woken with a payload-free [`Wake`] token. Exactly one wake is
//! issued per *appended* entry and none for an in-place replacement, and the UI thread
//! takes exactly one entry per wake. Wakes are interchangeable, so they do not need to
//! be ordered relative to each other; a wake is always issued after its entry was
//! appended, so the UI thread never finds the outbox empty, and no entry is left
//! without a wake that will fetch it. Handling one entry per loop iteration also keeps
//! window events responsive during a burst.
//!
//! # What is deliberately not coalesced
//!
//! Only `StateUpdate` is latest-wins. [`UserEvent::ScanProgress`] is frequent but tiny
//! and cheap to deliver, and every other event is a one-shot notification that must not
//! be lost, so all of them pass through unchanged. A consequence is that a snapshot
//! followed by a progress tick is sealed (rule 3) and will be delivered.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tao::event_loop::EventLoopProxy;

use super::events::UserEvent;
use super::proxy::EventProxy;

/// Payload-free token telling the UI thread that one entry is waiting in the outbox.
///
/// This is the user-event type of the tao event loop; the actual [`UserEvent`]s never
/// travel through tao's queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wake;

/// Something that can nudge the UI thread. Abstracted so the coalescing logic can be
/// tested without a real event loop.
pub trait WakeSink: Send + Sync + 'static {
    /// Requests one call to [`EventOutbox::next_event`] on the UI thread.
    fn wake(&self);
}

impl WakeSink for EventLoopProxy<Wake> {
    fn wake(&self) {
        // Fire-and-forget: this only fails when the event loop is already gone.
        if let Err(e) = self.send_event(Wake) {
            tracing::warn!("Failed to wake the event loop: {}", e);
        }
    }
}

/// The queue shared by all senders and the UI thread.
#[derive(Default)]
struct Outbox {
    queue: Mutex<VecDeque<UserEvent>>,
}

impl Outbox {
    /// The critical sections below cannot leave the queue half-modified, so a poisoned
    /// lock (a panic elsewhere while holding it) is safe to recover from.
    fn lock(&self) -> MutexGuard<'_, VecDeque<UserEvent>> {
        self.queue.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Enqueues `event`; returns `true` if a new entry was appended (a wake is due) and
    /// `false` if it replaced a pending snapshot in place.
    fn push(&self, event: UserEvent) -> bool {
        let superseded;
        {
            let mut queue = self.lock();
            match queue.back_mut() {
                Some(tail @ UserEvent::StateUpdate(_))
                    if matches!(event, UserEvent::StateUpdate(_)) =>
                {
                    superseded = std::mem::replace(tail, event);
                }
                _ => {
                    queue.push_back(event);
                    return true;
                }
            }
        }
        // Free the stale snapshot (a whole tree) outside the critical section.
        drop(superseded);
        false
    }

    fn pop(&self) -> Option<UserEvent> {
        self.lock().pop_front()
    }
}

/// Creates the two ends of the transport: the sending [`CoalescingProxy`] handed to the
/// backend and the receiving [`EventOutbox`] owned by the UI thread.
pub fn channel<W: WakeSink>(waker: W) -> (CoalescingProxy<W>, EventOutbox) {
    let outbox = Arc::new(Outbox::default());
    (
        CoalescingProxy {
            outbox: outbox.clone(),
            waker: Arc::new(waker),
        },
        EventOutbox { outbox },
    )
}

/// The production [`EventProxy`]: queues events for the UI thread and coalesces
/// back-to-back state snapshots (see the module docs for the ordering argument).
pub struct CoalescingProxy<W: WakeSink> {
    outbox: Arc<Outbox>,
    waker: Arc<W>,
}

// Manual impl: cloning shares the handles and must not require `W: Clone`.
impl<W: WakeSink> Clone for CoalescingProxy<W> {
    fn clone(&self) -> Self {
        Self {
            outbox: self.outbox.clone(),
            waker: self.waker.clone(),
        }
    }
}

impl<W: WakeSink> EventProxy for CoalescingProxy<W> {
    fn send_event(&self, event: UserEvent) {
        // The wake is issued after the lock is released; wakes are interchangeable, so
        // only their count matters, not their order relative to other senders.
        if self.outbox.push(event) {
            self.waker.wake();
        }
    }
}

/// The receiving end, drained by the UI thread: one entry per received [`Wake`].
pub struct EventOutbox {
    outbox: Arc<Outbox>,
}

impl EventOutbox {
    /// Takes the oldest undelivered event. Call once for every [`Wake`].
    ///
    /// Returns `None` only for a wake without a matching entry, which the wake
    /// accounting rules out; callers should simply ignore it.
    pub fn next_event(&self) -> Option<UserEvent> {
        self.outbox.pop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::AppState;
    use crate::app::view_model::generate_ui_state;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;

    /// Counts wakes instead of driving an event loop.
    #[derive(Clone, Default)]
    struct CountingWaker(Arc<AtomicUsize>);

    impl CountingWaker {
        fn count(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl WakeSink for CountingWaker {
        fn wake(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Forwards wakes to a consumer thread, like the real event loop queue does.
    struct ChannelWaker(Mutex<mpsc::Sender<Wake>>);

    impl WakeSink for ChannelWaker {
        fn wake(&self) {
            let _ = self
                .0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(Wake);
        }
    }

    /// A snapshot tagged through its status message.
    fn snapshot(tag: &str) -> UserEvent {
        let mut ui_state = generate_ui_state(&AppState::default());
        ui_state.status_message = tag.to_string();
        UserEvent::StateUpdate(Box::new(ui_state))
    }

    /// Renders an event as a short label so sequences can be compared.
    fn label(event: UserEvent) -> String {
        match event {
            UserEvent::StateUpdate(s) => format!("state:{}", s.status_message),
            UserEvent::ShowError(msg) => format!("error:{msg}"),
            UserEvent::ScanProgress(_) => "progress".to_string(),
            other => format!("{other:?}"),
        }
    }

    /// Plays the UI thread: one `next_event` per issued wake.
    fn drain(waker: &CountingWaker, outbox: &EventOutbox, already_handled: usize) -> Vec<String> {
        (already_handled..waker.count())
            .map(|_| label(outbox.next_event().expect("one entry per wake")))
            .collect()
    }

    #[test]
    fn rapid_updates_are_delivered_once_with_the_newest_snapshot() {
        let waker = CountingWaker::default();
        let (proxy, outbox) = channel(waker.clone());

        for i in 0..50 {
            proxy.send_event(snapshot(&i.to_string()));
        }

        assert_eq!(waker.count(), 1, "a replaced snapshot must not wake again");
        assert_eq!(drain(&waker, &outbox, 0), vec!["state:49"]);
        assert!(outbox.next_event().is_none());
    }

    #[test]
    fn non_state_event_seals_the_pending_snapshot_in_front_of_it() {
        let waker = CountingWaker::default();
        let (proxy, outbox) = channel(waker.clone());

        proxy.send_event(snapshot("stale"));
        proxy.send_event(snapshot("generating"));
        proxy.send_event(UserEvent::ShowError("content".to_string()));
        proxy.send_event(snapshot("stale-after"));
        proxy.send_event(snapshot("done"));

        assert_eq!(
            drain(&waker, &outbox, 0),
            vec!["state:generating", "error:content", "state:done"]
        );
        assert!(outbox.next_event().is_none());
    }

    #[test]
    fn non_state_events_are_never_coalesced() {
        let waker = CountingWaker::default();
        let (proxy, outbox) = channel(waker.clone());

        for _ in 0..3 {
            let progress = generate_ui_state(&AppState::default()).scan_progress;
            proxy.send_event(UserEvent::ScanProgress(progress));
        }
        proxy.send_event(UserEvent::ShowError("a".to_string()));
        proxy.send_event(UserEvent::ShowError("b".to_string()));

        assert_eq!(
            drain(&waker, &outbox, 0),
            vec!["progress", "progress", "progress", "error:a", "error:b"]
        );
    }

    #[test]
    fn updates_after_a_delivery_are_delivered_again() {
        let waker = CountingWaker::default();
        let (proxy, outbox) = channel(waker.clone());

        proxy.send_event(snapshot("first"));
        assert_eq!(drain(&waker, &outbox, 0), vec!["state:first"]);

        proxy.send_event(snapshot("second-stale"));
        proxy.send_event(snapshot("second"));
        assert_eq!(waker.count(), 2);
        assert_eq!(drain(&waker, &outbox, 1), vec!["state:second"]);
        assert!(outbox.next_event().is_none());
    }

    #[test]
    fn clones_share_one_outbox() {
        let waker = CountingWaker::default();
        let (proxy, outbox) = channel(waker.clone());
        let clone = proxy.clone();

        proxy.send_event(snapshot("from-original"));
        clone.send_event(snapshot("from-clone"));

        assert_eq!(drain(&waker, &outbox, 0), vec!["state:from-clone"]);
    }

    #[test]
    fn concurrent_senders_never_lose_the_final_state_or_reorder() {
        const THREADS: usize = 8;
        const UPDATES: usize = 200;
        const ERROR_EVERY: usize = 50;

        let (wake_tx, wake_rx) = mpsc::channel();
        let (proxy, outbox) = channel(ChannelWaker(Mutex::new(wake_tx)));
        let template = generate_ui_state(&AppState::default());

        // The "UI thread": handles one entry per wake until every sender is gone.
        let consumer = thread::spawn(move || {
            let mut seen = Vec::new();
            while wake_rx.recv().is_ok() {
                seen.push(label(outbox.next_event().expect("one entry per wake")));
            }
            assert!(outbox.next_event().is_none(), "entry left without a wake");
            seen
        });

        let tagged = move |tag: String| {
            let mut ui_state = template.clone();
            ui_state.status_message = tag;
            UserEvent::StateUpdate(Box::new(ui_state))
        };
        let senders: Vec<_> = (0..THREADS)
            .map(|t| {
                let proxy = proxy.clone();
                let tagged = tagged.clone();
                thread::spawn(move || {
                    for i in 0..UPDATES {
                        proxy.send_event(tagged(format!("{t}:{i}")));
                        if i % ERROR_EVERY == 0 {
                            proxy.send_event(UserEvent::ShowError(format!("{t}:{i}")));
                        }
                    }
                })
            })
            .collect();
        for sender in senders {
            sender.join().expect("sender thread panicked");
        }
        proxy.send_event(tagged("final".to_string()));
        drop(proxy); // Closes the wake channel so the consumer finishes.

        let seen = consumer.join().expect("consumer thread panicked");

        assert_eq!(seen.last().map(String::as_str), Some("state:final"));

        // Non-state events are never dropped.
        let errors = seen.iter().filter(|l| l.starts_with("error:")).count();
        assert_eq!(errors, THREADS * UPDATES.div_ceil(ERROR_EVERY));

        // Per sender, everything arrives in send order: snapshots may be skipped but
        // never reordered, and snapshots never cross that sender's non-state events.
        for t in 0..THREADS {
            let prefix = format!("{t}:");
            let mut last_state: Option<usize> = None;
            let mut last_error: Option<usize> = None;
            for entry in &seen {
                let (kind, tag) = entry.split_once(':').expect("labelled entry");
                let Some(index) = tag.strip_prefix(&prefix) else {
                    continue;
                };
                let index: usize = index.parse().expect("numeric index");
                if kind == "state" {
                    assert!(last_state.is_none_or(|prev| prev < index));
                    // Sent after error `last_error`, so it must be a newer snapshot.
                    assert!(last_error.is_none_or(|err| err < index));
                    last_state = Some(index);
                } else {
                    assert!(last_error.is_none_or(|prev| prev < index));
                    // Snapshots sent after this error must not have arrived yet.
                    assert!(last_state.is_none_or(|state| state <= index));
                    last_error = Some(index);
                }
            }
            assert_eq!(last_error, Some(UPDATES - ERROR_EVERY));
        }
    }
}
