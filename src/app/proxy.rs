//! Defines an abstraction over the event sending mechanism.

use super::events::UserEvent;

/// A trait that abstracts the sending of user events.
/// This is "fire-and-forget" and doesn't return a result, simplifying its use.
///
/// Implementations must deliver events in the order they were sent. The production
/// implementation is [`CoalescingProxy`](super::transport::CoalescingProxy), which may
/// additionally drop a `StateUpdate` that is superseded by a newer one before delivery.
pub trait EventProxy: Send + Sync + Clone + 'static {
    fn send_event(&self, event: UserEvent);
}
