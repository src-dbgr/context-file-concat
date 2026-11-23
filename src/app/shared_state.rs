//! The application state as it is shared between threads, and the one place that
//! locks it.
//!
//! # Panics
//!
//! A panic in a command or a background task is contained: the dispatcher and the
//! Tokio runtime catch it (the release build unwinds as well, see `Cargo.toml`), the
//! failing unit of work is logged and dropped, and the application keeps running.
//!
//! The standard library marks a mutex as *poisoned* when its holder panics, and every
//! later `lock()` then fails. Left alone, that would turn one contained panic into a
//! dead application: nothing could read or change the state any more. [`SharedState`]
//! therefore takes a poisoned lock over, logs that it did (once per incident) and
//! clears the mark.
//!
//! This is sound for [`AppState`]: it holds plain data without invariants that unsafe
//! code relies on. A command that panicked half-way may leave its own change
//! incomplete (the filtered list not yet recomputed, for example); the next command
//! and the next state update work on what is there, which is what the user sees.
//! Background runs are released by their guards (see `tasks.rs`), which lock the state
//! through this type too and therefore also work while a panic is unwinding.

use super::state::AppState;
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};

/// A handle to the application state. Cloning is cheap; all clones are the same state.
#[derive(Clone)]
pub struct SharedState(Arc<Mutex<AppState>>);

impl SharedState {
    pub fn new(state: AppState) -> Self {
        Self(Arc::new(Mutex::new(state)))
    }

    /// Locks the state, waiting for whoever holds it. Never fails: a lock poisoned by
    /// a panic is taken over, see the [module documentation](self).
    pub fn lock(&self) -> MutexGuard<'_, AppState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| self.take_over(poisoned.into_inner()))
    }

    /// Locks the state if nobody holds it right now; `None` otherwise. For callers
    /// that must not wait, such as the event loop while the window is being resized.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, AppState>> {
        match self.0.try_lock() {
            Ok(guard) => Some(guard),
            Err(TryLockError::Poisoned(poisoned)) => Some(self.take_over(poisoned.into_inner())),
            Err(TryLockError::WouldBlock) => None,
        }
    }

    fn take_over<'a>(&'a self, guard: MutexGuard<'a, AppState>) -> MutexGuard<'a, AppState> {
        tracing::error!(
            "A panic occurred while the application state was locked. Continuing with the \
             state as it was left."
        );
        self.0.clear_poison();
        guard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panics on another thread while holding the lock, after changing the state.
    fn panic_while_locked(state: &SharedState) {
        let state = state.clone();
        let result = std::thread::spawn(move || {
            let mut guard = state.lock();
            guard.current_path = "/changed/before/the/panic".to_string();
            panic!("deliberate panic while holding the state lock");
        })
        .join();
        assert!(result.is_err());
    }

    #[test]
    fn lock_takes_over_after_a_panic_and_clears_the_mark() {
        let state = SharedState::new(AppState::default());

        panic_while_locked(&state);
        assert!(state.0.is_poisoned());

        // What the panicking holder had done so far is what there is.
        assert_eq!(state.lock().current_path, "/changed/before/the/panic");
        // Clearing the mark is what makes this one incident (logged once), not a
        // condition every later lock has to deal with.
        assert!(!state.0.is_poisoned());
        state.lock().current_path.clear();
        assert_eq!(state.lock().current_path, "");
    }

    #[test]
    fn try_lock_takes_over_after_a_panic_as_well() {
        let state = SharedState::new(AppState::default());
        panic_while_locked(&state);

        let guard = state
            .try_lock()
            .expect("a poisoned lock is not a held lock");
        assert_eq!(guard.current_path, "/changed/before/the/panic");
        drop(guard);
        assert!(!state.0.is_poisoned());
    }

    #[test]
    fn try_lock_does_not_wait_for_a_held_lock() {
        let state = SharedState::new(AppState::default());

        let held = state.lock();
        assert!(state.try_lock().is_none());
        drop(held);

        assert!(state.try_lock().is_some());
    }

    #[test]
    fn clones_share_one_state() {
        let state = SharedState::new(AppState::default());
        let clone = state.clone();

        clone.lock().current_path = "/shared".to_string();

        assert_eq!(state.lock().current_path, "/shared");
    }
}
