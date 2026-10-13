//! Command validation, deterministic matching, and public book queries.
//!
//! `MatchingEngine` is the sole mutable owner for one symbol. Each command is
//! planned and its complete response capacity is checked before the book or
//! sequencing counters change.
//!
//! Maker commands preflight and then update storage directly. Takers first
//! collect reached makers and final level aggregates in reusable scratch; a
//! late self-trade therefore rejects without applying earlier planned fills.
//! Commit relies on exclusive ownership keeping those handles live. Scratch is
//! transient and is neither authoritative state nor part of a snapshot.

use std::collections::BTreeMap;

use crate::arena::Handle;
use crate::book::{Book, LevelInfo};
use crate::types::{
    AccountId, ClientOrderId, Command, EngineConfig, EngineId, Fill, InstrumentSpec, LevelDelta,
    MatchingError, MatchingResponse, Order, OrderDelta, OrderId, OrderStatus, Price, Quantity,
    Side,
};

/// One reached maker and its execution amount, retained until taker commit.
#[derive(Clone, Copy)]
struct PlannedFill {
    /// Live arena reference established by the read-only traversal.
    handle: Handle,
    /// Maker price used for both the fill and maker order delta.
    price: Price,
    /// Positive execution amount, at most the maker's remaining quantity.
    size: Quantity,
}

/// Final aggregate for one touched level, in first-visit matching order.
#[derive(Clone, Copy)]
struct PlannedLevel {
    /// Price of the reached maker level.
    price: Price,
    /// Maker side whose remaining aggregate is being planned.
    side: Side,
    /// Remaining quantity after all planned fills at this level.
    total_size: u128,
    /// Remaining live makers after all planned fills at this level.
    order_count: usize,
}

/// Deterministic single-symbol matching engine with price-time priority.
///
/// Submit commands through [`Self::execute_into`] using a reusable response.
/// Queries borrow or copy state and cannot mutate the book. Callers serialize
/// access to this owner; the engine supplies no background worker or queue.
/// Returned command errors preserve live state and counters. Ordered-map
/// allocator/process failure is outside that recoverable-error guarantee.
pub struct MatchingEngine {
    /// Caller-assigned nonzero domain shared by all issued public order IDs.
    pub(crate) engine_id: EngineId,
    /// Instrument rules validated at creation or restore.
    pub(crate) spec: InstrumentSpec,
    /// Logical limits persisted with snapshots for deterministic replay.
    pub(crate) config: EngineConfig,
    /// Arena, public-ID index, and side/price FIFO storage.
    pub(crate) book: Book,
    /// Live resting makers only; transient takers never enter this index.
    pub(crate) client_to_order: BTreeMap<(AccountId, ClientOrderId), OrderId>,
    /// Next unused public sequence, initially 1; never an arena index.
    pub(crate) next_order_id: u64,
    /// Next unused FIFO sequence, initially 0; always `next_order_id - 1`.
    pub(crate) next_sequence: u64,
    /// Startup-reserved scratch for a sweep of every legal live maker.
    fill_plan: Vec<PlannedFill>,
    /// Startup-reserved scratch for one final delta per touched level.
    level_plan: Vec<PlannedLevel>,
}

impl MatchingEngine {
    /// Creates a new engine for one symbol and reserves maximum sweep scratch.
    ///
    /// # Arguments
    ///
    /// * `engine_id` - Nonzero identity domain supplied by the caller.
    /// * `spec` - Exact price, quantity, and instrument size constraints.
    /// * `config` - Fixed order and price-level capacity limits.
    ///
    /// # Returns
    ///
    /// A ready engine, or an error if its identity, specification, or
    /// configured capacities are invalid, or `AllocationFailed` when arena or
    /// scratch reservation fails. The caller must assign engine IDs uniquely
    /// within its deployment's identity domain.
    pub fn new(
        engine_id: EngineId,
        spec: InstrumentSpec,
        config: EngineConfig,
    ) -> Result<Self, MatchingError> {
        if engine_id.0 == 0 {
            return Err(MatchingError::InvalidConfig);
        }
        spec.validate()?;
        let max_live_orders = config.validate()?;
        let book = Book::new(&config, max_live_orders)?;
        let mut fill_plan = Vec::new();
        fill_plan
            .try_reserve_exact(max_live_orders)
            .map_err(|_| MatchingError::AllocationFailed)?;
        let mut level_plan = Vec::new();
        level_plan
            .try_reserve_exact(config.max_levels)
            .map_err(|_| MatchingError::AllocationFailed)?;

        Ok(Self {
            engine_id,
            spec,
            config,
            book,
            client_to_order: BTreeMap::new(),
            next_order_id: 1,
            next_sequence: 0,
            fill_plan,
            level_plan,
        })
    }

    /// Applies one command and writes its complete ordered result into a
    /// caller-owned reusable response buffer.
    ///
    /// The response is cleared at entry. Every possible result size and all
    /// counter advances are checked before a command mutates authoritative
    /// state. A taker may fill any number of makers up to the live-order cap.
    /// No output vectors are grown by this method; create full-sweep storage
    /// with [`MatchingResponse::with_capacity`]. On rejection the response is
    /// empty and retains its allocated capacity. Ordered maps can allocate
    /// during commit; process allocation failure is not a returned rejection.
    ///
    /// # Arguments
    ///
    /// * `command` - Typed command whose values and ownership are checked at runtime.
    /// * `response` - Reusable output storage with enough capacity for this result.
    ///
    /// # Returns
    ///
    /// `Ok(())` after applying the command, or a controlled rejection with no
    /// book or counter mutation.
    pub fn execute_into(
        &mut self,
        command: &Command,
        response: &mut MatchingResponse,
    ) -> Result<(), MatchingError> {
        response.clear();
        match *command {
            Command::Add {
                account_id,
                client_id,
                side,
                price,
                size,
            } => self.add(account_id, client_id, side, price, size, response),
            Command::Cancel {
                account_id,
                order_id,
            } => self.cancel(account_id, order_id, response),
            Command::Amend {
                account_id,
                order_id,
                price,
                size,
            } => self.amend(account_id, order_id, price, size, response),
            Command::Take {
                account_id,
                client_id,
                side,
                limit_price,
                size,
            } => self.take(account_id, client_id, side, limit_price, size, response),
        }
    }

    /// Looks up a live order by its engine-scoped public identity.
    ///
    /// Returns `None` for a foreign engine, an unknown ID, or a terminal order.
    /// The shared reference prevents mutation of authoritative state.
    pub fn get_order(&self, order_id: OrderId) -> Option<&Order> {
        if order_id.engine_id != self.engine_id {
            return None;
        }
        self.book.get_order(order_id)
    }

    /// Looks up a live order by account-scoped client identity.
    ///
    /// Takers are never live in this index. Terminal maker IDs become reusable;
    /// successful amends preserve the client ID while changing its public ID.
    pub fn get_order_by_client_id(
        &self,
        account_id: AccountId,
        client_id: ClientOrderId,
    ) -> Option<&Order> {
        let order_id = self.client_to_order.get(&(account_id, client_id))?;
        self.book.get_order(*order_id)
    }

    /// Alias for [`get_order_by_client_id`](Self::get_order_by_client_id).
    pub fn get_by_client(&self, account_id: AccountId, client_id: ClientOrderId) -> Option<&Order> {
        self.get_order_by_client_id(account_id, client_id)
    }

    /// Reports whether an account currently has a live order with this client ID.
    pub fn has_order(&self, account_id: AccountId, client_id: ClientOrderId) -> bool {
        self.client_to_order.contains_key(&(account_id, client_id))
    }

    /// Returns the best bid price, if the bid book is nonempty.
    pub fn best_bid(&self) -> Option<Price> {
        self.book.best_bid()
    }

    /// Returns the best ask price, if the ask book is nonempty.
    pub fn best_ask(&self) -> Option<Price> {
        self.book.best_ask()
    }

    /// Returns the aggregate at one level, or `None` when it is empty.
    pub fn get_level(&self, side: Side, price: Price) -> Option<LevelDelta> {
        self.book.level_info(side, price).map(LevelInfo::delta)
    }

    /// Returns up to `levels` aggregates in best-to-worst price order.
    ///
    /// Bids descend, asks ascend, and zero returns an empty vector. This query
    /// allocates a result and is separate from reusable command output.
    pub fn depth(&self, side: Side, levels: usize) -> Vec<LevelDelta> {
        self.book
            .depth(side, levels)
            .into_iter()
            .map(LevelInfo::delta)
            .collect()
    }

    /// Returns this engine's immutable instrument specification.
    pub fn spec(&self) -> &InstrumentSpec {
        &self.spec
    }

    /// Returns this engine's immutable logical capacity configuration.
    pub fn config(&self) -> EngineConfig {
        self.config
    }

    /// Verifies book links, aggregates, indexes, ownership, and counters.
    ///
    /// Returns `InvalidSnapshot` on inconsistency. This diagnostic traverses
    /// the full book and allocates temporary order copies; use it at recovery
    /// or test boundaries rather than on the normal command path. Positive
    /// partial remainders may be below the instrument's command minimum.
    pub fn validate_invariants(&self) -> Result<(), MatchingError> {
        self.book.validate_invariants()?;
        if self.client_to_order.len() != self.book.len() {
            return Err(MatchingError::InvalidSnapshot);
        }
        let mut largest_order_id = 0u64;
        let mut largest_priority = None;
        for side in [Side::Buy, Side::Sell] {
            for order in self.book.orders_in_priority(side) {
                if order.order_id.engine_id != self.engine_id
                    || order.sequence.checked_add(1) != Some(order.order_id.sequence)
                    || order.price.0 == 0
                    || order.size.0 == 0
                    || self
                        .spec
                        .max_size
                        .is_some_and(|maximum| order.size.0 > maximum.0)
                    || self
                        .client_to_order
                        .get(&(order.account_id, order.client_id))
                        != Some(&order.order_id)
                {
                    return Err(MatchingError::InvalidSnapshot);
                }
                largest_order_id = largest_order_id.max(order.order_id.sequence);
                largest_priority = Some(
                    largest_priority
                        .map_or(order.sequence, |largest: u64| largest.max(order.sequence)),
                );
            }
        }
        if self.next_sequence.checked_add(1) != Some(self.next_order_id)
            || self.next_order_id <= largest_order_id
            || largest_priority.is_some_and(|largest| self.next_sequence <= largest)
            || self
                .book
                .best_bid()
                .zip(self.book.best_ask())
                .is_some_and(|(bid, ask)| bid >= ask)
        {
            return Err(MatchingError::InvalidSnapshot);
        }
        for (&(account_id, client_id), &order_id) in &self.client_to_order {
            let Some(order) = self.book.get_order(order_id) else {
                return Err(MatchingError::InvalidSnapshot);
            };
            if order.account_id != account_id || order.client_id != client_id {
                return Err(MatchingError::InvalidSnapshot);
            }
        }
        Ok(())
    }

    /// Copies all live orders for snapshot construction in deterministic order.
    pub(crate) fn orders_in_priority(&self, side: Side) -> Vec<Order> {
        self.book.orders_in_priority(side)
    }

    /// Validates a maker and reserves output/identity before appending to its FIFO.
    ///
    /// Only a committed resting maker enters the account/client index. The
    /// result contains one open-order delta and the final level aggregate.
    fn add(
        &mut self,
        account_id: AccountId,
        client_id: ClientOrderId,
        side: Side,
        price: Price,
        size: Quantity,
        response: &mut MatchingResponse,
    ) -> Result<(), MatchingError> {
        self.validate_price_size(price, size)?;
        if self.client_to_order.contains_key(&(account_id, client_id)) {
            return Err(MatchingError::DuplicateClientId);
        }
        if self.book.would_cross(side, price) {
            return Err(MatchingError::WouldCross);
        }
        self.book.can_add(side, price, size)?;
        response.ensure_capacity(0, 1, 1)?;
        let (order_id, priority, next_order_id, next_sequence) = self.next_identity()?;
        let order = Order {
            order_id,
            client_id,
            account_id,
            price,
            size,
            side,
            sequence: priority,
        };
        let level = self.book.add_order(order)?;
        self.client_to_order
            .insert((account_id, client_id), order_id);
        self.next_order_id = next_order_id;
        self.next_sequence = next_sequence;
        response.order_deltas.push(OrderDelta {
            order_id,
            client_id,
            status: OrderStatus::Open,
            remaining_size: size,
            filled_size: Quantity(0),
            fill_price: None,
        });
        response.level_deltas.push(level.delta());
        Ok(())
    }

    /// Removes an owned maker and its client mapping without advancing counters.
    ///
    /// Output capacity is checked before unlinking; the result reports a
    /// terminal order and the final aggregate, including an emptied level.
    fn cancel(
        &mut self,
        account_id: AccountId,
        order_id: OrderId,
        response: &mut MatchingResponse,
    ) -> Result<(), MatchingError> {
        let old_order = self.resolve_owned(account_id, order_id)?;
        response.ensure_capacity(0, 1, 1)?;
        let (removed, new_level) = self.book.remove_order(order_id)?;
        debug_assert_eq!(removed, old_order);
        self.client_to_order
            .remove(&(old_order.account_id, old_order.client_id));
        response.order_deltas.push(OrderDelta {
            order_id,
            client_id: old_order.client_id,
            status: OrderStatus::Cancelled,
            remaining_size: Quantity(0),
            filled_size: Quantity(0),
            fill_price: None,
        });
        response.level_deltas.push(new_level.delta());
        Ok(())
    }

    /// Replaces an owned maker after validating the complete resulting state.
    ///
    /// Every successful replacement gets a fresh ID/priority, even when price
    /// and size are unchanged. Old/new order deltas and removal/insertion level
    /// deltas retain that order; a rejection preserves the original maker.
    fn amend(
        &mut self,
        account_id: AccountId,
        order_id: OrderId,
        price: Price,
        size: Quantity,
        response: &mut MatchingResponse,
    ) -> Result<(), MatchingError> {
        let old_order = self.resolve_owned(account_id, order_id)?;
        self.validate_price_size(price, size)?;
        if self.book.would_cross(old_order.side, price) {
            return Err(MatchingError::WouldCross);
        }
        self.book.can_amend(order_id, price, size)?;
        response.ensure_capacity(0, 2, 2)?;
        let (new_order_id, priority, next_order_id, next_sequence) = self.next_identity()?;
        let replacement = Order {
            order_id: new_order_id,
            client_id: old_order.client_id,
            account_id: old_order.account_id,
            price,
            size,
            side: old_order.side,
            sequence: priority,
        };
        let (removed, old_level, new_level) = self.book.replace_order(order_id, replacement)?;
        debug_assert_eq!(removed, old_order);
        self.client_to_order
            .insert((old_order.account_id, old_order.client_id), new_order_id);
        self.next_order_id = next_order_id;
        self.next_sequence = next_sequence;
        response.order_deltas.push(OrderDelta {
            order_id,
            client_id: old_order.client_id,
            status: OrderStatus::Replaced,
            remaining_size: Quantity(0),
            filled_size: Quantity(0),
            fill_price: None,
        });
        response.order_deltas.push(OrderDelta {
            order_id: new_order_id,
            client_id: old_order.client_id,
            status: OrderStatus::Open,
            remaining_size: size,
            filled_size: Quantity(0),
            fill_price: None,
        });
        response.level_deltas.push(old_level.delta());
        response.level_deltas.push(new_level.delta());
        Ok(())
    }

    /// Plans the complete eligible execution before applying any maker fill.
    ///
    /// Planning stops at requested size or the inclusive limit. Self-trade is
    /// checked only for reached makers, but any conflict rejects the whole plan.
    /// After identity and output preflight, commit emits the taker delta first,
    /// followed by maker deltas/fills and one final delta per touched level.
    /// Unfilled taker quantity is discarded and never enters the client index.
    fn take(
        &mut self,
        account_id: AccountId,
        client_id: ClientOrderId,
        side: Side,
        limit_price: Price,
        size: Quantity,
        response: &mut MatchingResponse,
    ) -> Result<(), MatchingError> {
        self.validate_price_size(limit_price, size)?;
        self.fill_plan.clear();
        self.level_plan.clear();
        let mut remaining = size.0;
        let mut planning_error = None;
        {
            let book = &self.book;
            let fills = &mut self.fill_plan;
            let levels = &mut self.level_plan;
            book.visit_eligible(side.opposite(), limit_price, |price, handle, order| {
                if remaining == 0 {
                    return false;
                }
                if order.account_id == account_id {
                    planning_error = Some(MatchingError::SelfTrade);
                    return false;
                }
                if fills.len() == fills.capacity() {
                    planning_error = Some(MatchingError::CapacityExceeded);
                    return false;
                }
                let fill_amount = order.size.0.min(remaining);
                let fill_size = Quantity(fill_amount);
                fills.push(PlannedFill {
                    handle,
                    price,
                    size: fill_size,
                });
                remaining -= fill_amount;

                if levels
                    .last()
                    .is_none_or(|level| level.price != price || level.side != order.side)
                {
                    if levels.len() == levels.capacity() {
                        planning_error = Some(MatchingError::CapacityExceeded);
                        return false;
                    }
                    let Some(level_info) = book.level_info(order.side, price) else {
                        planning_error = Some(MatchingError::InvalidSnapshot);
                        return false;
                    };
                    levels.push(PlannedLevel {
                        price,
                        side: order.side,
                        total_size: level_info.total_size,
                        order_count: level_info.order_count,
                    });
                }
                let Some(level) = levels.last_mut() else {
                    planning_error = Some(MatchingError::InvalidSnapshot);
                    return false;
                };
                let Some(after_fill) = level.total_size.checked_sub(u128::from(fill_amount)) else {
                    planning_error = Some(MatchingError::InvalidSnapshot);
                    return false;
                };
                level.total_size = after_fill;
                if fill_amount == order.size.0 {
                    level.order_count -= 1;
                }
                remaining > 0
            });
        }
        if let Some(error) = planning_error {
            return Err(error);
        }
        if self.fill_plan.is_empty() {
            return Err(MatchingError::NoLiquidity);
        }
        let filled_size = Quantity(size.0 - remaining);
        let (taker_order_id, _priority, next_order_id, next_sequence) = self.next_identity()?;
        response.ensure_capacity(
            self.fill_plan.len(),
            self.fill_plan.len() + 1,
            self.level_plan.len(),
        )?;

        response.order_deltas.push(OrderDelta {
            order_id: taker_order_id,
            client_id,
            status: if remaining == 0 {
                OrderStatus::Filled
            } else {
                OrderStatus::PartiallyFilled
            },
            remaining_size: Quantity(0),
            filled_size,
            fill_price: None,
        });
        for planned in self.fill_plan.iter().copied() {
            let order = *self
                .book
                .order_for_handle(planned.handle)
                .expect("planned maker remains live until commit");
            response.fills.push(Fill {
                taker_order_id,
                taker_client_id: client_id,
                taker_account_id: account_id,
                maker_order_id: order.order_id,
                maker_client_id: order.client_id,
                maker_account_id: order.account_id,
                price: planned.price,
                size: planned.size,
                taker_side: side,
            });
            let fully_filled = planned.size.0 == order.size.0;
            response.order_deltas.push(OrderDelta {
                order_id: order.order_id,
                client_id: order.client_id,
                status: if fully_filled {
                    OrderStatus::Filled
                } else {
                    OrderStatus::PartiallyFilled
                },
                remaining_size: Quantity(order.size.0 - planned.size.0),
                filled_size: planned.size,
                fill_price: Some(planned.price),
            });
            let removed = self.book.apply_fill(planned.handle, planned.size);
            if fully_filled {
                debug_assert_eq!(removed, Some(order));
                self.client_to_order
                    .remove(&(order.account_id, order.client_id));
            } else {
                debug_assert!(removed.is_none());
            }
        }
        for planned in self.level_plan.iter().copied() {
            response.level_deltas.push(LevelDelta {
                price: planned.price,
                size: planned.total_size,
                side: planned.side,
                order_count: planned.order_count,
            });
        }
        self.next_order_id = next_order_id;
        self.next_sequence = next_sequence;
        Ok(())
    }

    /// Checks a positive price and inclusive command-size bounds in exact units.
    fn validate_price_size(&self, price: Price, size: Quantity) -> Result<(), MatchingError> {
        if price.0 == 0 {
            return Err(MatchingError::InvalidPrice);
        }
        if !self.spec.accepts_size(size) {
            return Err(MatchingError::InvalidSize);
        }
        Ok(())
    }

    /// Copies a live order after checking engine domain, existence, then account.
    ///
    /// That validation order determines `WrongEngine`, `OrderNotFound`, or
    /// `WrongOwner` without mutating state.
    fn resolve_owned(
        &self,
        account_id: AccountId,
        order_id: OrderId,
    ) -> Result<Order, MatchingError> {
        if order_id.engine_id != self.engine_id {
            return Err(MatchingError::WrongEngine);
        }
        let order = self
            .book
            .get_order(order_id)
            .copied()
            .ok_or(MatchingError::OrderNotFound)?;
        if order.account_id != account_id {
            return Err(MatchingError::WrongOwner);
        }
        Ok(order)
    }

    /// Previews `(public_id, priority, next_id, next_priority)` without consumption.
    ///
    /// Both stored counters name the next unused value. Callers install the
    /// returned successors only after commit; either checked increment failing
    /// returns `CounterExhausted` and leaves both unchanged.
    fn next_identity(&self) -> Result<(OrderId, u64, u64, u64), MatchingError> {
        let priority = self.next_sequence;
        let next_order_id = self
            .next_order_id
            .checked_add(1)
            .ok_or(MatchingError::CounterExhausted)?;
        let next_sequence = priority
            .checked_add(1)
            .ok_or(MatchingError::CounterExhausted)?;
        Ok((
            OrderId::new(self.engine_id, self.next_order_id),
            priority,
            next_order_id,
            next_sequence,
        ))
    }
}
