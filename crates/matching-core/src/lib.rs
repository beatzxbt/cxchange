//! Deterministic price-time matching with bounded book and response capacity.
//!
//! The crate exposes serializable domain types and keeps arena storage private
//! to the matching-core implementation. Public commands and events use exact
//! integer instrument units rather than floating-point values.
//!
//! # Example
//!
//! Reuse one response buffer for commands, and persist snapshots outside the
//! matching path. Prices and quantities below are integer instrument units.
//!
//! ```
//! use matching_core::{
//!     AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId,
//!     InstrumentSpec, MatchingEngine, MatchingResponse, Price, Quantity, Side,
//! };
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let config = EngineConfig::default();
//! let spec = InstrumentSpec {
//!     symbol: "BTC-USD".into(),
//!     tick_size: DecimalScale::new(1, 2)?,
//!     lot_size: DecimalScale::new(1, 8)?,
//!     min_size: Quantity(1),
//!     max_size: None,
//! };
//! let mut engine = MatchingEngine::new(EngineId(7), spec, config)?;
//! let mut response = MatchingResponse::with_capacity(&config)?;
//! engine.execute_into(&Command::Add {
//!     account_id: AccountId(1), client_id: ClientOrderId(10),
//!     side: Side::Sell, price: Price(100), size: Quantity(3),
//! }, &mut response)?;
//! engine.execute_into(&Command::Take {
//!     account_id: AccountId(2), client_id: ClientOrderId(20),
//!     side: Side::Buy, limit_price: Price(100), size: Quantity(2),
//! }, &mut response)?;
//! assert_eq!(response.fills[0].size, Quantity(2));
//! assert_eq!(engine.get_level(Side::Sell, Price(100)).unwrap().size, 1);
//! let encoded = serde_json::to_vec(&engine.export_snapshot())?;
//! let restored = MatchingEngine::from_snapshot(serde_json::from_slice(&encoded)?)?;
//! assert_eq!(restored.export_snapshot(), engine.export_snapshot());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod types;

mod arena;
mod book;
mod engine;
pub mod execution;
mod snapshot;

pub use engine::MatchingEngine;
pub use snapshot::{EngineSnapshot, LevelSnapshot, SNAPSHOT_VERSION};

pub use types::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, Fill, InstrumentSpec,
    LevelDelta, MAX_DECIMAL_SCALE, MAX_LIVE_ORDERS, MatchingError, MatchingResponse, Order,
    OrderDelta, OrderId, OrderStatus, Price, Quantity, Side,
};
