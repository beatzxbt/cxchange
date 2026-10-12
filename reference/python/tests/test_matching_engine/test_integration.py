"""Integration tests for end-to-end matching engine scenarios."""

from __future__ import annotations

import pytest

from cxchange.engine.engine import MatchingEngine
from cxchange.engine.exceptions import SelfTradeError
from cxchange.engine.structures import InstrumentSpec, MatchingResponse
from cxchange.engine.types import Side


def add_maker(
    engine: MatchingEngine,
    client_id: str,
    account_id: str,
    side: Side,
    price: int,
    size: int,
) -> None:
    """Add a maker order to the engine.

    Args:
        engine (MatchingEngine): Engine instance.
        client_id (str): Client order ID.
        account_id (str): Account identifier.
        side (Side): Order side.
        price (int): Price in ticks.
        size (int): Size in lots.
    """
    engine.add_maker_order(client_id, account_id, side, price, size)


def match_taker(
    engine: MatchingEngine,
    client_id: str,
    account_id: str,
    side: Side,
    limit_price: int,
    size: int,
) -> MatchingResponse:
    """Match a taker order against the engine.

    Args:
        engine (MatchingEngine): Engine instance.
        client_id (str): Client order ID.
        account_id (str): Account identifier.
        side (Side): Order side.
        limit_price (int): Price limit.
        size (int): Size in lots.

    Returns:
        MatchingResponse: Response for the match operation.
    """
    return engine.match_taker_order(client_id, account_id, side, limit_price, size)


def assert_level_sizes_consistent(engine: MatchingEngine) -> None:
    """Assert level total sizes equal sum of order sizes.

    Args:
        engine (MatchingEngine): Engine instance.
    """
    for side in (Side.BUY, Side.SELL):
        for level in engine._book.iter_levels(side):
            total = sum(order.size for order in level.orders)
            assert total == level.total_size


class TestMarketScenarios:
    """Realistic market scenarios testing full flows."""

    def test_build_book_then_take(self, engine: MatchingEngine) -> None:
        """Add multiple makers, then taker sweeps through."""
        add_maker(engine, "s1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "s2", "a2", Side.SELL, 101, 2)
        add_maker(engine, "s3", "a3", Side.SELL, 102, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 102, 5)
        assert len(response.fills) == 3
        assert engine.get_best_ask() == 102

    def test_interleaved_makers_and_takers(self, engine: MatchingEngine) -> None:
        """Alternating maker/taker operations."""
        add_maker(engine, "b1", "a1", Side.BUY, 95, 2)
        add_maker(engine, "s1", "a2", Side.SELL, 105, 2)
        match_taker(engine, "t1", "ta", Side.BUY, 105, 1)
        add_maker(engine, "s2", "a3", Side.SELL, 106, 1)
        match_taker(engine, "t2", "tb", Side.SELL, 95, 2)
        assert engine.get_best_bid() is None

    def test_cancel_while_taking(self, engine: MatchingEngine) -> None:
        """Cancel orders between taker operations."""
        add_maker(engine, "s1", "a1", Side.SELL, 100, 1)
        add_maker(engine, "s2", "a2", Side.SELL, 101, 1)
        match_taker(engine, "t1", "ta", Side.BUY, 101, 1)
        engine.cancel_maker_order("s2")
        assert engine.get_order_by_client_id("s2") is None

    def test_amend_price_improvement(self, engine: MatchingEngine) -> None:
        """Amend to better price, verify priority and best price."""
        add_maker(engine, "b1", "a1", Side.BUY, 95, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 94, 1)
        old_order = engine.get_order_by_client_id("b1")
        engine.amend_maker_order("b1", 96, 1)
        new_order = engine.get_order_by_client_id("b1")
        assert new_order is not None
        assert old_order is not None
        assert new_order.order_id != old_order.order_id
        assert engine.get_best_bid() == 96

    def test_self_trade_complex_scenario(self, engine: MatchingEngine) -> None:
        """Multiple accounts, interleaved orders, self-trade attempts."""
        add_maker(engine, "s1", "acct", Side.SELL, 100, 1)
        add_maker(engine, "s2", "other", Side.SELL, 101, 1)
        with pytest.raises(SelfTradeError):
            match_taker(engine, "t1", "acct", Side.BUY, 101, 2)

    def test_exhaust_book_one_side(self, engine: MatchingEngine) -> None:
        """Taker consumes all liquidity on one side."""
        add_maker(engine, "s1", "a1", Side.SELL, 100, 1)
        add_maker(engine, "s2", "a2", Side.SELL, 101, 1)
        match_taker(engine, "t1", "ta", Side.BUY, 101, 5)
        assert engine.get_best_ask() is None

    def test_ping_pong_orders(self, engine: MatchingEngine) -> None:
        """Orders bouncing around best bid/ask."""
        add_maker(engine, "b1", "a1", Side.BUY, 95, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 105, 2)
        engine.cancel_maker_order("b1")
        add_maker(engine, "b2", "a3", Side.BUY, 96, 1)
        engine.amend_maker_order("s1", 104, 2)
        assert (engine.get_best_bid(), engine.get_best_ask()) == (96, 104)
        match_taker(engine, "t1", "ta", Side.BUY, 104, 1)
        assert engine.get_best_ask() == 104


class TestDeterminism:
    """Verify deterministic behavior."""

    def test_same_sequence_same_result(self, spec: InstrumentSpec) -> None:
        """Same operations produce identical state."""
        engine_a = MatchingEngine(spec)
        engine_b = MatchingEngine(spec)

        add_maker(engine_a, "b1", "a1", Side.BUY, 100, 2)
        add_maker(engine_b, "b1", "a1", Side.BUY, 100, 2)
        add_maker(engine_a, "s1", "a2", Side.SELL, 110, 2)
        add_maker(engine_b, "s1", "a2", Side.SELL, 110, 2)
        match_taker(engine_a, "t1", "ta", Side.BUY, 110, 1)
        match_taker(engine_b, "t1", "ta", Side.BUY, 110, 1)
        engine_a.cancel_maker_order("b1")
        engine_b.cancel_maker_order("b1")

        assert engine_a.export_snapshot() == engine_b.export_snapshot()

    def test_snapshot_restore_same_result(self, engine: MatchingEngine) -> None:
        """Operations after restore match original."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)

        match_taker(engine, "t1", "ta", Side.BUY, 110, 1)
        match_taker(restored, "t1", "ta", Side.BUY, 110, 1)
        assert engine.export_snapshot() == restored.export_snapshot()


class TestStressScenarios:
    """Edge cases and boundary conditions."""

    def test_many_price_levels(self, engine: MatchingEngine) -> None:
        """Hundreds of price levels."""
        for i in range(100):
            add_maker(engine, f"b{i}", f"a{i}", Side.BUY, 100 + i, 1)
        assert engine.get_best_bid() == 199
        depth = engine.get_depth(Side.BUY, levels=5)
        assert len(depth) == 5
        assert depth[0][0] == 199

    def test_many_orders_per_level(self, engine: MatchingEngine) -> None:
        """Hundreds of orders at same price."""
        for i in range(100):
            add_maker(engine, f"b{i}", f"a{i}", Side.BUY, 100, 1)
        level = engine.get_level(Side.BUY, 100)
        assert level == (100, 100, 100)

    def test_rapid_add_cancel(self, engine: MatchingEngine) -> None:
        """Rapid add/cancel cycles."""
        for i in range(50):
            add_maker(engine, f"b{i}", f"a{i}", Side.BUY, 100, 1)
            engine.cancel_maker_order(f"b{i}")
        assert engine.get_best_bid() is None

    def test_single_tick_spread(self, engine: MatchingEngine) -> None:
        """Spread is exactly one tick."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 101, 1)
        best_bid = engine.get_best_bid()
        best_ask = engine.get_best_ask()
        assert (best_bid, best_ask) == (100, 101)
        assert best_bid is not None
        assert best_ask is not None
        assert (best_bid + best_ask) // 2 == 100

    def test_wide_spread(self, engine: MatchingEngine) -> None:
        """Very wide spread."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 200, 1)
        best_bid = engine.get_best_bid()
        best_ask = engine.get_best_ask()
        assert (best_bid, best_ask) == (100, 200)
        assert best_bid is not None
        assert best_ask is not None
        assert (best_bid + best_ask) // 2 == 150


class TestStateConsistency:
    """Verify internal state consistency after operations."""

    def test_order_count_matches(self, engine: MatchingEngine) -> None:
        """client_to_order size matches book order count."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 101, 1)
        add_maker(engine, "s1", "a3", Side.SELL, 110, 1)
        match_taker(engine, "t1", "ta", Side.BUY, 110, 1)
        engine.cancel_maker_order("b1")
        assert len(engine._client_to_order) == len(engine._book.order_to_level)

    def test_level_sizes_match(self, engine: MatchingEngine) -> None:
        """Level total_size matches sum of order sizes."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 2)
        add_maker(engine, "b2", "a2", Side.BUY, 100, 3)
        assert_level_sizes_consistent(engine)

    def test_no_orphan_orders(self, engine: MatchingEngine) -> None:
        """All orders in book are in order_to_level."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        for order_id, (side, price) in engine._book.order_to_level.items():
            level = engine._book.get_level(side, price)
            assert level is not None
            assert order_id in level.order_map

    def test_no_empty_levels(self, engine: MatchingEngine) -> None:
        """No levels with zero orders exist."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        for side in (Side.BUY, Side.SELL):
            for level in engine._book.iter_levels(side):
                assert not level.is_empty()
