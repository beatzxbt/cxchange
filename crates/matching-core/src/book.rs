//! Fixed-capacity order storage, FIFO price levels, and ordered price lookup.
//!
//! `Book` owns stable arena entries and links orders within each price level.
//! Bid and ask levels use `BTreeMap`; its node allocations are part of this
//! correctness-first baseline and are not claimed to be real-time bounded.

use std::collections::BTreeMap;

use crate::arena::{Arena, ArenaError, Handle};
use crate::types::{
    EngineConfig, LevelDelta, MatchingError, Order, OrderId, Price, Quantity, Side,
};

struct Entry {
    order: Order,
    previous: Option<Handle>,
    next: Option<Handle>,
}

#[derive(Default)]
struct Level {
    head: Option<Handle>,
    tail: Option<Handle>,
    count: usize,
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
    orders: Arena<Entry>,
    order_index: BTreeMap<OrderId, Handle>,
    bids: BTreeMap<Price, Level>,
    asks: BTreeMap<Price, Level>,
    active_levels: usize,
    max_orders_per_level: usize,
    max_levels: usize,
}

impl Book {
    /// Creates an empty book with capacity derived from its validated config.
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

    pub(crate) fn len(&self) -> usize {
        self.orders.len()
    }

    pub(crate) fn best_bid(&self) -> Option<Price> {
        self.bids.last_key_value().map(|(price, _)| *price)
    }

    pub(crate) fn best_ask(&self) -> Option<Price> {
        self.asks.first_key_value().map(|(price, _)| *price)
    }

    pub(crate) fn level_info(&self, side: Side, price: Price) -> Option<LevelInfo> {
        let level = self.levels(side).get(&price)?;
        Some(LevelInfo {
            price,
            side,
            total_size: level.total_size,
            order_count: level.count,
        })
    }

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

    pub(crate) fn get_order(&self, order_id: OrderId) -> Option<&Order> {
        let handle = *self.order_index.get(&order_id)?;
        Some(&self.orders.get(handle)?.order)
    }

    pub(crate) fn handle_for(&self, order_id: OrderId) -> Option<Handle> {
        self.order_index.get(&order_id).copied()
    }

    pub(crate) fn order_for_handle(&self, handle: Handle) -> Option<&Order> {
        self.orders.get(handle).map(|entry| &entry.order)
    }

    pub(crate) fn would_cross(&self, side: Side, price: Price) -> bool {
        match side {
            Side::Buy => self.best_ask().is_some_and(|ask| price >= ask),
            Side::Sell => self.best_bid().is_some_and(|bid| price <= bid),
        }
    }

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

    fn levels(&self, side: Side) -> &BTreeMap<Price, Level> {
        match side {
            Side::Buy => &self.bids,
            Side::Sell => &self.asks,
        }
    }

    fn levels_mut(&mut self, side: Side) -> &mut BTreeMap<Price, Level> {
        match side {
            Side::Buy => &mut self.bids,
            Side::Sell => &mut self.asks,
        }
    }
}

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
