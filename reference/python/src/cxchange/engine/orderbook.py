"""Order book data structures and utilities for matching engine.

Defines a two-sided order book with price-time priority and supporting
operations for adding, removing, and iterating orders.
"""

from __future__ import annotations

from collections import deque
from typing import Iterator

from sortedcontainers import SortedDict

from .structures import LevelDelta, Order
from .types import AccountId, ClientId, Lot, OrderId, Side, Tick


class PriceLevel:
    """A single price level with FIFO order queue.

    Maintains:
    - deque for O(1) front operations (matching)
    - dict for O(1) order lookup by ID
    - total_size for quick aggregation
    - order_count for constant-time counts
    """

    __slots__ = ("price", "total_size", "order_count", "orders", "order_map")

    def __init__(self, price: Tick) -> None:
        """Initialize a price level.

        Args:
            price (Tick): Price for this level in ticks.
        """
        self.price: Tick = price
        self.total_size: Lot = 0
        self.order_count: int = 0
        self.orders: deque[Order] = deque()
        self.order_map: dict[OrderId, Order] = {}

    def add_order(self, order: Order) -> None:
        """Add order to back of FIFO queue.

        Args:
            order (Order): Order to add.
        """
        self.orders.append(order)
        self.order_map[order.order_id] = order
        self.total_size += order.size
        self.order_count += 1

    def remove_order(self, order_id: OrderId) -> Order | None:
        """Remove order by ID from anywhere in queue.

        Args:
            order_id (OrderId): Order ID to remove.

        Returns:
            Order | None: Removed order if found.
        """
        order = self.order_map.get(order_id)
        if order is None:
            return None
        try:
            self.orders.remove(order)
        except ValueError:
            return None
        self.order_map.pop(order_id, None)
        self.total_size -= order.size
        self.order_count -= 1
        return order

    def peek_front(self) -> Order | None:
        """Peek at front order without removing.

        Returns:
            Order | None: Front order if present.
        """
        if not self.orders:
            return None
        return self.orders[0]

    def pop_front(self) -> Order | None:
        """Remove and return front order.

        Returns:
            Order | None: Front order if present.
        """
        if not self.orders:
            return None
        order = self.orders.popleft()
        self.order_map.pop(order.order_id, None)
        self.total_size -= order.size
        self.order_count -= 1
        return order

    def reduce_front(self, amount: Lot) -> None:
        """Reduce size of front order (partial fill).

        Args:
            amount (Lot): Amount to reduce.
        """
        if not self.orders:
            return
        front = self.orders[0]
        reduction = amount if amount <= front.size else front.size
        front.size -= reduction
        self.total_size -= reduction
        if front.size == 0:
            self.pop_front()

    def is_empty(self) -> bool:
        """Check if level has no orders.

        Returns:
            bool: True if no orders remain.
        """
        return self.order_count == 0


class OrderBook:
    """Two-sided order book with price-time priority.

    Implementation Details:
    - Bids: SortedDict with negated keys (-price) for descending order
    - Asks: SortedDict with natural keys for ascending order
    - order_to_level: O(1) lookup of order location

    SortedDict Key Strategy:
    - Bids use -price so peekitem(0) returns highest bid
    - Asks use price so peekitem(0) returns lowest ask
    """

    __slots__ = ("bids", "asks", "order_to_level")

    def __init__(self) -> None:
        """Initialize an empty order book."""
        self.bids: SortedDict = SortedDict()
        self.asks: SortedDict = SortedDict()
        self.order_to_level: dict[OrderId, tuple[Side, Tick]] = {}

    def get_best_bid(self) -> Tick | None:
        """Get the best bid price.

        Returns:
            Tick | None: Highest bid price if available.
        """
        if not self.bids:
            return None
        best_key, _ = self.bids.peekitem(0)
        return -best_key

    def get_best_ask(self) -> Tick | None:
        """Get the best ask price.

        Returns:
            Tick | None: Lowest ask price if available.
        """
        if not self.asks:
            return None
        best_key, _ = self.asks.peekitem(0)
        return best_key

    def get_level(self, side: Side, price: Tick) -> PriceLevel | None:
        """Get a price level by side and price.

        Args:
            side (Side): Side of the book.
            price (Tick): Price level.

        Returns:
            PriceLevel | None: Level if present.
        """
        if side == Side.BUY:
            return self.bids.get(-price)
        return self.asks.get(price)

    def get_or_create_level(self, side: Side, price: Tick) -> PriceLevel:
        """Get a price level, creating it if needed.

        Args:
            side (Side): Side of the book.
            price (Tick): Price level.

        Returns:
            PriceLevel: Existing or newly created level.
        """
        level = self.get_level(side, price)
        if level is not None:
            return level
        level = PriceLevel(price)
        if side == Side.BUY:
            self.bids[-price] = level
        else:
            self.asks[price] = level
        return level

    def remove_level_if_empty(self, side: Side, price: Tick) -> None:
        """Remove a price level if it has no orders.

        Args:
            side (Side): Side of the book.
            price (Tick): Price level.
        """
        level = self.get_level(side, price)
        if level is None or not level.is_empty():
            return
        if side == Side.BUY:
            self.bids.pop(-price, None)
        else:
            self.asks.pop(price, None)

    def add_order(self, order: Order) -> LevelDelta:
        """Add an order to the book and return the level delta.

        Args:
            order (Order): Order to add.

        Returns:
            LevelDelta: Updated level information.
        """
        level = self.get_or_create_level(order.side, order.price)
        level.add_order(order)
        self.order_to_level[order.order_id] = (order.side, order.price)
        return LevelDelta(
            price=order.price,
            size=level.total_size,
            side=order.side,
            order_count=level.order_count,
        )

    def remove_order(self, order_id: OrderId) -> tuple[Order | None, LevelDelta | None]:
        """Remove an order from the book by ID.

        Args:
            order_id (OrderId): Order ID to remove.

        Returns:
            tuple[Order | None, LevelDelta | None]: Removed order and level delta.
        """
        location = self.order_to_level.pop(order_id, None)
        if location is None:
            return None, None
        side, price = location
        level = self.get_level(side, price)
        if level is None:
            return None, None
        order = level.remove_order(order_id)
        if order is None:
            return None, None
        if level.is_empty():
            self.remove_level_if_empty(side, price)
            level_size = 0
            order_count = 0
        else:
            level_size = level.total_size
            order_count = level.order_count
        return (
            order,
            LevelDelta(
                price=price,
                size=level_size,
                side=side,
                order_count=order_count,
            ),
        )

    def get_order(self, order_id: OrderId) -> Order | None:
        """Get an order by internal order ID.

        Args:
            order_id (OrderId): Order ID to lookup.

        Returns:
            Order | None: Order if present.
        """
        location = self.order_to_level.get(order_id)
        if location is None:
            return None
        side, price = location
        level = self.get_level(side, price)
        if level is None:
            return None
        return level.order_map.get(order_id)

    def iter_levels(self, side: Side) -> Iterator[PriceLevel]:
        """Iterate levels for a side in best-to-worst order.

        Args:
            side (Side): Side to iterate.

        Returns:
            Iterator[PriceLevel]: Levels in price order.
        """
        if side == Side.BUY:
            return iter(self.bids.values())
        return iter(self.asks.values())

    def iter_levels_up_to(self, side: Side, limit_price: Tick) -> Iterator[PriceLevel]:
        """Iterate levels up to a price limit.

        Args:
            side (Side): Side to iterate.
            limit_price (Tick): Price limit (inclusive).

        Returns:
            Iterator[PriceLevel]: Levels within the limit.
        """
        if side == Side.BUY:
            for key, level in self.bids.items():
                price = -key
                if price < limit_price:
                    break
                yield level
        else:
            for price, level in self.asks.items():
                if price > limit_price:
                    break
                yield level

    def find_self_trade(
        self,
        side: Side,
        price: Tick,
        size: Lot,
        account_id: AccountId,
    ) -> ClientId | None:
        """Find a self-trade conflict in the book.

        Walks the opposite side in FIFO/price order up to the price limit and
        only reports a conflict if the same-account maker would actually be
        reached after consuming earlier liquidity.

        Args:
            side (Side): Incoming order side.
            price (Tick): Incoming order price limit.
            size (Lot): Size to match.
            account_id (AccountId): Account ID to check.

        Returns:
            ClientId | None: Conflicting client ID if reached.
        """
        remaining = size
        opposite_side = Side.SELL if side == Side.BUY else Side.BUY
        for level in self.iter_levels_up_to(opposite_side, price):
            for order in level.orders:
                if remaining <= 0:
                    return None
                if order.account_id == account_id:
                    return order.client_id
                fill_size = order.size if order.size <= remaining else remaining
                remaining -= fill_size
        return None

    def would_cross(self, side: Side, price: Tick) -> bool:
        """Check if a maker order would cross the spread.

        Args:
            side (Side): Order side.
            price (Tick): Order price.

        Returns:
            bool: True if order would cross the spread.
        """
        if side == Side.BUY:
            best_ask = self.get_best_ask()
            return best_ask is not None and price >= best_ask
        best_bid = self.get_best_bid()
        return best_bid is not None and price <= best_bid
