//! Versioned snapshots for exporting and restoring matching engine state.
//!
//! Snapshots contain public live orders and FIFO order; private arena handles
//! are rebuilt during restore so no storage implementation leaks into the
//! serialized representation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::engine::MatchingEngine;
use crate::types::{
    AccountId, ClientOrderId, EngineConfig, EngineId, InstrumentSpec, MatchingError, Order,
    OrderId, Price, Side,
};

/// Schema version emitted by the current matching-core snapshot format.
pub const SNAPSHOT_VERSION: u32 = 1;

/// Live orders at one price level, serialized in their FIFO priority order.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LevelSnapshot {
    /// Price of this side-specific level.
    pub price: Price,
    /// Live orders in oldest-to-newest time priority.
    pub orders: Vec<Order>,
}

/// Complete serializable state required to continue one matching engine.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineSnapshot {
    /// Snapshot schema version for compatibility validation.
    pub version: u32,
    /// Identity domain used by this engine's public order IDs.
    pub engine_id: EngineId,
    /// Exact instrument rules used by this engine.
    pub spec: InstrumentSpec,
    /// Logical capacity limits used by this engine.
    pub config: EngineConfig,
    /// Next exchange order ID sequence to allocate.
    pub next_order_id: u64,
    /// Next FIFO priority sequence to allocate, starting at zero.
    pub next_sequence: u64,
    /// Bid levels ordered from highest to lowest price.
    pub bids: Vec<LevelSnapshot>,
    /// Ask levels ordered from lowest to highest price.
    pub asks: Vec<LevelSnapshot>,
}

impl MatchingEngine {
    /// Copies this engine's live state into a versioned snapshot.
    ///
    /// Snapshot construction may allocate and is intended for persistence or
    /// replay boundaries, outside the matching command path.
    pub fn export_snapshot(&self) -> EngineSnapshot {
        EngineSnapshot {
            version: SNAPSHOT_VERSION,
            engine_id: self.engine_id,
            spec: self.spec.clone(),
            config: self.config,
            next_order_id: self.next_order_id,
            next_sequence: self.next_sequence,
            bids: levels_from_orders(self.orders_in_priority(Side::Buy)),
            asks: levels_from_orders(self.orders_in_priority(Side::Sell)),
        }
    }

    /// Validates and restores a complete engine snapshot.
    ///
    /// The book, live client-ID index, and private arena handles are rebuilt
    /// from the snapshot's public orders. Invalid state is rejected before a
    /// usable engine is returned.
    ///
    /// # Arguments
    ///
    /// * `snapshot` - Versioned state to validate and restore.
    ///
    /// # Returns
    ///
    /// A ready matching engine, or `InvalidSnapshot` when the serialized state
    /// is inconsistent. Startup allocation failures are preserved as
    /// `AllocationFailed`.
    pub fn from_snapshot(snapshot: EngineSnapshot) -> Result<Self, MatchingError> {
        validate_snapshot(&snapshot)?;

        let mut engine = MatchingEngine::new(snapshot.engine_id, snapshot.spec, snapshot.config)?;
        engine.next_order_id = snapshot.next_order_id;
        engine.next_sequence = snapshot.next_sequence;

        for level in snapshot.bids.iter().chain(&snapshot.asks) {
            for order in &level.orders {
                engine
                    .book
                    .add_order(*order)
                    .map_err(|_| MatchingError::InvalidSnapshot)?;
                engine
                    .client_to_order
                    .insert((order.account_id, order.client_id), order.order_id);
            }
        }

        engine
            .validate_invariants()
            .map_err(|_| MatchingError::InvalidSnapshot)?;

        Ok(engine)
    }
}

fn levels_from_orders(orders: Vec<Order>) -> Vec<LevelSnapshot> {
    let mut levels: Vec<LevelSnapshot> = Vec::new();
    for order in orders {
        if let Some(level) = levels.last_mut().filter(|level| level.price == order.price) {
            level.orders.push(order);
        } else {
            levels.push(LevelSnapshot {
                price: order.price,
                orders: vec![order],
            });
        }
    }
    levels
}

fn validate_snapshot(snapshot: &EngineSnapshot) -> Result<(), MatchingError> {
    if snapshot.version != SNAPSHOT_VERSION
        || snapshot.engine_id.0 == 0
        || snapshot.next_order_id == 0
        || snapshot.next_sequence.checked_add(1) != Some(snapshot.next_order_id)
    {
        return Err(MatchingError::InvalidSnapshot);
    }
    snapshot
        .spec
        .validate()
        .map_err(|_| MatchingError::InvalidSnapshot)?;
    let max_live_orders = snapshot
        .config
        .validate()
        .map_err(|_| MatchingError::InvalidSnapshot)?;

    let level_count = snapshot
        .bids
        .len()
        .checked_add(snapshot.asks.len())
        .ok_or(MatchingError::InvalidSnapshot)?;
    if level_count > snapshot.config.max_levels {
        return Err(MatchingError::InvalidSnapshot);
    }

    let mut order_ids = BTreeSet::new();
    let mut client_ids = BTreeSet::new();
    let mut priorities = BTreeSet::new();
    let mut order_count = 0usize;
    validate_levels(
        &snapshot.bids,
        Side::Buy,
        snapshot,
        &mut order_ids,
        &mut client_ids,
        &mut priorities,
        &mut order_count,
    )?;
    validate_levels(
        &snapshot.asks,
        Side::Sell,
        snapshot,
        &mut order_ids,
        &mut client_ids,
        &mut priorities,
        &mut order_count,
    )?;

    if order_count > max_live_orders
        || snapshot
            .bids
            .first()
            .zip(snapshot.asks.first())
            .is_some_and(|(bid, ask)| bid.price >= ask.price)
    {
        return Err(MatchingError::InvalidSnapshot);
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_levels(
    levels: &[LevelSnapshot],
    side: Side,
    snapshot: &EngineSnapshot,
    order_ids: &mut BTreeSet<OrderId>,
    client_ids: &mut BTreeSet<(AccountId, ClientOrderId)>,
    priorities: &mut BTreeSet<u64>,
    order_count: &mut usize,
) -> Result<(), MatchingError> {
    let mut previous_price: Option<Price> = None;
    for level in levels {
        if level.price.0 == 0
            || level.orders.is_empty()
            || level.orders.len() > snapshot.config.max_orders_per_level
        {
            return Err(MatchingError::InvalidSnapshot);
        }
        if let Some(previous) = previous_price {
            let sorted = match side {
                Side::Buy => previous > level.price,
                Side::Sell => previous < level.price,
            };
            if !sorted {
                return Err(MatchingError::InvalidSnapshot);
            }
        }
        previous_price = Some(level.price);

        let mut previous_priority = None;
        for order in &level.orders {
            if order.side != side
                || order.price != level.price
                || order.price.0 == 0
                || order.size.0 == 0
                || snapshot
                    .spec
                    .max_size
                    .is_some_and(|maximum| order.size.0 > maximum.0)
                || order.order_id.engine_id != snapshot.engine_id
                || order.order_id.sequence == 0
                || order.order_id.sequence >= snapshot.next_order_id
                || order.sequence.checked_add(1) != Some(order.order_id.sequence)
                || order.sequence >= snapshot.next_sequence
                || !order_ids.insert(order.order_id)
                || !client_ids.insert((order.account_id, order.client_id))
                || !priorities.insert(order.sequence)
                || previous_priority.is_some_and(|previous| order.sequence <= previous)
            {
                return Err(MatchingError::InvalidSnapshot);
            }
            previous_priority = Some(order.sequence);
            *order_count = order_count
                .checked_add(1)
                .ok_or(MatchingError::InvalidSnapshot)?;
        }
    }
    Ok(())
}
