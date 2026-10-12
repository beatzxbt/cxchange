"""Tests for matching engine query methods."""

from __future__ import annotations

from cxchange.engine.engine import MatchingEngine
from cxchange.engine.structures import InstrumentSpec
from cxchange.engine.types import Side


class TestEngineCreation:
    """Tests for engine initialization."""

    def test_with_spec(self, spec: InstrumentSpec) -> None:
        """Verify engine can be created with a spec."""
        engine = MatchingEngine(spec)
        assert engine.spec.symbol == "BTC-USD"

    def test_empty_book(self, engine: MatchingEngine) -> None:
        """Verify a new engine has empty best prices."""
        assert engine.get_best_bid() is None
        assert engine.get_best_ask() is None

    def test_spec_accessible(self, spec: InstrumentSpec) -> None:
        """Verify engine exposes its spec."""
        engine = MatchingEngine(spec)
        assert engine.spec is spec


class TestEngineGetOrder:
    """Tests for order lookup APIs."""

    def test_nonexistent_by_internal_id(self, engine: MatchingEngine) -> None:
        """Verify missing internal ID returns None."""
        assert engine.get_order(1) is None

    def test_nonexistent_by_client_id(self, engine: MatchingEngine) -> None:
        """Verify missing client ID returns None."""
        assert engine.get_order_by_client_id("c1") is None

    def test_existing_by_internal_id(self, engine: MatchingEngine) -> None:
        """Verify lookup by internal ID returns the order."""
        response = engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        order_id = response.order_deltas[0].order_id
        order = engine.get_order(order_id)
        assert order is not None
        assert order.client_id == "c1"

    def test_existing_by_client_id(self, engine: MatchingEngine) -> None:
        """Verify lookup by client ID returns the order."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        order = engine.get_order_by_client_id("c1")
        assert order is not None
        assert order.order_id == 1

    def test_has_order(self, engine: MatchingEngine) -> None:
        """Verify has_order reflects presence of an order."""
        assert not engine.has_order("c1")
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 5)
        assert engine.has_order("c1")


class TestEngineBookQueries:
    """Tests for book query helpers."""

    def test_best_bid_empty(self, engine: MatchingEngine) -> None:
        """Verify best bid is None when empty."""
        assert engine.get_best_bid() is None

    def test_best_ask_empty(self, engine: MatchingEngine) -> None:
        """Verify best ask is None when empty."""
        assert engine.get_best_ask() is None

    def test_best_bid_with_orders(self, engine: MatchingEngine) -> None:
        """Verify best bid reflects highest bid."""
        engine.add_maker_order("c1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("c2", "a2", Side.BUY, 105, 1)
        assert engine.get_best_bid() == 105

    def test_best_ask_with_orders(self, engine: MatchingEngine) -> None:
        """Verify best ask reflects lowest ask."""
        engine.add_maker_order("c1", "a1", Side.SELL, 110, 1)
        engine.add_maker_order("c2", "a2", Side.SELL, 108, 1)
        assert engine.get_best_ask() == 108

    def test_get_level(self, engine: MatchingEngine) -> None:
        """Verify level query returns price, size, count."""
        engine.add_maker_order("b1", "a1", Side.BUY, 100, 2)
        level = engine.get_level(Side.BUY, 100)
        assert level == (100, 2, 1)

    def test_get_depth(self, engine: MatchingEngine) -> None:
        """Verify depth returns ordered levels."""
        engine.add_maker_order("b1", "a1", Side.BUY, 100, 1)
        engine.add_maker_order("b2", "a2", Side.BUY, 105, 2)
        depth = engine.get_depth(Side.BUY, levels=2)
        assert depth == [(105, 2, 1), (100, 1, 1)]
