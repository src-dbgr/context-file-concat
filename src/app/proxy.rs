//! Defines an abstraction over the event sending mechanism.

use super::events::UserEvent;
use tao::event_loop::EventLoopProxy;

/// A trait that abstracts the sending of user events.
/// This is "fire-and-forget" and doesn't return a result, simplifying its use.
pub trait EventProxy: Send + Sync + Clone + 'static {
    fn send_event(&self, event: UserEvent);
}

/// Implement the trait for the real `tao::event_loop::EventLoopProxy`.
impl EventProxy for EventLoopProxy<UserEvent> {
    fn send_event(&self, event: UserEvent) {
        // The real proxy can return an error, but for our app's logic,
        // we'll treat it as fire-and-forget. We log the error if it occurs.
        if let Err(e) = self.send_event(event) {
            tracing::warn!("Failed to send event to event loop: {}", e);
        }
    }
}
