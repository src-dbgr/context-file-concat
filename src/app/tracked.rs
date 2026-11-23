//! A value that knows when it may have changed.
//!
//! Data derived from a part of the state (the rows of the tree from the expanded
//! directories) is cached and must be rebuilt exactly when that part changes. Instead
//! of asking every place that changes it to say so, the value itself hands out a new
//! revision whenever it is borrowed mutably.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};

/// Revisions are unique across the process: a revision identifies one content of one
/// value, also after the value has been replaced by another one.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

/// Wraps a value and gives it a revision that changes with every mutable access.
///
/// Reading goes through `Deref` and leaves the revision alone. A mutable borrow counts
/// as a change whether or not anything is written: that can cost a rebuild that was
/// not necessary, but never misses one.
#[derive(Debug)]
pub struct Tracked<T> {
    value: T,
    revision: u64,
}

impl<T> Tracked<T> {
    /// Identifies the current content.
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}

impl<T> From<T> for Tracked<T> {
    fn from(value: T) -> Self {
        Self {
            value,
            revision: next_revision(),
        }
    }
}

impl<T: Default> Default for Tracked<T> {
    fn default() -> Self {
        Self::from(T::default())
    }
}

impl<A, T: FromIterator<A>> FromIterator<A> for Tracked<T> {
    fn from_iter<I: IntoIterator<Item = A>>(items: I) -> Self {
        Self::from(T::from_iter(items))
    }
}

impl<T> Deref for Tracked<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

impl<T> DerefMut for Tracked<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.revision = next_revision();
        &mut self.value
    }
}

impl<T: PartialEq> PartialEq<T> for Tracked<T> {
    fn eq(&self, other: &T) -> bool {
        &self.value == other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn reading_keeps_the_revision_and_mutable_access_changes_it() {
        let mut set: Tracked<HashSet<u32>> = HashSet::from([1]).into();
        let initial = set.revision();

        assert!(set.contains(&1));
        assert_eq!(set.len(), 1);
        assert_eq!(set.revision(), initial);

        set.insert(2);
        let after_insert = set.revision();
        assert_ne!(after_insert, initial);

        set.clear();
        assert_ne!(set.revision(), after_insert);
    }

    #[test]
    fn a_replaced_value_never_reuses_a_revision() {
        let first: Tracked<HashSet<u32>> = HashSet::new().into();
        let second: Tracked<HashSet<u32>> = HashSet::new().into();
        assert_ne!(first.revision(), second.revision());
    }
}
