//! Serializable domain types, validated configuration, and matching events.
//!
//! Public prices count instrument tick-size quanta and public quantities count
//! lot-size quanta. This representation keeps core arithmetic exact and
//! independent of floating-point rounding.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Maximum number of live orders supported by one configured book.
pub const MAX_LIVE_ORDERS: usize = 131_072;

/// Maximum decimal precision accepted for exact instrument units.
pub const MAX_DECIMAL_SCALE: u8 = 18;

/// Globally unique identifier for an account.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct AccountId(pub u64);

/// Client-provided identifier, unique among that account's live orders.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct ClientOrderId(pub u64);

/// Exact price expressed as a count of instrument tick-size quanta.
///
/// For an instrument with a tick size of `0.25`, `Price(401)` represents the
/// exact value `100.25`. A price of zero is invalid for a matching command.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct Price(pub u64);

/// Exact size expressed as a count of instrument lot-size quanta.
///
/// If an instrument has a lot size of `0.001`, `Quantity(100)` represents the
/// exact size `0.100`. Valid order sizes must also satisfy the instrument's
/// minimum and optional maximum sizes.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct Quantity(pub u64);

/// Identity domain for one matching engine instance.
#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct EngineId(pub u64);

/// Exchange-assigned public order identity.
///
/// The engine identity prevents IDs from independent engine instances from
/// colliding, while the sequence identifies an order within that instance.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct OrderId {
    /// Engine identity domain that issued this order ID.
    pub engine_id: EngineId,
    /// Monotonically increasing engine-local sequence number.
    pub sequence: u64,
}

impl OrderId {
    /// Creates an order ID from its engine domain and sequence number.
    ///
    /// # Arguments
    ///
    /// * `engine_id` - Identity domain that issues this ID.
    /// * `sequence` - Engine-local monotonic order sequence.
    ///
    /// # Returns
    ///
    /// The composed public order identity.
    pub const fn new(engine_id: EngineId, sequence: u64) -> Self {
        Self {
            engine_id,
            sequence,
        }
    }
}

/// Exact non-negative decimal value represented by an integer mantissa.
///
/// The represented value is `mantissa * 10^-scale`; no floating-point value
/// is constructed. Instrument validation requires a positive mantissa and a
/// scale no greater than [`MAX_DECIMAL_SCALE`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct DecimalScale {
    /// Integer digits before applying decimal scale.
    pub mantissa: u64,
    /// Number of base-ten decimal places in the represented value.
    pub scale: u8,
}

impl DecimalScale {
    /// Creates a decimal unit after validating its exact representation.
    ///
    /// # Arguments
    ///
    /// * `mantissa` - Positive integer digits in the representation.
    /// * `scale` - Decimal places to apply to the mantissa.
    ///
    /// # Returns
    ///
    /// A valid decimal scale, or `InvalidInstrumentSpec` when its precision or
    /// mantissa is outside the supported range.
    pub const fn new(mantissa: u64, scale: u8) -> Result<Self, MatchingError> {
        if mantissa == 0 || scale > MAX_DECIMAL_SCALE {
            return Err(MatchingError::InvalidInstrumentSpec);
        }
        Ok(Self { mantissa, scale })
    }

    /// Reports whether the value is a valid exact instrument unit.
    pub const fn is_valid(self) -> bool {
        self.mantissa > 0 && self.scale <= MAX_DECIMAL_SCALE
    }
}

/// Validated limits for one symbol's order book.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct EngineConfig {
    /// Maximum live orders at one `(side, price)` level.
    pub max_orders_per_level: usize,
    /// Maximum non-empty price levels across both sides of the book.
    pub max_levels: usize,
}

impl EngineConfig {
    /// Validates configured limits and returns their derived live-order cap.
    ///
    /// # Returns
    ///
    /// The checked product of orders per level and active levels, bounded by
    /// [`MAX_LIVE_ORDERS`], or `InvalidConfig` for an invalid configuration.
    pub fn validate(&self) -> Result<usize, MatchingError> {
        if self.max_orders_per_level == 0 || self.max_levels == 0 {
            return Err(MatchingError::InvalidConfig);
        }

        let max_orders = self
            .max_orders_per_level
            .checked_mul(self.max_levels)
            .filter(|count| *count <= MAX_LIVE_ORDERS)
            .ok_or(MatchingError::InvalidConfig)?;
        Ok(max_orders)
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_orders_per_level: 128,
            max_levels: 1_024,
        }
    }
}

/// Exact unit and size constraints for one instrument.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InstrumentSpec {
    /// Symbol handled by this instrument specification.
    pub symbol: String,
    /// Exact decimal value of one integer price quantum.
    pub tick_size: DecimalScale,
    /// Exact decimal value of one integer quantity quantum.
    pub lot_size: DecimalScale,
    /// Smallest accepted order size, in lot-size quanta.
    pub min_size: Quantity,
    /// Largest accepted order size, in lot-size quanta, when bounded.
    pub max_size: Option<Quantity>,
}

impl InstrumentSpec {
    /// Validates the symbol, exact units, and order-size bounds.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the instrument can be used for matching, or
    /// `InvalidInstrumentSpec` when any unit or bound is invalid.
    pub fn validate(&self) -> Result<(), MatchingError> {
        if self.symbol.trim().is_empty()
            || !self.tick_size.is_valid()
            || !self.lot_size.is_valid()
            || self.min_size.0 == 0
            || self
                .max_size
                .is_some_and(|maximum| maximum.0 < self.min_size.0)
        {
            return Err(MatchingError::InvalidInstrumentSpec);
        }
        Ok(())
    }

    /// Checks whether a quantity satisfies this instrument's configured size bounds.
    ///
    /// # Arguments
    ///
    /// * `size` - Size in lot-size quanta to validate.
    ///
    /// # Returns
    ///
    /// `true` when the size is positive and within the configured minimum and
    /// optional maximum.
    pub fn accepts_size(&self, size: Quantity) -> bool {
        size.0 > 0
            && size.0 >= self.min_size.0
            && self.max_size.is_none_or(|maximum| size.0 <= maximum.0)
    }
}

/// Direction of a resting or taking order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// Buy orders bid for the instrument.
    Buy,
    /// Sell orders offer the instrument.
    Sell,
}

impl Side {
    /// Returns the opposite side of the book.
    pub const fn opposite(self) -> Self {
        match self {
            Self::Buy => Self::Sell,
            Self::Sell => Self::Buy,
        }
    }
}

/// Lifecycle status of an order represented in a public delta.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    /// A newly accepted order with no fills.
    Open,
    /// A live order that has executed part of its original size.
    PartiallyFilled,
    /// An order whose full requested size has executed.
    Filled,
    /// An order removed by an accepted cancel command.
    Cancelled,
    /// An old order replaced by an accepted amend command.
    Replaced,
}

/// A command applied sequentially to one symbol's matching engine.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Adds a resting maker order.
    Add {
        /// Account submitting the order.
        account_id: AccountId,
        /// Client-chosen ID, unique among this account's live orders.
        client_id: ClientOrderId,
        /// Resting order direction.
        side: Side,
        /// Limit price in tick-size quanta.
        price: Price,
        /// Requested size in lot-size quanta.
        size: Quantity,
    },
    /// Cancels a live order after checking account ownership.
    Cancel {
        /// Account submitting the cancellation.
        account_id: AccountId,
        /// Public identity of the order to cancel.
        order_id: OrderId,
    },
    /// Replaces a live maker order with a new order and fresh time priority.
    Amend {
        /// Account submitting the amendment.
        account_id: AccountId,
        /// Public identity of the order to replace.
        order_id: OrderId,
        /// Replacement limit price in tick-size quanta.
        price: Price,
        /// Replacement size in lot-size quanta.
        size: Quantity,
    },
    /// Executes immediately against eligible opposite-side resting orders.
    Take {
        /// Account submitting the taking order.
        account_id: AccountId,
        /// Client-chosen ID for the taking order.
        client_id: ClientOrderId,
        /// Direction of the taking order.
        side: Side,
        /// Worst price the taking order may execute at, in tick-size quanta.
        limit_price: Price,
        /// Requested size in lot-size quanta.
        size: Quantity,
    },
}

/// Stable, exchange-assigned metadata for one live order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Order {
    /// Exchange-assigned public order identity.
    pub order_id: OrderId,
    /// Client-chosen order identity scoped to its account while live.
    pub client_id: ClientOrderId,
    /// Account that owns the order.
    pub account_id: AccountId,
    /// Limit price in tick-size quanta.
    pub price: Price,
    /// Remaining live size in lot-size quanta.
    pub size: Quantity,
    /// Direction of the resting order.
    pub side: Side,
    /// Monotonic exchange sequence used for time priority.
    pub sequence: u64,
}

/// One execution between an incoming order and a resting maker.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Fill {
    /// Public identity of the incoming taker order.
    pub taker_order_id: OrderId,
    /// Client identity supplied for the incoming taker order.
    pub taker_client_id: ClientOrderId,
    /// Account submitting the incoming taker order.
    pub taker_account_id: AccountId,
    /// Public identity of the resting maker order.
    pub maker_order_id: OrderId,
    /// Client identity supplied for the resting maker order.
    pub maker_client_id: ClientOrderId,
    /// Account that owns the resting maker order.
    pub maker_account_id: AccountId,
    /// Execution price in tick-size quanta, set by the maker.
    pub price: Price,
    /// Executed size in lot-size quanta.
    pub size: Quantity,
    /// Direction of the taker order.
    pub taker_side: Side,
}

/// Public state change for a resting, cancelled, replaced, or taking order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OrderDelta {
    /// Exchange-assigned public order identity.
    pub order_id: OrderId,
    /// Client-provided order identity.
    pub client_id: ClientOrderId,
    /// Status after applying the operation.
    pub status: OrderStatus,
    /// Remaining live size, or zero for a non-resting order.
    pub remaining_size: Quantity,
    /// Quantity executed by the command that produced this delta.
    pub filled_size: Quantity,
    /// Maker execution price for this order's fill delta, or `None` when it
    /// represents an aggregate taker result or a non-fill update.
    pub fill_price: Option<Price>,
}

/// Public aggregate change for one side and price level.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LevelDelta {
    /// Price of the changed level in tick-size quanta.
    pub price: Price,
    /// Total remaining size at the level, accumulated without 64-bit overflow.
    pub size: u128,
    /// Side containing the changed level.
    pub side: Side,
    /// Number of live orders at the level after the change.
    pub order_count: usize,
}

/// Complete ordered result of one accepted matching command.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct MatchingResponse {
    /// Executions in price-time matching order.
    pub fills: Vec<Fill>,
    /// Order updates in command-defined event order.
    pub order_deltas: Vec<OrderDelta>,
    /// Changed level aggregates in deterministic event order.
    pub level_deltas: Vec<LevelDelta>,
}

impl MatchingResponse {
    /// Creates preallocated output buffers sized for the largest legal command.
    ///
    /// # Arguments
    ///
    /// * `config` - Validated book limits used to bound command results.
    ///
    /// # Returns
    ///
    /// A reusable response with capacity for a full legal sweep, or an error
    /// when the book limits are invalid or startup allocation cannot be
    /// reserved.
    pub fn with_capacity(config: &EngineConfig) -> Result<Self, MatchingError> {
        let max_orders = config.validate()?;
        let max_order_deltas = max_orders
            .checked_add(1)
            .ok_or(MatchingError::InvalidConfig)?;
        let max_order_deltas = max_order_deltas.max(2);
        let max_level_deltas = config.max_levels.max(2);
        let mut fills = Vec::new();
        fills
            .try_reserve_exact(max_orders)
            .map_err(|_| MatchingError::AllocationFailed)?;
        let mut order_deltas = Vec::new();
        order_deltas
            .try_reserve_exact(max_order_deltas)
            .map_err(|_| MatchingError::AllocationFailed)?;
        let mut level_deltas = Vec::new();
        level_deltas
            .try_reserve_exact(max_level_deltas)
            .map_err(|_| MatchingError::AllocationFailed)?;
        Ok(Self {
            fills,
            order_deltas,
            level_deltas,
        })
    }

    /// Clears the used prefix of each output buffer while retaining its capacity.
    pub fn clear(&mut self) {
        self.fills.clear();
        self.order_deltas.clear();
        self.level_deltas.clear();
    }

    /// Checks all required result sizes before an operation mutates the book.
    ///
    /// Each argument is an additional number of entries the next command
    /// needs to append. The check uses the current lengths as well as capacities
    /// so callers can safely stage more than one result in a response.
    ///
    /// # Arguments
    ///
    /// * `fills` - Additional fills required by the operation.
    /// * `order_deltas` - Additional order deltas required by the operation.
    /// * `level_deltas` - Additional level deltas required by the operation.
    ///
    /// # Returns
    ///
    /// `Ok(())` when all requested entries fit without allocation, otherwise
    /// `OutputCapacity`.
    pub fn ensure_capacity(
        &self,
        fills: usize,
        order_deltas: usize,
        level_deltas: usize,
    ) -> Result<(), MatchingError> {
        let fits = |length: usize, additional: usize, capacity: usize| {
            length
                .checked_add(additional)
                .is_some_and(|required| required <= capacity)
        };

        if fits(self.fills.len(), fills, self.fills.capacity())
            && fits(
                self.order_deltas.len(),
                order_deltas,
                self.order_deltas.capacity(),
            )
            && fits(
                self.level_deltas.len(),
                level_deltas,
                self.level_deltas.capacity(),
            )
        {
            Ok(())
        } else {
            Err(MatchingError::OutputCapacity)
        }
    }
}

/// Controlled failure returned by the matching core.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchingError {
    /// Startup memory reservation failed; this is a system failure, not a trading rejection.
    AllocationFailed,
    /// Engine limits are zero, overflow, or exceed the supported live-order cap.
    InvalidConfig,
    /// An instrument symbol, exact unit, or size bound is invalid.
    InvalidInstrumentSpec,
    /// A command contains an invalid or zero price.
    InvalidPrice,
    /// A command contains a zero or out-of-range size.
    InvalidSize,
    /// A client order ID is already live for the submitting account.
    DuplicateClientId,
    /// The public order ID does not identify a live order.
    OrderNotFound,
    /// The submitting account does not own the referenced order.
    WrongOwner,
    /// The public order ID belongs to a different engine identity domain.
    WrongEngine,
    /// A maker add or amend would cross the opposite side of the book.
    WouldCross,
    /// A taker would execute against a reached order from its own account.
    SelfTrade,
    /// A taker has no eligible opposite-side liquidity.
    NoLiquidity,
    /// The requested resulting book state exceeds configured logical limits.
    CapacityExceeded,
    /// A response buffer cannot hold the complete result before mutation.
    OutputCapacity,
    /// An exchange sequence or identity counter cannot advance safely.
    CounterExhausted,
    /// A restored snapshot failed validation.
    InvalidSnapshot,
}

impl fmt::Display for MatchingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::AllocationFailed => "startup memory reservation failed",
            Self::InvalidConfig => "invalid engine configuration",
            Self::InvalidInstrumentSpec => "invalid instrument specification",
            Self::InvalidPrice => "invalid price",
            Self::InvalidSize => "invalid size",
            Self::DuplicateClientId => "client order ID is already live for this account",
            Self::OrderNotFound => "order was not found",
            Self::WrongOwner => "account does not own the order",
            Self::WrongEngine => "order ID belongs to another engine",
            Self::WouldCross => "maker order would cross the opposite book",
            Self::SelfTrade => "taker would trade with the same account",
            Self::NoLiquidity => "no eligible opposite-side liquidity",
            Self::CapacityExceeded => "configured book capacity would be exceeded",
            Self::OutputCapacity => "response buffers cannot hold the complete result",
            Self::CounterExhausted => "engine identity counter is exhausted",
            Self::InvalidSnapshot => "snapshot failed validation",
        };
        formatter.write_str(message)
    }
}

impl MatchingError {
    /// Returns the stable machine-readable code for this failure.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AllocationFailed => "allocation_failed",
            Self::InvalidConfig => "invalid_config",
            Self::InvalidInstrumentSpec => "invalid_instrument_spec",
            Self::InvalidPrice => "invalid_price",
            Self::InvalidSize => "invalid_size",
            Self::DuplicateClientId => "duplicate_client_id",
            Self::OrderNotFound => "order_not_found",
            Self::WrongOwner => "wrong_owner",
            Self::WrongEngine => "wrong_engine",
            Self::WouldCross => "would_cross",
            Self::SelfTrade => "self_trade",
            Self::NoLiquidity => "no_liquidity",
            Self::CapacityExceeded => "capacity_exceeded",
            Self::OutputCapacity => "output_capacity",
            Self::CounterExhausted => "counter_exhausted",
            Self::InvalidSnapshot => "invalid_snapshot",
        }
    }
}

impl std::error::Error for MatchingError {}

#[cfg(test)]
mod tests {
    use super::{
        AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
        MAX_DECIMAL_SCALE, MAX_LIVE_ORDERS, MatchingError, MatchingResponse, OrderId, Price,
        Quantity, Side,
    };

    #[test]
    fn decimal_scale_accepts_exact_precision_through_the_supported_limit() {
        assert_eq!(
            DecimalScale::new(25, 2),
            Ok(DecimalScale {
                mantissa: 25,
                scale: 2
            })
        );
        assert_eq!(
            DecimalScale::new(1, MAX_DECIMAL_SCALE),
            Ok(DecimalScale {
                mantissa: 1,
                scale: MAX_DECIMAL_SCALE
            })
        );
    }

    #[test]
    fn decimal_scale_rejects_zero_mantissa_and_excessive_precision() {
        assert_eq!(
            DecimalScale::new(0, 0),
            Err(MatchingError::InvalidInstrumentSpec)
        );
        assert_eq!(
            DecimalScale::new(1, MAX_DECIMAL_SCALE + 1),
            Err(MatchingError::InvalidInstrumentSpec)
        );
        assert!(
            !DecimalScale {
                mantissa: 0,
                scale: 0
            }
            .is_valid()
        );
    }

    #[test]
    fn engine_config_defaults_to_the_documented_bounded_book() {
        let config = EngineConfig::default();

        assert_eq!(config.max_orders_per_level, 128);
        assert_eq!(config.max_levels, 1_024);
        assert_eq!(config.validate(), Ok(MAX_LIVE_ORDERS));
    }

    #[test]
    fn engine_config_accepts_smaller_limits_and_rejects_zero_overflow_or_oversize() {
        assert_eq!(
            EngineConfig {
                max_orders_per_level: 3,
                max_levels: 7,
            }
            .validate(),
            Ok(21)
        );
        assert_eq!(
            EngineConfig {
                max_orders_per_level: 0,
                max_levels: 1,
            }
            .validate(),
            Err(MatchingError::InvalidConfig)
        );
        assert_eq!(
            EngineConfig {
                max_orders_per_level: usize::MAX,
                max_levels: 2,
            }
            .validate(),
            Err(MatchingError::InvalidConfig)
        );
        assert_eq!(
            EngineConfig {
                max_orders_per_level: MAX_LIVE_ORDERS,
                max_levels: 2,
            }
            .validate(),
            Err(MatchingError::InvalidConfig)
        );
    }

    fn valid_spec() -> InstrumentSpec {
        InstrumentSpec {
            symbol: "BTC-USD".to_owned(),
            tick_size: DecimalScale {
                mantissa: 25,
                scale: 2,
            },
            lot_size: DecimalScale {
                mantissa: 1,
                scale: 3,
            },
            min_size: Quantity(1),
            max_size: Some(Quantity(10_000)),
        }
    }

    #[test]
    fn instrument_spec_accepts_exact_units_and_inclusive_size_bounds() {
        let spec = valid_spec();

        assert_eq!(spec.validate(), Ok(()));
        assert!(!spec.accepts_size(Quantity(0)));
        assert!(spec.accepts_size(Quantity(1)));
        assert!(spec.accepts_size(Quantity(10_000)));
        assert!(!spec.accepts_size(Quantity(10_001)));

        let mut unbounded_spec = spec;
        unbounded_spec.max_size = None;
        assert!(unbounded_spec.accepts_size(Quantity(u64::MAX)));
    }

    #[test]
    fn instrument_spec_rejects_blank_symbol_invalid_units_and_bad_bounds() {
        let mut spec = valid_spec();
        spec.symbol = "  ".to_owned();
        assert_eq!(spec.validate(), Err(MatchingError::InvalidInstrumentSpec));

        let mut spec = valid_spec();
        spec.tick_size.scale = MAX_DECIMAL_SCALE + 1;
        assert_eq!(spec.validate(), Err(MatchingError::InvalidInstrumentSpec));

        let mut spec = valid_spec();
        spec.min_size = Quantity(0);
        assert_eq!(spec.validate(), Err(MatchingError::InvalidInstrumentSpec));

        let mut spec = valid_spec();
        spec.max_size = Some(Quantity(0));
        assert_eq!(spec.validate(), Err(MatchingError::InvalidInstrumentSpec));
    }

    #[test]
    fn response_preallocates_for_maximum_sweep_and_can_be_reused() {
        let config = EngineConfig {
            max_orders_per_level: 2,
            max_levels: 3,
        };
        let mut response = MatchingResponse::with_capacity(&config).expect("valid config");

        assert!(response.fills.capacity() >= 6);
        assert!(response.order_deltas.capacity() >= 7);
        assert!(response.level_deltas.capacity() >= 3);
        assert_eq!(
            response.ensure_capacity(
                response.fills.capacity(),
                response.order_deltas.capacity(),
                response.level_deltas.capacity(),
            ),
            Ok(())
        );
        assert_eq!(
            response.ensure_capacity(response.fills.capacity() + 1, 0, 0),
            Err(MatchingError::OutputCapacity)
        );

        response.fills.push(super::Fill {
            taker_order_id: OrderId::new(EngineId(1), 8),
            taker_client_id: ClientOrderId(2),
            taker_account_id: AccountId(3),
            maker_order_id: OrderId::new(EngineId(1), 4),
            maker_client_id: ClientOrderId(5),
            maker_account_id: AccountId(6),
            price: Price(100),
            size: Quantity(1),
            taker_side: Side::Buy,
        });
        response.clear();
        assert!(response.fills.is_empty());
        assert!(response.fills.capacity() >= 6);
    }

    #[test]
    fn response_reserves_two_deltas_for_single_order_amends() {
        let config = EngineConfig {
            max_orders_per_level: 1,
            max_levels: 1,
        };
        let response = MatchingResponse::with_capacity(&config).expect("valid config");

        assert!(response.order_deltas.capacity() >= 2);
        assert!(response.level_deltas.capacity() >= 2);
        assert_eq!(response.ensure_capacity(0, 2, 2), Ok(()));
    }

    #[test]
    fn command_and_public_ids_round_trip_through_serde() {
        let command = Command::Add {
            account_id: AccountId(41),
            client_id: ClientOrderId(9),
            side: Side::Sell,
            price: Price(100_005),
            size: Quantity(12),
        };
        let serialized = serde_json::to_string(&command).expect("command serializes");
        assert!(serialized.starts_with(r#"{"type":"add""#));
        assert!(serialized.contains(r#""side":"sell""#));
        let decoded: Command = serde_json::from_str(&serialized).expect("command deserializes");
        assert_eq!(decoded, command);

        let order_id = OrderId::new(EngineId(7), u64::MAX);
        let serialized = serde_json::to_string(&order_id).expect("order ID serializes");
        let decoded: OrderId = serde_json::from_str(&serialized).expect("order ID deserializes");
        assert_eq!(decoded, order_id);
    }

    #[test]
    fn side_returns_the_opposite_book_side() {
        assert_eq!(Side::Buy.opposite(), Side::Sell);
        assert_eq!(Side::Sell.opposite(), Side::Buy);
    }

    #[test]
    fn matching_error_codes_are_stable_snake_case_values() {
        assert_eq!(
            MatchingError::InvalidInstrumentSpec.code(),
            "invalid_instrument_spec"
        );
        assert_eq!(MatchingError::CounterExhausted.code(), "counter_exhausted");
        assert_eq!(MatchingError::OutputCapacity.code(), "output_capacity");
    }
}
