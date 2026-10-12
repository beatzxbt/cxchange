"""Matching engine for single-symbol, integer-tick order matching.

Provides a correctness-first, deterministic engine with maker/taker operations,
price-time priority, and snapshot export/import support.
"""

from __future__ import annotations

from typing import Iterable

from .exceptions import (
    NoLiquidityError,
    OrderAlreadyExistsError,
    OrderCrossesSpreadError,
    OrderNotFoundError,
    SelfTradeError,
)
from .orderbook import OrderBook
from .structures import (
    Fill,
    InstrumentSpec,
    LevelDelta,
    LevelSnapshot,
    MatchingResponse,
    Order,
    OrderBookSnapshot,
    OrderDelta,
)
from .types import AccountId, ClientId, Lot, OrderId, OrderStatus, Side, Tick


class MatchingEngine:
    """Single-symbol matching engine with price-time priority.

    Design Principles:
    - Correctness over performance
    - Deterministic (same inputs = same outputs)
    - Stateless history (caller handles audit)
    - Trust caller for validation

    State:
    - _book: OrderBook instance
    - _next_order_id: Counter for internal IDs
    - _next_timestamp: Counter for time priority
    - _client_to_order: Map client ID -> internal ID
    - _spec: Instrument specification
    """

    __slots__ = (
        "_book",
        "_next_order_id",
        "_next_timestamp",
        "_client_to_order",
        "_spec",
    )

    def __init__(self, spec: InstrumentSpec) -> None:
        """Initialize the matching engine.

        Args:
            spec (InstrumentSpec): Instrument specification.
        """
        self._spec = spec
        self._book = OrderBook()
        self._next_order_id = 1
        self._next_timestamp = 0
        self._client_to_order: dict[ClientId, OrderId] = {}

    def _generate_order_id(self) -> OrderId:
        """Generate the next internal order ID.

        Returns:
            OrderId: Next internal order ID.
        """
        order_id = self._next_order_id
        self._next_order_id += 1
        return order_id

    def _generate_timestamp(self) -> int:
        """Generate the next time-priority timestamp.

        Returns:
            int: Monotonic timestamp value.
        """
        timestamp = self._next_timestamp
        self._next_timestamp += 1
        return timestamp

    @staticmethod
    def _clone_order(order: Order) -> Order:
        """Clone an order for snapshot isolation.

        Args:
            order (Order): Order to clone.

        Returns:
            Order: New order instance with identical fields.
        """
        return Order(
            order_id=order.order_id,
            client_id=order.client_id,
            account_id=order.account_id,
            price=order.price,
            size=order.size,
            side=order.side,
            timestamp=order.timestamp,
        )

    @property
    def spec(self) -> InstrumentSpec:
        """Return the engine's instrument specification.

        Returns:
            InstrumentSpec: Specification for this engine.
        """
        return self._spec

    def get_order(self, order_id: OrderId) -> Order | None:
        """Get an order by internal ID.

        Args:
            order_id (OrderId): Internal order ID.

        Returns:
            Order | None: Order if present.
        """
        return self._book.get_order(order_id)

    def get_order_by_client_id(self, client_id: ClientId) -> Order | None:
        """Get an order by client-provided ID.

        Args:
            client_id (ClientId): Client order ID.

        Returns:
            Order | None: Order if present.
        """
        order_id = self._client_to_order.get(client_id)
        if order_id is None:
            return None
        return self._book.get_order(order_id)

    def has_order(self, client_id: ClientId) -> bool:
        """Check if an order exists for a client ID.

        Args:
            client_id (ClientId): Client order ID.

        Returns:
            bool: True if order exists.
        """
        return client_id in self._client_to_order

    def get_best_bid(self) -> Tick | None:
        """Get the best bid price.

        Returns:
            Tick | None: Best bid price if present.
        """
        return self._book.get_best_bid()

    def get_best_ask(self) -> Tick | None:
        """Get the best ask price.

        Returns:
            Tick | None: Best ask price if present.
        """
        return self._book.get_best_ask()

    def get_level(self, side: Side, price: Tick) -> tuple[Tick, Lot, int] | None:
        """Get level information for a given side/price.

        Args:
            side (Side): Book side.
            price (Tick): Price level.

        Returns:
            tuple[Tick, Lot, int] | None: (price, size, order_count) if present.
        """
        level = self._book.get_level(side, price)
        if level is None:
            return None
        return level.price, level.total_size, level.order_count

    def get_depth(self, side: Side, levels: int = 10) -> list[tuple[Tick, Lot, int]]:
        """Get depth snapshot for a side.

        Args:
            side (Side): Book side.
            levels (int): Number of price levels to include.

        Returns:
            list[tuple[Tick, Lot, int]]: Level tuples in best-to-worst order.
        """
        depth: list[tuple[Tick, Lot, int]] = []
        for level in self._book.iter_levels(side):
            depth.append((level.price, level.total_size, level.order_count))
            if len(depth) >= levels:
                break
        return depth

    def add_maker_order(
        self,
        client_id: ClientId,
        account_id: AccountId,
        side: Side,
        price: Tick,
        size: Lot,
    ) -> MatchingResponse:
        """Add a maker order to the book.

        Preconditions (caller must ensure):
        - price > 0
        - size > 0
        - client_id is unique

        Raises:
            OrderAlreadyExistsError: Client ID already in use.
            OrderCrossesSpreadError: Order would cross spread.

        Returns:
            MatchingResponse: Response containing order and level deltas.
        """
        if client_id in self._client_to_order:
            raise OrderAlreadyExistsError(client_id)
        if self._book.would_cross(side, price):
            opposite_best = (
                self._book.get_best_ask()
                if side == Side.BUY
                else self._book.get_best_bid()
            )
            if opposite_best is None:
                opposite_best = price
            raise OrderCrossesSpreadError(client_id, side, price, opposite_best)

        order_id = self._generate_order_id()
        timestamp = self._generate_timestamp()
        order = Order(
            order_id=order_id,
            client_id=client_id,
            account_id=account_id,
            price=price,
            size=size,
            side=side,
            timestamp=timestamp,
        )
        level_delta = self._book.add_order(order)
        self._client_to_order[client_id] = order_id

        return MatchingResponse(
            fills=[],
            order_deltas=[
                OrderDelta(
                    order_id=order_id,
                    client_id=client_id,
                    status=OrderStatus.OPEN,
                    remaining_size=size,
                )
            ],
            level_deltas=[level_delta],
        )

    def cancel_maker_order(self, client_id: ClientId) -> MatchingResponse:
        """Cancel a maker order.

        Args:
            client_id (ClientId): Client order ID.

        Raises:
            OrderNotFoundError: Order not found.

        Returns:
            MatchingResponse: Response containing order and level deltas.
        """
        order_id = self._client_to_order.get(client_id)
        if order_id is None:
            raise OrderNotFoundError(client_id)
        order, level_delta = self._book.remove_order(order_id)
        if order is None:
            raise OrderNotFoundError(client_id)
        self._client_to_order.pop(client_id, None)

        return MatchingResponse(
            fills=[],
            order_deltas=[
                OrderDelta(
                    order_id=order_id,
                    client_id=client_id,
                    status=OrderStatus.CANCELLED,
                    remaining_size=0,
                )
            ],
            level_deltas=[level_delta] if level_delta is not None else [],
        )

    def amend_maker_order(
        self,
        client_id: ClientId,
        new_price: Tick,
        new_size: Lot,
    ) -> MatchingResponse:
        """Amend a maker order (always resets time priority).

        Implementation: Remove old order, add new order with fresh timestamp.

        Args:
            client_id (ClientId): Client order ID.
            new_price (Tick): New price in ticks.
            new_size (Lot): New size in lots.

        Raises:
            OrderNotFoundError: Order not found.
            OrderCrossesSpreadError: New price would cross spread.

        Returns:
            MatchingResponse: Response containing order and level deltas.
        """
        order_id = self._client_to_order.get(client_id)
        if order_id is None:
            raise OrderNotFoundError(client_id)
        existing_order = self._book.get_order(order_id)
        if existing_order is None:
            raise OrderNotFoundError(client_id)

        side = existing_order.side
        account_id = existing_order.account_id
        if self._book.would_cross(side, new_price):
            opposite_best = (
                self._book.get_best_ask()
                if side == Side.BUY
                else self._book.get_best_bid()
            )
            if opposite_best is None:
                opposite_best = new_price
            raise OrderCrossesSpreadError(client_id, side, new_price, opposite_best)

        _, old_level_delta = self._book.remove_order(order_id)
        self._client_to_order.pop(client_id, None)

        new_order_id = self._generate_order_id()
        new_timestamp = self._generate_timestamp()
        new_order = Order(
            order_id=new_order_id,
            client_id=client_id,
            account_id=account_id,
            price=new_price,
            size=new_size,
            side=side,
            timestamp=new_timestamp,
        )
        new_level_delta = self._book.add_order(new_order)
        self._client_to_order[client_id] = new_order_id

        level_deltas: list[LevelDelta] = []
        if old_level_delta is not None:
            level_deltas.append(old_level_delta)
        level_deltas.append(new_level_delta)

        order_deltas: list[OrderDelta] = [
            OrderDelta(
                order_id=order_id,
                client_id=client_id,
                status=OrderStatus.REPLACED,
                remaining_size=0,
            ),
            OrderDelta(
                order_id=new_order_id,
                client_id=client_id,
                status=OrderStatus.OPEN,
                remaining_size=new_size,
            ),
        ]

        return MatchingResponse(
            fills=[],
            order_deltas=order_deltas,
            level_deltas=level_deltas,
        )

    def match_taker_order(
        self,
        client_id: ClientId,
        account_id: AccountId,
        side: Side,
        limit_price: Tick,
        size: Lot,
    ) -> MatchingResponse:
        """Match a taker order against the book.

        Behavior:
        - Matches against opposite side from best price
        - Stops at price limit
        - Partial fills are success (remainder discarded)
        - Self-trade prevention: rejects if any match would be same account

        Args:
            client_id (ClientId): Client order ID.
            account_id (AccountId): Account identifier.
            side (Side): Taker order side.
            limit_price (Tick): Limit price.
            size (Lot): Size in lots.

        Raises:
            SelfTradeError: Order would self-trade.
            NoLiquidityError: No liquidity at or better than price.

        Returns:
            MatchingResponse: Response with fills, order deltas, and level deltas.
        """
        conflicting = self._book.find_self_trade(side, limit_price, size, account_id)
        if conflicting is not None:
            raise SelfTradeError(client_id, account_id, conflicting)

        remaining = size
        taker_order_id = self._generate_order_id()
        self._generate_timestamp()

        fills: list[Fill] = []
        order_deltas: list[OrderDelta] = []
        level_deltas: list[LevelDelta] = []
        affected_levels: list[tuple[Side, Tick]] = []
        seen_levels: set[tuple[Side, Tick]] = set()

        opposite_side = Side.SELL if side == Side.BUY else Side.BUY

        for level in self._book.iter_levels_up_to(opposite_side, limit_price):
            while remaining > 0 and not level.is_empty():
                maker = level.peek_front()
                if maker is None:
                    break
                fill_size = maker.size if maker.size <= remaining else remaining

                fills.append(
                    Fill(
                        taker_order_id=taker_order_id,
                        maker_order_id=maker.order_id,
                        taker_client_id=client_id,
                        maker_client_id=maker.client_id,
                        taker_account_id=account_id,
                        maker_account_id=maker.account_id,
                        price=level.price,
                        size=fill_size,
                        taker_side=side,
                    )
                )

                remaining -= fill_size

                if fill_size >= maker.size:
                    removed = level.pop_front()
                    if removed is None:
                        break
                    self._book.order_to_level.pop(removed.order_id, None)
                    self._client_to_order.pop(removed.client_id, None)
                    order_deltas.append(
                        OrderDelta(
                            order_id=removed.order_id,
                            client_id=removed.client_id,
                            status=OrderStatus.FILLED,
                            remaining_size=0,
                            filled_size=fill_size,
                            fill_price=level.price,
                        )
                    )
                else:
                    level.reduce_front(fill_size)
                    order_deltas.append(
                        OrderDelta(
                            order_id=maker.order_id,
                            client_id=maker.client_id,
                            status=OrderStatus.PARTIALLY_FILLED,
                            remaining_size=maker.size,
                            filled_size=fill_size,
                            fill_price=level.price,
                        )
                    )

                key = (opposite_side, level.price)
                if key not in seen_levels:
                    seen_levels.add(key)
                    affected_levels.append(key)

                if remaining == 0:
                    break
            if remaining == 0:
                break

        filled_size = size - remaining
        if filled_size == 0:
            raise NoLiquidityError(client_id, side, limit_price)

        for level_side, level_price in affected_levels:
            level = self._book.get_level(level_side, level_price)
            if level is None or level.is_empty():
                self._book.remove_level_if_empty(level_side, level_price)
                level_deltas.append(
                    LevelDelta(
                        price=level_price,
                        size=0,
                        side=level_side,
                        order_count=0,
                    )
                )
            else:
                level_deltas.append(
                    LevelDelta(
                        price=level_price,
                        size=level.total_size,
                        side=level_side,
                        order_count=level.order_count,
                    )
                )

        taker_status = (
            OrderStatus.FILLED if remaining == 0 else OrderStatus.PARTIALLY_FILLED
        )
        order_deltas.insert(
            0,
            OrderDelta(
                order_id=taker_order_id,
                client_id=client_id,
                status=taker_status,
                remaining_size=0,
                filled_size=filled_size,
            ),
        )

        return MatchingResponse(
            fills=fills,
            order_deltas=order_deltas,
            level_deltas=level_deltas,
        )

    def export_snapshot(self) -> OrderBookSnapshot:
        """Export current state as serializable snapshot.

        Returns:
            OrderBookSnapshot: Snapshot of current book and counters.
        """
        bids: list[LevelSnapshot] = []
        for level in self._book.iter_levels(Side.BUY):
            bids.append(
                LevelSnapshot(
                    price=level.price,
                    orders=[self._clone_order(order) for order in level.orders],
                )
            )

        asks: list[LevelSnapshot] = []
        for level in self._book.iter_levels(Side.SELL):
            asks.append(
                LevelSnapshot(
                    price=level.price,
                    orders=[self._clone_order(order) for order in level.orders],
                )
            )

        return OrderBookSnapshot(
            bids=bids,
            asks=asks,
            next_order_id=self._next_order_id,
            next_timestamp=self._next_timestamp,
        )

    @classmethod
    def from_snapshot(
        cls,
        snapshot: OrderBookSnapshot,
        spec: InstrumentSpec,
    ) -> "MatchingEngine":
        """Restore engine from a snapshot.

        Args:
            snapshot (OrderBookSnapshot): Serialized snapshot.
            spec (InstrumentSpec): Instrument specification.

        Returns:
            MatchingEngine: Restored matching engine instance.
        """
        engine = cls(spec)
        engine._next_order_id = snapshot.next_order_id
        engine._next_timestamp = snapshot.next_timestamp

        def restore(levels: Iterable[LevelSnapshot]) -> None:
            """Restore levels into the engine's book.

            Args:
                levels (Iterable[LevelSnapshot]): Level snapshots to restore.
            """
            for level_snapshot in levels:
                for order in level_snapshot.orders:
                    cloned = engine._clone_order(order)
                    engine._book.add_order(cloned)
                    engine._client_to_order[cloned.client_id] = cloned.order_id

        restore(snapshot.bids)
        restore(snapshot.asks)
        return engine
