//! Fixed-capacity generational storage for live order nodes.
//!
//! `Arena` provides stable private handles over reusable slots. Removed slots
//! advance their generation before reuse, and a slot is retired if that
//! generation would overflow.

use std::fmt;

/// A private stable reference to one arena slot and its current generation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Handle {
    /// Slot offset, meaningful only in the arena that issued this handle.
    pub(crate) index: u32,
    /// Reuse epoch that must match the slot before its value can be accessed.
    pub(crate) generation: u64,
}

/// Startup failure when validating or reserving a fixed arena.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ArenaError {
    /// The requested slot count cannot be represented by private handles.
    CapacityTooLarge,
    /// The slot allocation could not be reserved at startup.
    AllocationFailed,
}

impl fmt::Display for ArenaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityTooLarge => formatter.write_str("arena capacity exceeds handle range"),
            Self::AllocationFailed => formatter.write_str("arena allocation could not be reserved"),
        }
    }
}

impl std::error::Error for ArenaError {}

/// Storage cell that is either live, chained as free, or permanently retired.
struct Slot<T> {
    /// Epoch advanced on removal before this slot can be reused.
    generation: u64,
    /// Next reusable slot; irrelevant while a value occupies the cell.
    next_free: Option<u32>,
    /// Live payload, or `None` for free and retired cells.
    value: Option<T>,
}

/// A fixed-capacity slot arena with generation-checked handles.
pub(crate) struct Arena<T> {
    /// Fixed-length backing storage, never grown after startup.
    slots: Vec<Slot<T>>,
    /// First reusable slot; `None` can mean full or generation-retired capacity.
    free_head: Option<u32>,
    /// Live payload count, excluding free and retired slots.
    len: usize,
}

impl<T> Arena<T> {
    /// Creates an arena with a deterministic initial free-slot chain.
    ///
    /// Slots are allocated up front and initially handed out in ascending
    /// index order. Recycled slots are pushed to the head of the free chain.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of slots addressable by this arena.
    ///
    /// # Returns
    ///
    /// The empty arena, `CapacityTooLarge` if an index cannot fit in a handle,
    /// or `AllocationFailed` when startup memory cannot be reserved.
    pub(crate) fn with_capacity(capacity: usize) -> Result<Self, ArenaError> {
        if capacity as u128 > (u32::MAX as u128) + 1 {
            return Err(ArenaError::CapacityTooLarge);
        }

        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| ArenaError::AllocationFailed)?;
        for index in 0..capacity {
            let next_free = (index + 1 < capacity).then_some((index + 1) as u32);
            slots.push(Slot {
                generation: 0,
                next_free,
                value: None,
            });
        }

        Ok(Self {
            slots,
            free_head: (capacity > 0).then_some(0),
            len: 0,
        })
    }

    /// Inserts a value and returns its private handle.
    ///
    /// # Arguments
    ///
    /// * `value` - Value to place in the next deterministic free slot.
    ///
    /// # Returns
    ///
    /// The handle on success, or the uninserted value if no reusable slot is
    /// available. Generation retirement can exhaust reuse before `len` reaches
    /// the fixed capacity. This operation does not allocate.
    pub(crate) fn insert(&mut self, value: T) -> Result<Handle, T> {
        let Some(index) = self.free_head else {
            return Err(value);
        };

        let slot = &mut self.slots[index as usize];
        self.free_head = slot.next_free.take();
        slot.value = Some(value);
        self.len += 1;

        Ok(Handle {
            index,
            generation: slot.generation,
        })
    }

    /// Returns a value when the handle still names its live slot generation.
    ///
    /// # Arguments
    ///
    /// * `handle` - Private slot reference to validate and resolve.
    ///
    /// # Returns
    ///
    /// A shared reference for a live handle, or `None` for a stale handle.
    pub(crate) fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_ref())
            .flatten()
    }

    /// Returns a mutable value when the handle still names its live slot generation.
    ///
    /// # Arguments
    ///
    /// * `handle` - Private slot reference to validate and resolve.
    ///
    /// # Returns
    ///
    /// A mutable reference for a live handle, or `None` for a stale handle.
    pub(crate) fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        (slot.generation == handle.generation)
            .then_some(slot.value.as_mut())
            .flatten()
    }

    /// Removes a value and invalidates every copy of its handle.
    ///
    /// If advancing the generation would overflow, the slot is retired instead
    /// of being reused, so an old handle can never become valid again.
    ///
    /// # Arguments
    ///
    /// * `handle` - Private slot reference to remove.
    ///
    /// # Returns
    ///
    /// The removed value, or `None` when the handle is stale or invalid.
    pub(crate) fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }

        let value = slot.value.take()?;
        self.len -= 1;
        if let Some(next_generation) = slot.generation.checked_add(1) {
            slot.generation = next_generation;
            slot.next_free = self.free_head;
            self.free_head = Some(handle.index);
        }
        Some(value)
    }

    /// Returns the number of live values.
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    /// Returns whether the arena has no live values.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the fixed number of slots, including retired slots.
    pub(crate) fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Reports whether a non-retired free slot can accept a new order.
    pub(crate) fn has_available(&self) -> bool {
        self.free_head.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::{Arena, ArenaError, Handle};

    #[test]
    fn capacity_and_initial_allocation_order_are_deterministic() {
        let mut arena = Arena::with_capacity(2).expect("small capacity is addressable");

        let first = arena.insert("first").expect("first slot available");
        let second = arena.insert("second").expect("second slot available");

        assert_eq!(first.index, 0);
        assert_eq!(second.index, 1);
        assert!(arena.insert("full").is_err());
        assert_eq!(arena.capacity(), 2);
        assert_eq!(arena.len(), 2);
        assert!(!arena.is_empty());
    }

    #[test]
    fn removal_rejects_stale_handles_after_slot_reuse() {
        let mut arena = Arena::with_capacity(1).expect("small capacity is addressable");
        let old = arena.insert(7).expect("slot available");

        assert_eq!(arena.remove(old), Some(7));
        assert_eq!(arena.get(old), None);
        let new = arena.insert(11).expect("removed slot is reusable");
        assert_eq!(old.index, new.index);
        assert_ne!(old.generation, new.generation);
        assert_eq!(arena.get(old), None);
        assert_eq!(arena.get(new), Some(&11));
    }

    #[test]
    fn invalid_and_duplicate_removals_do_not_change_state() {
        let mut arena = Arena::with_capacity(1).expect("small capacity is addressable");
        let handle = arena.insert(4).expect("slot available");
        let invalid = Handle {
            index: u32::MAX,
            generation: 0,
        };

        assert_eq!(arena.remove(invalid), None);
        assert_eq!(arena.remove(handle), Some(4));
        assert_eq!(arena.remove(handle), None);
        assert!(arena.is_empty());
    }

    #[test]
    fn zero_capacity_is_valid_but_never_accepts_values() {
        let mut arena = Arena::<u8>::with_capacity(0).expect("zero capacity is representable");

        assert_eq!(arena.capacity(), 0);
        assert!(arena.is_empty());
        assert_eq!(arena.insert(1), Err(1));
    }

    #[test]
    fn generation_overflow_retires_the_slot() {
        let mut arena = Arena::with_capacity(1).expect("small capacity is addressable");
        let original = arena.insert(3).expect("slot available");
        arena.slots[0].generation = u64::MAX;
        let last_valid = Handle {
            index: original.index,
            generation: u64::MAX,
        };

        assert_eq!(arena.remove(last_valid), Some(3));
        assert!(!arena.has_available());
        assert_eq!(arena.insert(5), Err(5));
        assert_eq!(arena.get(last_valid), None);
    }

    #[test]
    fn capacity_that_cannot_fit_a_handle_is_rejected() {
        if usize::BITS > 32 {
            let capacity = (u32::MAX as usize).saturating_add(2);
            assert_eq!(
                Arena::<u8>::with_capacity(capacity).err(),
                Some(ArenaError::CapacityTooLarge)
            );
        }
    }
}
