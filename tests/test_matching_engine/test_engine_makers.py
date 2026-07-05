"""Tests for matching engine maker order operations."""

from __future__ import annotations

import pytest

from cxchange.matching_engine.engine import MatchingEngine
from cxchange.matching_engine.exceptions import (
    OrderAlreadyExistsError,
    OrderCrossesSpreadError,
    OrderNotFoundError,
)
from cxchange.matching_engine.types import OrderStatus, Side


class TestAddMakerOrder:
    """Tests for add_maker_order behavior."""

    def test_add_buy_order(self, engine: MatchingEngine) -> None:
        """Verify adding a buy maker order succeeds."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        assert response.order_deltas[0].status == OrderStatus.OPEN
        assert engine.has_order("c1")

    def test_add_sell_order(self, engine: MatchingEngine) -> None:
        """Verify adding a sell maker order succeeds."""
        response = engine.add_maker_order("c1", "a1", Side.SELL, 110, 5)
        assert response.order_deltas[0].status == OrderStatus.OPEN
        assert engine.has_order("c1")

    def test_response_structure(self, engine: MatchingEngine) -> None:
        """Verify maker response contains deltas and no fills."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        assert response.fills == []
        assert len(response.order_deltas) == 1
        assert len(response.level_deltas) == 1

    def test_order_delta_correct(self, engine: MatchingEngine) -> None:
        """Verify order delta fields are correct."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        delta = response.order_deltas[0]
        assert delta.client_id == "c1"
        assert delta.status == OrderStatus.OPEN
        assert delta.remaining_size == 5

    def test_level_delta_correct(self, engine: MatchingEngine) -> None:
        """Verify level delta reflects level state."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        level_delta = response.level_deltas[0]
        assert level_delta.price == 100
        assert level_delta.size == 5
        assert level_delta.side == Side.BUY
        assert level_delta.order_count == 1

    def test_internal_id_generated(self, engine: MatchingEngine) -> None:
        """Verify internal order IDs increment."""
        first = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        second = engine.add_maker_order("c2", "a2", Side.BUY, 101, 5)
        assert first.order_deltas[0].order_id == 1
        assert second.order_deltas[0].order_id == 2

    def test_timestamp_assigned(self, engine: MatchingEngine) -> None:
        """Verify orders receive timestamps for priority."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.timestamp == 0

    def test_add_multiple_same_price(self, engine: MatchingEngine) -> None:
        """Verify multiple orders at same price are accepted."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("c2", "a2", Side.BUY, 100, 2)
        level = engine.get_level(Side.BUY, 100)
        assert level == (100, 3, 2)

    def test_add_multiple_different_prices(self, engine: MatchingEngine) -> None:
        """Verify multiple price levels can be created."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("c2", "a2", Side.BUY, 105, 1)
        assert engine.get_best_bid() == 105

    def test_fifo_preserved(self, engine: MatchingEngine) -> None:
        """Verify FIFO priority within a price level."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("c2", "a2", Side.BUY, 100, 1)
        level = engine._book.get_level(Side.BUY, 100)
        assert level is not None
        level_orders = list(level.orders)
        assert level_orders[0].client_id == "c1"
        assert level_orders[1].client_id == "c2"

    def test_duplicate_client_id_raises(self, engine: MatchingEngine) -> None:
        """Verify duplicate client IDs are rejected."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        with pytest.raises(OrderAlreadyExistsError):
            engine.add_maker_order("c1", "a1", Side.BUY, 101, 1)

    def test_crosses_spread_buy_raises(self, engine: MatchingEngine) -> None:
        """Verify buy maker crossing spread is rejected."""
        engine.add_maker_order("s1", "a1", Side.SELL, 100, 1)
        with pytest.raises(OrderCrossesSpreadError):
            engine.add_maker_order("b1", "a2", Side.BUY, 100, 1)

    def test_crosses_spread_sell_raises(self, engine: MatchingEngine) -> None:
        """Verify sell maker crossing spread is rejected."""
        engine.add_maker_order("b1", "a1", Side.BUY, 100, 1)
        with pytest.raises(OrderCrossesSpreadError):
            engine.add_maker_order("s1", "a2", Side.SELL, 100, 1)

    def test_at_spread_crosses(self, engine: MatchingEngine) -> None:
        """Verify price equal to opposite best crosses."""
        engine.add_maker_order("s1", "a1", Side.SELL, 105, 1)
        with pytest.raises(OrderCrossesSpreadError):
            engine.add_maker_order("b1", "a2", Side.BUY, 105, 1)

    def test_first_order_never_crosses(self, engine: MatchingEngine) -> None:
        """Verify first order does not cross with empty opposite side."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        assert response.order_deltas[0].status == OrderStatus.OPEN

    def test_opposite_side_empty_never_crosses(self, engine: MatchingEngine) -> None:
        """Verify adding with empty opposite side succeeds."""
        response = engine.add_maker_order("c1", "a1", Side.SELL, 120, 1)
        assert response.order_deltas[0].status == OrderStatus.OPEN


class TestCancelMakerOrder:
    """Tests for canceling maker orders."""

    def test_cancel_existing(self, engine: MatchingEngine) -> None:
        """Verify cancelling an existing order succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.cancel_maker_order("c1")
        assert response.order_deltas[0].status == OrderStatus.CANCELLED

    def test_response_structure(self, engine: MatchingEngine) -> None:
        """Verify cancel response contains deltas and no fills."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.cancel_maker_order("c1")
        assert response.fills == []
        assert len(response.order_deltas) == 1
        assert len(response.level_deltas) == 1

    def test_order_delta_cancelled(self, engine: MatchingEngine) -> None:
        """Verify cancel order delta is cancelled with zero remaining."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        delta = engine.cancel_maker_order("c1").order_deltas[0]
        assert delta.status == OrderStatus.CANCELLED
        assert delta.remaining_size == 0

    def test_level_delta_updated(self, engine: MatchingEngine) -> None:
        """Verify cancel updates level delta to zero size."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        level_delta = engine.cancel_maker_order("c1").level_deltas[0]
        assert level_delta.size == 0
        assert level_delta.order_count == 0

    def test_level_removed_when_empty(self, engine: MatchingEngine) -> None:
        """Verify cancel removes empty level."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.cancel_maker_order("c1")
        assert engine.get_level(Side.BUY, 100) is None

    def test_cancel_nonexistent_raises(self, engine: MatchingEngine) -> None:
        """Verify cancelling a missing order raises."""
        with pytest.raises(OrderNotFoundError):
            engine.cancel_maker_order("missing")

    def test_cancel_already_cancelled_raises(self, engine: MatchingEngine) -> None:
        """Verify cancelling twice raises not found."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.cancel_maker_order("c1")
        with pytest.raises(OrderNotFoundError):
            engine.cancel_maker_order("c1")

    def test_order_not_gettable_after_cancel(self, engine: MatchingEngine) -> None:
        """Verify cancelled order is not retrievable."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.cancel_maker_order("c1")
        assert engine.get_order_by_client_id("c1") is None

    def test_client_id_reusable_after_cancel(self, engine: MatchingEngine) -> None:
        """Verify client ID can be reused after cancel."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.cancel_maker_order("c1")
        response = engine.add_maker_order("c1", "a1", Side.BUY, 101, 1)
        assert response.order_deltas[0].status == OrderStatus.OPEN


class TestAmendMakerOrder:
    """Tests for amending maker orders."""

    def test_amend_price_up(self, engine: MatchingEngine) -> None:
        """Verify amending to a higher price succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 101, 1)
        assert response.order_deltas[1].status == OrderStatus.OPEN
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.price == 101

    def test_amend_price_down(self, engine: MatchingEngine) -> None:
        """Verify amending to a lower price succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 99, 1)
        assert response.order_deltas[1].status == OrderStatus.OPEN
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.price == 99

    def test_amend_size_increase(self, engine: MatchingEngine) -> None:
        """Verify amending to a larger size succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 100, 2)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.size == 2
        assert response.order_deltas[1].remaining_size == 2

    def test_amend_size_decrease(self, engine: MatchingEngine) -> None:
        """Verify amending to a smaller size succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 2)
        response = engine.amend_maker_order("c1", 100, 1)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.size == 1
        assert response.order_deltas[1].remaining_size == 1

    def test_amend_price_and_size(self, engine: MatchingEngine) -> None:
        """Verify amending price and size together succeeds."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 2)
        response = engine.amend_maker_order("c1", 101, 3)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.price == 101
        assert order.size == 3
        assert response.order_deltas[1].remaining_size == 3

    def test_response_has_two_level_deltas(self, engine: MatchingEngine) -> None:
        """Verify amend response contains two level deltas."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 101, 1)
        assert len(response.level_deltas) == 2

    def test_new_order_id_assigned(self, engine: MatchingEngine) -> None:
        """Verify amend assigns a new internal order ID."""
        first = engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        old_id = first.order_deltas[0].order_id
        response = engine.amend_maker_order("c1", 101, 1)
        assert response.order_deltas[1].order_id != old_id

    def test_new_timestamp_assigned(self, engine: MatchingEngine) -> None:
        """Verify amend assigns a new timestamp."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        old_order = engine.get_order_by_client_id("c1")
        assert old_order is not None
        old_timestamp = old_order.timestamp
        engine.amend_maker_order("c1", 101, 1)
        new_order = engine.get_order_by_client_id("c1")
        assert new_order is not None
        assert new_order.timestamp != old_timestamp

    def test_client_id_preserved(self, engine: MatchingEngine) -> None:
        """Verify client ID is preserved after amend."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.amend_maker_order("c1", 101, 1)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.client_id == "c1"

    def test_account_id_preserved(self, engine: MatchingEngine) -> None:
        """Verify account ID is preserved after amend."""
        engine.add_maker_order("c1", "acct", Side.BUY, 100, 1)
        engine.amend_maker_order("c1", 101, 1)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.account_id == "acct"

    def test_priority_reset_on_amend(self, engine: MatchingEngine) -> None:
        """Verify time priority resets on amend."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.amend_maker_order("c1", 100, 1)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.timestamp == 1

    def test_moves_to_back_of_queue(self, engine: MatchingEngine) -> None:
        """Verify amended order moves to back of FIFO queue."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("c2", "a2", Side.BUY, 100, 1)
        engine.amend_maker_order("c1", 100, 1)
        level = engine._book.get_level(Side.BUY, 100)
        assert level is not None
        order_ids = [order.client_id for order in level.orders]
        assert order_ids == ["c2", "c1"]

    def test_amend_nonexistent_raises(self, engine: MatchingEngine) -> None:
        """Verify amending a missing order raises."""
        with pytest.raises(OrderNotFoundError):
            engine.amend_maker_order("missing", 100, 1)

    def test_amend_would_cross_raises(self, engine: MatchingEngine) -> None:
        """Verify amend that crosses spread raises."""
        engine.add_maker_order("b1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("s1", "a2", Side.SELL, 110, 1)
        with pytest.raises(OrderCrossesSpreadError):
            engine.amend_maker_order("b1", 120, 1)

    def test_amend_emits_replaced_delta_for_old_order(
        self, engine: MatchingEngine
    ) -> None:
        """Verify amend emits a REPLACED delta for the old order_id first."""
        first = engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        old_id = first.order_deltas[0].order_id
        response = engine.amend_maker_order("c1", 101, 1)
        assert len(response.order_deltas) == 2
        assert response.order_deltas[0].order_id == old_id
        assert response.order_deltas[0].status == OrderStatus.REPLACED
        assert response.order_deltas[0].remaining_size == 0

    def test_amend_replaced_delta_before_open_delta(
        self, engine: MatchingEngine
    ) -> None:
        """Verify old order is REPLACED before new order is OPEN."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 101, 1)
        assert response.order_deltas[0].status == OrderStatus.REPLACED
        assert response.order_deltas[1].status == OrderStatus.OPEN

    def test_amend_new_order_delta_has_new_id(self, engine: MatchingEngine) -> None:
        """Verify the OPEN delta uses the new internal order ID."""
        first = engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        old_id = first.order_deltas[0].order_id
        response = engine.amend_maker_order("c1", 101, 1)
        new_id = response.order_deltas[1].order_id
        assert new_id != old_id
        assert response.order_deltas[1].status == OrderStatus.OPEN

    def test_amend_old_order_no_longer_in_book(self, engine: MatchingEngine) -> None:
        """Verify the old internal order ID is removed from the book."""
        first = engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        old_id = first.order_deltas[0].order_id
        engine.amend_maker_order("c1", 101, 1)
        assert engine.get_order(old_id) is None

    def test_amend_same_values_allowed(self, engine: MatchingEngine) -> None:
        """Verify amending to same values is allowed."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        response = engine.amend_maker_order("c1", 100, 1)
        assert response.order_deltas[1].status == OrderStatus.OPEN
