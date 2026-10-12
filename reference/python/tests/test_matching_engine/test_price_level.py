"""Tests for PriceLevel FIFO behavior and size tracking."""

from __future__ import annotations

from cxchange.engine.orderbook import PriceLevel
from cxchange.engine.structures import Order
from cxchange.engine.types import Side


def make_order(
    order_id: int,
    price: int = 100,
    size: int = 1,
    side: Side = Side.BUY,
    client_id: str | None = None,
    account_id: str = "acct",
) -> Order:
    """Create an Order for testing.

    Args:
        order_id (int): Internal order ID.
        price (int): Order price in ticks.
        size (int): Order size in lots.
        side (Side): Order side.
        client_id (str | None): Client ID override.
        account_id (str): Account identifier.

    Returns:
        Order: Constructed order instance.
    """
    return Order(
        order_id=order_id,
        client_id=client_id or f"c{order_id}",
        account_id=account_id,
        price=price,
        size=size,
        side=side,
        timestamp=order_id,
    )


class TestPriceLevelCreation:
    """Tests for PriceLevel creation."""

    def test_empty_level(self) -> None:
        """Verify a new level starts empty."""
        level = PriceLevel(100)
        assert level.total_size == 0
        assert level.order_count == 0
        assert level.is_empty()

    def test_price_stored(self) -> None:
        """Verify the level stores its price."""
        level = PriceLevel(105)
        assert level.price == 105


class TestPriceLevelAddOrder:
    """Tests for adding orders to a PriceLevel."""

    def test_add_single_order(self) -> None:
        """Verify adding a single order updates state."""
        level = PriceLevel(100)
        order = make_order(1, size=2)
        level.add_order(order)
        assert level.order_count == 1
        assert level.total_size == 2

    def test_add_multiple_orders_fifo(self) -> None:
        """Verify FIFO ordering is preserved on add."""
        level = PriceLevel(100)
        first = make_order(1)
        second = make_order(2)
        level.add_order(first)
        level.add_order(second)
        assert list(level.orders)[0].order_id == 1
        assert list(level.orders)[1].order_id == 2

    def test_total_size_updated(self) -> None:
        """Verify total size aggregates added orders."""
        level = PriceLevel(100)
        level.add_order(make_order(1, size=3))
        level.add_order(make_order(2, size=5))
        assert level.total_size == 8

    def test_order_map_updated(self) -> None:
        """Verify order map tracks orders by ID."""
        level = PriceLevel(100)
        order = make_order(1)
        level.add_order(order)
        assert level.order_map[1] is order


class TestPriceLevelRemoveOrder:
    """Tests for removing orders from a PriceLevel."""

    def test_remove_existing(self) -> None:
        """Verify removing an existing order returns it."""
        level = PriceLevel(100)
        order = make_order(1, size=2)
        level.add_order(order)
        removed = level.remove_order(1)
        assert removed is order
        assert level.is_empty()

    def test_remove_nonexistent(self) -> None:
        """Verify removing a missing order returns None."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        removed = level.remove_order(99)
        assert removed is None
        assert level.order_count == 1

    def test_remove_from_middle(self) -> None:
        """Verify removing an order from the middle preserves order."""
        level = PriceLevel(100)
        first = make_order(1)
        middle = make_order(2)
        last = make_order(3)
        level.add_order(first)
        level.add_order(middle)
        level.add_order(last)
        removed = level.remove_order(2)
        assert removed is middle
        assert [order.order_id for order in level.orders] == [1, 3]

    def test_total_size_decremented(self) -> None:
        """Verify total size decreases after removal."""
        level = PriceLevel(100)
        level.add_order(make_order(1, size=2))
        level.add_order(make_order(2, size=3))
        level.remove_order(1)
        assert level.total_size == 3

    def test_order_map_cleared(self) -> None:
        """Verify order map removes deleted orders."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        level.remove_order(1)
        assert 1 not in level.order_map


class TestPriceLevelPeek:
    """Tests for peeking at PriceLevel front."""

    def test_peek_empty(self) -> None:
        """Verify peek returns None when empty."""
        level = PriceLevel(100)
        assert level.peek_front() is None

    def test_peek_single(self) -> None:
        """Verify peek returns the single order."""
        level = PriceLevel(100)
        order = make_order(1)
        level.add_order(order)
        assert level.peek_front() is order

    def test_peek_multiple_returns_first(self) -> None:
        """Verify peek returns the first order in FIFO."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        level.add_order(make_order(2))
        front = level.peek_front()
        assert front is not None
        assert front.order_id == 1

    def test_peek_does_not_remove(self) -> None:
        """Verify peek does not remove the order."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        _ = level.peek_front()
        assert level.order_count == 1


class TestPriceLevelPop:
    """Tests for popping from PriceLevel front."""

    def test_pop_empty(self) -> None:
        """Verify pop returns None when empty."""
        level = PriceLevel(100)
        assert level.pop_front() is None

    def test_pop_single(self) -> None:
        """Verify pop removes and returns the only order."""
        level = PriceLevel(100)
        order = make_order(1)
        level.add_order(order)
        assert level.pop_front() is order
        assert level.is_empty()

    def test_pop_returns_front(self) -> None:
        """Verify pop returns the FIFO front order."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        level.add_order(make_order(2))
        first = level.pop_front()
        assert first is not None
        assert first.order_id == 1

    def test_pop_removes_from_level(self) -> None:
        """Verify pop removes order from internal tracking."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        _ = level.pop_front()
        assert level.order_count == 0
        assert 1 not in level.order_map

    def test_pop_fifo_order(self) -> None:
        """Verify sequential pops follow FIFO order."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        level.add_order(make_order(2))
        first = level.pop_front()
        assert first is not None
        assert first.order_id == 1
        second = level.pop_front()
        assert second is not None
        assert second.order_id == 2


class TestPriceLevelReduce:
    """Tests for reducing front order size."""

    def test_reduce_partial(self) -> None:
        """Verify reducing partially decrements size."""
        level = PriceLevel(100)
        order = make_order(1, size=5)
        level.add_order(order)
        level.reduce_front(2)
        assert order.size == 3

    def test_reduce_to_zero(self) -> None:
        """Verify reducing to zero removes the order."""
        level = PriceLevel(100)
        level.add_order(make_order(1, size=2))
        level.reduce_front(2)
        assert level.is_empty()

    def test_total_size_decremented(self) -> None:
        """Verify total size decreases on reduction."""
        level = PriceLevel(100)
        level.add_order(make_order(1, size=5))
        level.reduce_front(3)
        assert level.total_size == 2


class TestPriceLevelIsEmpty:
    """Tests for empty state detection."""

    def test_empty_after_creation(self) -> None:
        """Verify new level reports empty."""
        level = PriceLevel(100)
        assert level.is_empty()

    def test_not_empty_after_add(self) -> None:
        """Verify level reports not empty after add."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        assert not level.is_empty()

    def test_empty_after_all_removed(self) -> None:
        """Verify level reports empty after removals."""
        level = PriceLevel(100)
        level.add_order(make_order(1))
        level.pop_front()
        assert level.is_empty()
