"""Core matching engine data structures using msgspec Struct.

All structures are designed for:
- Fast serialization/deserialization (msgspec)
- Memory efficiency (gc=False where appropriate)
- Immutability where semantically correct (frozen=True for outputs)
"""

from __future__ import annotations

import msgspec

from .types import AccountId, ClientId, Lot, OrderId, OrderStatus, Side, Tick


class InstrumentSpec(msgspec.Struct, frozen=True, gc=False):
    """Specification for a tradeable instrument.

    Args:
        base (str): Base currency/asset (e.g., "BTC").
        quote (str): Quote currency (e.g., "USD").
        symbol (str): Display symbol (e.g., "BTC-USD").
        tick_size (float): Minimum price increment in quote currency.
        lot_size (float): Minimum size increment in base currency.
        min_size (Lot): Minimum order size in lots.
        max_size (Lot): Maximum order size in lots (0 = unlimited).
    """

    base: str
    quote: str
    symbol: str
    tick_size: float
    lot_size: float
    min_size: Lot = 1
    max_size: Lot = 0


class Order(msgspec.Struct, frozen=False, gc=False):
    """Represents an order stored in the order book.

    Mutable because size can be reduced on partial fills.

    Args:
        order_id (OrderId): Engine-generated internal order ID.
        client_id (ClientId): Caller-provided external order ID.
        account_id (AccountId): Account identifier for self-trade prevention.
        price (Tick): Price in ticks.
        size (Lot): Remaining size in lots.
        side (Side): Order side (BUY or SELL).
        timestamp (int): Monotonic sequence number for time priority.
    """

    order_id: OrderId
    client_id: ClientId
    account_id: AccountId
    price: Tick
    size: Lot
    side: Side
    timestamp: int

    def __setattr__(self, name: str, value: object) -> None:
        """Set attributes while enforcing size-only mutability.

        Args:
            name (str): Attribute name to set.
            value (object): Value to assign.

        Raises:
            AttributeError: If attempting to change a non-size field.
        """
        if name != "size" and hasattr(self, name):
            raise AttributeError(f"{name} is immutable on Order")
        super().__setattr__(name, value)


class Fill(msgspec.Struct, frozen=True, gc=False):
    """Represents a single trade execution.

    Immutable - fills are historical records.

    Args:
        taker_order_id (OrderId): Internal order ID of taker.
        maker_order_id (OrderId): Internal order ID of maker.
        taker_client_id (ClientId): Client ID of taker.
        maker_client_id (ClientId): Client ID of maker.
        taker_account_id (AccountId): Account ID of taker.
        maker_account_id (AccountId): Account ID of maker.
        price (Tick): Execution price in ticks.
        size (Lot): Execution size in lots.
        taker_side (Side): Side of the taker order.
    """

    taker_order_id: OrderId
    maker_order_id: OrderId
    taker_client_id: ClientId
    maker_client_id: ClientId
    taker_account_id: AccountId
    maker_account_id: AccountId
    price: Tick
    size: Lot
    taker_side: Side


class LevelDelta(msgspec.Struct, frozen=True, gc=False):
    """Represents a change to a price level in the book.

    Args:
        price (Tick): Price level in ticks.
        size (Lot): New total size at level (0 = level removed).
        side (Side): Side of the book (BUY = bid, SELL = ask).
        order_count (int): Number of orders at this level.
    """

    price: Tick
    size: Lot
    side: Side
    order_count: int


class OrderDelta(msgspec.Struct, frozen=True, gc=False):
    """Represents a change to an order's status.

    Args:
        order_id (OrderId): Internal order ID.
        client_id (ClientId): Caller-provided order ID.
        status (OrderStatus): New order status.
        remaining_size (Lot): Remaining size after the change.
        filled_size (Lot): Size filled in this operation (for fills).
        fill_price (Tick): Price at which fill occurred (0 if no fill).
    """

    order_id: OrderId
    client_id: ClientId
    status: OrderStatus
    remaining_size: Lot
    filled_size: Lot = 0
    fill_price: Tick = 0


class MatchingResponse(msgspec.Struct, frozen=True, gc=False):
    """Container for all deltas from a matching engine operation.

    Every operation returns a MatchingResponse, even if some lists are empty.
    This provides a consistent interface for event sourcing.

    Args:
        fills (list[Fill]): List of trade executions (empty for maker add/cancel).
        order_deltas (list[OrderDelta]): List of order status changes.
        level_deltas (list[LevelDelta]): List of book level changes.
    """

    fills: list[Fill]
    order_deltas: list[OrderDelta]
    level_deltas: list[LevelDelta]


class LevelSnapshot(msgspec.Struct, frozen=True):
    """Snapshot of a single price level for serialization.

    Args:
        price (Tick): Price in ticks.
        orders (list[Order]): List of orders at this level in FIFO order.
    """

    price: Tick
    orders: list[Order]


class OrderBookSnapshot(msgspec.Struct, frozen=True):
    """Serializable snapshot of the entire order book state.

    Can be used to:
    - Persist state to disk
    - Replicate state to another instance
    - Debug/inspect book state

    Args:
        bids (list[LevelSnapshot]): Bid levels from best (highest) to worst.
        asks (list[LevelSnapshot]): Ask levels from best (lowest) to worst.
        next_order_id (OrderId): Next internal order ID to generate.
        next_timestamp (int): Next timestamp for time priority.
    """

    bids: list[LevelSnapshot]
    asks: list[LevelSnapshot]
    next_order_id: OrderId
    next_timestamp: int
