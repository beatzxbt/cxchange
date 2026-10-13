//! Fixed-capacity order storage, FIFO price levels, and ordered price lookup.
//!
//! `Book` owns stable arena entries and links orders within each price level.
//! Bid and ask levels use `BTreeMap`; its node allocations are part of this
//! correctness-first baseline and are not claimed to be real-time bounded.
//!
//! # Internal contract
//!
//! Every live order has one arena entry, one public-ID index entry, and one
//! position in a doubly linked FIFO at its `(side, price)`. Active levels are
//! nonempty, their counts and quantities equal their linked orders, and their
//! FIFO sequences strictly increase. The engine supplies validated positive
//! values, fresh identities, and account ownership checks; this module handles
//! storage and resulting-state capacity, not command authorization.
//!
//! Public-ID resolution costs `O(log N)` for `N` live orders. Once resolved,
//! unlinking updates neighboring handles without scanning the level; price-map
//! lookup/removal still costs `O(log L)` for `L` levels. Mutations assume these
//! invariants hold. Internal corruption is not a recoverable trading rejection.

use std::collections::BTreeMap;

use crate::arena::{Arena, ArenaError, Handle};
use crate::types::{
    EngineConfig, LevelDelta, MatchingError, Order, OrderId, Price, Quantity, Side,
};

/// One live order and its neighbors within a single side/price FIFO.
struct Entry {
    /// Remaining order state; its side and price identify the containing level.
    order: Order,
    /// Older sibling, or `None` at the level's head.
    previous: Option<Handle>,
    /// Newer sibling, or `None` at the level's tail.
    next: Option<Handle>,
}

/// FIFO endpoints and cached aggregates; empty instances exist only during insertion.
#[derive(Default)]
struct Level {
    /// Oldest live maker, consumed first by matching.
    head: Option<Handle>,
    /// Newest live maker, followed by the next append.
    tail: Option<Handle>,
    /// Number of nodes reachable from `head`.
    count: usize,
    /// Sum of the linked orders' remaining quantities in lot-size quanta.
    total_size: u128,
}

/// Immutable aggregate information for one active price level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LevelInfo {
    /// Price represented by the level.
    pub(crate) price: Price,
    /// Side represented by the level.
    pub(crate) side: Side,
    /// Total remaining quantity at the level.
    pub(crate) total_size: u128,
    /// Number of live orders at the level.
    pub(crate) order_count: usize,
}

impl LevelInfo {
    /// Converts this captured aggregate into a public response value.
    pub(crate) fn delta(self) -> LevelDelta {
        LevelDelta {
            price: self.price,
            size: self.total_size,
            side: self.side,
            order_count: self.order_count,
        }
    }
}

/// One symbol's fixed-arena order store and ordered price levels.
pub(crate) struct Book {
    /// Preallocated nodes; private handles remain stable while an order is live.
    orders: Arena<Entry>,
    /// Exchange identities resolve independently of arena slot reuse.
    order_index: BTreeMap<OrderId, Handle>,
    /// Ascending keys; best bids are read from the back.
    bids: BTreeMap<Price, Level>,
    /// Ascending keys; best asks are read from the front.
    asks: BTreeMap<Price, Level>,
    /// Combined nonempty bid and ask level count.
    active_levels: usize,
    /// Logical order limit for each `(side, price)`.
    max_orders_per_level: usize,
    /// Logical level limit across both sides.
    max_levels: usize,
}

impl Book {
    /// Creates an empty book with capacity derived from its validated config.
    ///
    /// The caller supplies `max_live_orders = config.validate()?`. Arena
    /// reservation can return `AllocationFailed`; ordered maps allocate later
    /// during insertion and retain the documented process-failure boundary.
    pub(crate) fn new(
        config: &EngineConfig,
        max_live_orders: usize,
    ) -> Result<Self, MatchingError> {
        let orders = Arena::with_capacity(max_live_orders).map_err(|error| match error {
            ArenaError::CapacityTooLarge => MatchingError::InvalidConfig,
            ArenaError::AllocationFailed => MatchingError::AllocationFailed,
        })?;
        Ok(Self {
            orders,
            order_index: BTreeMap::new(),
            bids: BTreeMap::new(),
            asks: BTreeMap::new(),
            active_levels: 0,
            max_orders_per_level: config.max_orders_per_level,
            max_levels: config.max_levels,
        })
    }

    /// Returns the number of live orders across both sides.
    pub(crate) fn len(&self) -> usize {
        self.orders.len()
    }

    /// Returns the highest bid, or `None` for an empty bid side.
    pub(crate) fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(price, _)| *price)
    }

    /// Returns the lowest ask, or `None` for an empty ask side.
    pub(crate) fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(price, _)| *price)
    }

    /// Copies a level's aggregate, returning `None` when that side/price is absent.
    pub(crate) fn level_info(&self, side: Side, price: Price) -> Option<LevelInfo> {
        let level = self.levels(side).get(&price)?;
        Some(LevelInfo {
            price,
            side,
            total_size: level.total_size,
            order_count: level.count,
        })
    }

    /// Allocates up to `limit` aggregates in best-to-worst order for one side.
    ///
    /// A zero limit returns no levels. This is a query convenience, not a
    /// preallocated command-path operation.
    pub(crate) fn depth(&self, side: Side, limit: usize) -> Vec<LevelInfo> {
        if limit == 0 {
            return Vec::new();
        }
        let mut result = Vec::with_capacity(limit.min(self.active_levels));
        self.visit_levels(side, |info| {
            result.push(info);
            result.len() < limit
        });
        result
    }

    /// Resolves a public ID through the ordered index and validates its handle.
    pub(crate) fn get_order(&self, order_id: OrderId) -> Option<&Order> {
        let handle = *self.order_index.get(&order_id)?;
        Some(&self.orders.get(handle)?.order)
    }

    /// Returns the indexed private handle without dereferencing its arena slot.
    pub(crate) fn handle_for(&self, order_id: OrderId) -> Option<Handle> {
        self.order_index.get(&order_id).copied()
    }

    /// Resolves a live generation-checked handle; stale or absent slots return `None`.
    pub(crate) fn order_for_handle(&self, handle: Handle) -> Option<&Order> {
        self.orders.get(handle).map(|entry| &entry.order)
    }

    /// Checks whether a maker price meets or passes the best opposite price.
    ///
    /// Equality crosses; an empty opposite side never crosses.
    pub(crate) fn would_cross(&self, side: Side, price: Price) -> bool {
        match side {
            Side::Buy => self.best_ask().is_some_and(|ask| price >= ask),
            Side::Sell => self.best_bid().is_some_and(|bid| price <= bid),
        }
    }

    /// Preflights level count, order count, aggregate arithmetic, and arena availability.
    ///
    /// Returns `CapacityExceeded` without mutation if any limit fails, including
    /// when generation retirement leaves no reusable slot. Price/size validity,
    /// crossing, ownership, and client-ID uniqueness are checked by the engine.
    pub(crate) fn can_add(
        &self,
        side: Side,
        price: Price,
        size: Quantity,
    ) -> Result<(), MatchingError> {
        let levels = self.levels(side);
        if let Some(level) = levels.get(&price) {
            if level.count >= self.max_orders_per_level
                || level.total_size.checked_add(u128::from(size.0)).is_none()
            {
                return Err(MatchingError::CapacityExceeded);
            }
        } else if self.active_levels >= self.max_levels {
            return Err(MatchingError::CapacityExceeded);
        }
        if self.orders.len() >= self.orders.capacity() || !self.orders.has_available() {
            return Err(MatchingError::CapacityExceeded);
        }
        Ok(())
    }

    /// Checks replacement capacity after logically removing the original order.
    ///
    /// Removing the last order releases a level; a same-price replacement first
    /// subtracts its own count and quantity. No spare arena slot is needed.
    /// Returns `OrderNotFound`, `InvalidSnapshot` for an inconsistent source
    /// level, or `CapacityExceeded`. It performs no mutation.
    pub(crate) fn can_amend(
        &self,
        order_id: OrderId,
        new_price: Price,
        new_size: Quantity,
    ) -> Result<(), MatchingError> {
        let old_handle = self
            .handle_for(order_id)
            .ok_or(MatchingError::OrderNotFound)?;
        let old = self
            .order_for_handle(old_handle)
            .ok_or(MatchingError::OrderNotFound)?;
        let old_info = self
            .level_info(old.side, old.price)
            .ok_or(MatchingError::InvalidSnapshot)?;
        let old_level_will_empty = old_info.order_count == 1;
        let same_level = old.price == new_price;
        let target = self.levels(old.side).get(&new_price);
        let target_count_after_remove =
            target.map_or(0, |level| level.count - usize::from(same_level));
        if target_count_after_remove >= self.max_orders_per_level {
            return Err(MatchingError::CapacityExceeded);
        }

        let target_total_after_remove = target.map_or(0, |level| {
            level.total_size.saturating_sub(if same_level {
                u128::from(old.size.0)
            } else {
                0
            })
        });
        if target_total_after_remove
            .checked_add(u128::from(new_size.0))
            .is_none()
        {
            return Err(MatchingError::CapacityExceeded);
        }

        let target_exists_after_remove = target_count_after_remove > 0;
        let resulting_levels = self
            .active_levels
            .saturating_sub(usize::from(old_level_will_empty))
            + usize::from(!target_exists_after_remove);
        if resulting_levels > self.max_levels {
            return Err(MatchingError::CapacityExceeded);
        }
        Ok(())
    }

    /// Inserts a fresh public ID at its level's FIFO tail and returns the final aggregate.
    ///
    /// The caller validates domain values and increasing FIFO priority. Capacity
    /// and duplicate exchange IDs are checked here before insertion; duplicate
    /// IDs return `CapacityExceeded`. Map allocations remain system failures.
    pub(crate) fn add_order(&mut self, order: Order) -> Result<LevelInfo, MatchingError> {
        self.can_add(order.side, order.price, order.size)?;
        if self.order_index.contains_key(&order.order_id) {
            return Err(MatchingError::CapacityExceeded);
        }

        let handle = self
            .orders
            .insert(Entry {
                order,
                previous: None,
                next: None,
            })
            .map_err(|_| MatchingError::CapacityExceeded)?;
        let stored_order = &self.orders.get(handle).expect("inserted arena entry").order;
        let side = stored_order.side;
        let price = stored_order.price;
        let order_id = stored_order.order_id;
        let size = stored_order.size;

        if !self.levels(side).contains_key(&price) {
            self.levels_mut(side).insert(price, Level::default());
            self.active_levels += 1;
        }
        self.append_to_level(side, price, handle, size);
        self.order_index.insert(order_id, handle);
        Ok(self
            .level_info(side, price)
            .expect("new order creates active level"))
    }

    /// Unlinks a live order, frees its arena slot, and removes an emptied level.
    ///
    /// Returns the removed order and the final level aggregate, including a zero
    /// aggregate for a removed level. An unknown public ID returns `OrderNotFound`;
    /// broken internal handles return `InvalidSnapshot`.
    pub(crate) fn remove_order(
        &mut self,
        order_id: OrderId,
    ) -> Result<(Order, LevelInfo), MatchingError> {
        let handle = self
            .order_index
            .get(&order_id)
            .copied()
            .ok_or(MatchingError::OrderNotFound)?;
        let entry = self
            .orders
            .get(handle)
            .ok_or(MatchingError::InvalidSnapshot)?;
        let side = entry.order.side;
        let price = entry.order.price;
        let size = entry.order.size;
        self.unlink_from_level(side, price, handle, size);
        self.order_index.remove(&order_id);
        let removed = self
            .orders
            .remove(handle)
            .ok_or(MatchingError::InvalidSnapshot)?;
        let info = self.level_info(side, price).unwrap_or(LevelInfo {
            side,
            price,
            total_size: 0,
            order_count: 0,
        });
        Ok((removed.order, info))
    }

    /// Reuses the old slot while replacing its public ID and appending at the tail.
    ///
    /// The engine must preserve side, account, and client ID and supply a fresh
    /// exchange ID/priority. Returns the old order, the source aggregate after
    /// removal, and the destination aggregate after insertion. Even for the
    /// same price, both intermediate and final aggregates must be emitted in
    /// that order. Capacity checks precede unlinking; valid internal links are
    /// a precondition throughout commit.
    pub(crate) fn replace_order(
        &mut self,
        old_order_id: OrderId,
        new_order: Order,
    ) -> Result<(Order, LevelInfo, LevelInfo), MatchingError> {
        self.can_amend(old_order_id, new_order.price, new_order.size)?;
        if self.order_index.contains_key(&new_order.order_id) {
            return Err(MatchingError::CapacityExceeded);
        }

        let handle = self
            .order_index
            .get(&old_order_id)
            .copied()
            .ok_or(MatchingError::OrderNotFound)?;
        let old_entry = self
            .orders
            .get(handle)
            .ok_or(MatchingError::InvalidSnapshot)?;
        let old_side = old_entry.order.side;
        let old_price = old_entry.order.price;
        let old_size = old_entry.order.size;
        self.unlink_from_level(old_side, old_price, handle, old_size);
        self.order_index.remove(&old_order_id);
        let old_info = self.level_info(old_side, old_price).unwrap_or(LevelInfo {
            side: old_side,
            price: old_price,
            total_size: 0,
            order_count: 0,
        });
        let old_order = {
            let entry = self
                .orders
                .get_mut(handle)
                .ok_or(MatchingError::InvalidSnapshot)?;
            std::mem::replace(&mut entry.order, new_order)
        };

        let (new_side, new_price, new_id, new_size) = {
            let order = &self
                .orders
                .get(handle)
                .expect("replacement preserves arena slot")
                .order;
            (order.side, order.price, order.order_id, order.size)
        };
        if !self.levels(new_side).contains_key(&new_price) {
            self.levels_mut(new_side)
                .insert(new_price, Level::default());
            self.active_levels += 1;
        }
        self.append_to_level(new_side, new_price, handle, new_size);
        self.order_index.insert(new_id, handle);

        let new_info = self
            .level_info(new_side, new_price)
            .expect("replacement creates destination level");
        Ok((old_order, old_info, new_info))
    }

    /// Commits a previously planned fill against a live maker.
    ///
    /// The caller supplies a positive size no greater than the maker's remaining
    /// size. A full fill removes and returns the order; a partial fill returns
    /// `None` and retains its FIFO position. Invalid handles also return `None`,
    /// so this is not a validation API: planning must establish handle validity.
    pub(crate) fn apply_fill(&mut self, handle: Handle, fill_size: Quantity) -> Option<Order> {
        let entry = self.orders.get(handle)?;
        let order_id = entry.order.order_id;
        let side = entry.order.side;
        let price = entry.order.price;
        let current_size = entry.order.size;
        if fill_size.0 >= current_size.0 {
            self.remove_order(order_id).ok().map(|(order, _)| order)
        } else {
            let entry = self.orders.get_mut(handle)?;
            entry.order.size = Quantity(current_size.0 - fill_size.0);
            let level = self.levels_mut(side).get_mut(&price)?;
            level.total_size -= u128::from(fill_size.0);
            None
        }
    }

    /// Visits makers in best-price/FIFO order up to an inclusive taker limit.
    ///
    /// `maker_side` names the opposite side being consumed: asks at or below the
    /// limit, or bids at or above it. The callback receives price, private handle,
    /// and a shared order; returning `false` stops the entire traversal. No book
    /// mutation or result allocation occurs here. A broken handle stops traversal.
    pub(crate) fn visit_eligible(
        &self,
        maker_side: Side,
        limit_price: Price,
        mut visit: impl FnMut(Price, Handle, &Order) -> bool,
    ) {
        let mut visit_level = |price: Price, level: &Level| {
            let mut cursor = level.head;
            while let Some(handle) = cursor {
                let Some(entry) = self.orders.get(handle) else {
                    return false;
                };
                cursor = entry.next;
                if !visit(price, handle, &entry.order) {
                    return false;
                }
            }
            true
        };

        match maker_side {
            Side::Sell => {
                for (&price, level) in self.asks.range(..=limit_price) {
                    if !visit_level(price, level) {
                        break;
                    }
                }
            }
            Side::Buy => {
                for (&price, level) in self.bids.range(limit_price..).rev() {
                    if !visit_level(price, level) {
                        break;
                    }
                }
            }
        }
    }

    /// Visits aggregates in best-to-worst order until the callback returns `false`.
    pub(crate) fn visit_levels(&self, side: Side, mut visit: impl FnMut(LevelInfo) -> bool) {
        match side {
            Side::Buy => {
                for (&price, level) in self.bids.iter().rev() {
                    if !visit(LevelInfo {
                        price,
                        side,
                        total_size: level.total_size,
                        order_count: level.count,
                    }) {
                        break;
                    }
                }
            }
            Side::Sell => {
                for (&price, level) in &self.asks {
                    if !visit(LevelInfo {
                        price,
                        side,
                        total_size: level.total_size,
                        order_count: level.count,
                    }) {
                        break;
                    }
                }
            }
        }
    }

    /// Allocates copies in best-price/FIFO order for snapshots and diagnostics.
    ///
    /// Requires valid FIFO links; it is not a corruption-tolerant traversal.
    pub(crate) fn orders_in_priority(&self, side: Side) -> Vec<Order> {
        let mut result = Vec::with_capacity(self.len());
        self.visit_levels(side, |info| {
            let level = self
                .levels(side)
                .get(&info.price)
                .expect("visited level exists");
            let mut cursor = level.head;
            while let Some(handle) = cursor {
                let entry = self.orders.get(handle).expect("linked handle is live");
                result.push(copy_order(&entry.order));
                cursor = entry.next;
            }
            true
        });
        result
    }

    /// Audits link reciprocity, FIFO sequence, aggregates, indexes, and limits.
    ///
    /// Every reachable handle must resolve to the indexed order at that level.
    /// A bounded node count detects cycles; total reachability must match arena
    /// occupancy. Returns `InvalidSnapshot` on inconsistency. This full-book
    /// diagnostic is used at recovery/test boundaries, not for each command.
    pub(crate) fn validate_invariants(&self) -> Result<(), MatchingError> {
        let mut visited = 0usize;
        let mut expected_active_levels = 0usize;
        for side in [Side::Buy, Side::Sell] {
            let mut check = |price: Price, level: &Level| -> Result<(), MatchingError> {
                expected_active_levels += 1;
                if level.count == 0 || level.count > self.max_orders_per_level {
                    return Err(MatchingError::InvalidSnapshot);
                }
                let mut count = 0usize;
                let mut total_size = 0u128;
                let mut previous = None;
                let mut previous_sequence = None;
                let mut cursor = level.head;
                while let Some(handle) = cursor {
                    count += 1;
                    if count > self.len() {
                        return Err(MatchingError::InvalidSnapshot);
                    }
                    let entry = self
                        .orders
                        .get(handle)
                        .ok_or(MatchingError::InvalidSnapshot)?;
                    if entry.previous != previous
                        || entry.order.side != side
                        || entry.order.price != price
                        || self.order_index.get(&entry.order.order_id) != Some(&handle)
                        || previous_sequence
                            .is_some_and(|sequence| sequence >= entry.order.sequence)
                    {
                        return Err(MatchingError::InvalidSnapshot);
                    }
                    total_size = total_size
                        .checked_add(u128::from(entry.order.size.0))
                        .ok_or(MatchingError::InvalidSnapshot)?;
                    previous_sequence = Some(entry.order.sequence);
                    previous = Some(handle);
                    cursor = entry.next;
                }
                if count != level.count || previous != level.tail || total_size != level.total_size
                {
                    return Err(MatchingError::InvalidSnapshot);
                }
                visited += count;
                Ok(())
            };
            match side {
                Side::Buy => {
                    for (&price, level) in &self.bids {
                        check(price, level)?;
                    }
                }
                Side::Sell => {
                    for (&price, level) in &self.asks {
                        check(price, level)?;
                    }
                }
            }
        }
        if expected_active_levels != self.active_levels
            || expected_active_levels > self.max_levels
            || visited != self.orders.len()
            || self.order_index.len() != self.orders.len()
        {
            return Err(MatchingError::InvalidSnapshot);
        }
        for (&order_id, &handle) in &self.order_index {
            if self
                .orders
                .get(handle)
                .is_none_or(|entry| entry.order.order_id != order_id)
            {
                return Err(MatchingError::InvalidSnapshot);
            }
        }
        Ok(())
    }

    /// Links a detached live node at an existing level's tail and adds its aggregate.
    ///
    /// The level and handle must exist, `size` must match the order, and capacity
    /// and arithmetic must already be preflighted. Violations indicate a caller bug.
    fn append_to_level(&mut self, side: Side, price: Price, handle: Handle, size: Quantity) {
        let tail = self.levels(side).get(&price).and_then(|level| level.tail);
        if let Some(tail) = tail {
            self.orders.get_mut(tail).expect("level tail is live").next = Some(handle);
        }
        let entry = self
            .orders
            .get_mut(handle)
            .expect("appended handle is live");
        entry.previous = tail;
        entry.next = None;
        let level = self.levels_mut(side).get_mut(&price).expect("level exists");
        if level.head.is_none() {
            level.head = Some(handle);
        }
        level.tail = Some(handle);
        level.count += 1;
        level.total_size += u128::from(size.0);
    }

    /// Detaches a live node, repairs neighbors, and removes an emptied level.
    ///
    /// The arena slot remains live for removal or replacement by the caller.
    /// Side, price, size, and reciprocal links must match the node's current
    /// membership; lookup and aggregate subtraction rely on that invariant.
    fn unlink_from_level(&mut self, side: Side, price: Price, handle: Handle, size: Quantity) {
        let (previous, next) = {
            let entry = self.orders.get(handle).expect("unlinked handle is live");
            (entry.previous, entry.next)
        };
        if let Some(previous) = previous {
            self.orders
                .get_mut(previous)
                .expect("previous link is live")
                .next = next;
        }
        if let Some(next) = next {
            self.orders
                .get_mut(next)
                .expect("next link is live")
                .previous = previous;
        }
        let level = self
            .levels_mut(side)
            .get_mut(&price)
            .expect("order level exists");
        if level.head == Some(handle) {
            level.head = next;
        }
        if level.tail == Some(handle) {
            level.tail = previous;
        }
        level.count -= 1;
        level.total_size -= u128::from(size.0);
        if level.count == 0 {
            self.levels_mut(side).remove(&price);
            self.active_levels -= 1;
        }
        let entry = self
            .orders
            .get_mut(handle)
            .expect("unlinked handle is live");
        entry.previous = None;
        entry.next = None;
    }

    /// Selects a side's ascending price map without changing traversal order.
    fn levels(&self, side: Side) -> &BTreeMap<Price, Level> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    /// Selects the price map whose levels a storage mutation will update.
    fn levels_mut(&mut self, side: Side) -> &mut BTreeMap<Price, Level> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }
}

/// Copies public order state without exposing private node links or handles.
fn copy_order(order: &Order) -> Order {
    Order {
        order_id: order.order_id,
        client_id: order.client_id,
        account_id: order.account_id,
        price: order.price,
        size: order.size,
        side: order.side,
        sequence: order.sequence,
    }
}
