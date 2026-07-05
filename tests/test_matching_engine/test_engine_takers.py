"""Tests for matching engine taker order matching behavior."""

from __future__ import annotations

import pytest

from cxchange.matching_engine.engine import MatchingEngine
from cxchange.matching_engine.exceptions import NoLiquidityError, SelfTradeError
from cxchange.matching_engine.structures import MatchingResponse
from cxchange.matching_engine.types import OrderStatus, Side


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


class TestMatchTakerOrderBasic:
    """Basic taker matching scenarios."""

    def test_full_fill_single_maker(self, engine: MatchingEngine) -> None:
        """Verify full fill against a single maker order."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 5)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert len(response.fills) == 1
        assert response.fills[0].size == 5
        assert response.order_deltas[0].status == OrderStatus.FILLED

    def test_partial_fill_single_maker(self, engine: MatchingEngine) -> None:
        """Verify partial fill against a single maker order."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 5)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        maker_delta = [
            delta for delta in response.order_deltas if delta.client_id == "m1"
        ][0]
        assert maker_delta.status == OrderStatus.PARTIALLY_FILLED
        assert maker_delta.remaining_size == 3
        assert response.order_deltas[0].status == OrderStatus.FILLED

    def test_taker_larger_than_maker(self, engine: MatchingEngine) -> None:
        """Verify taker larger than maker results in partial taker fill."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert response.order_deltas[0].status == OrderStatus.PARTIALLY_FILLED
        maker_delta = [
            delta for delta in response.order_deltas if delta.client_id == "m1"
        ][0]
        assert maker_delta.status == OrderStatus.FILLED

    def test_response_has_fills(self, engine: MatchingEngine) -> None:
        """Verify taker response includes fills."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1)
        assert response.fills

    def test_fill_prices_correct(self, engine: MatchingEngine) -> None:
        """Verify fill prices match maker prices."""
        add_maker(engine, "m1", "a1", Side.SELL, 101, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 101, 1)
        assert response.fills[0].price == 101

    def test_fill_sizes_correct(self, engine: MatchingEngine) -> None:
        """Verify fill sizes match matched amounts."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 3)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        assert response.fills[0].size == 2

    def test_taker_delta_first(self, engine: MatchingEngine) -> None:
        """Verify taker order delta is first."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1)
        assert response.order_deltas[0].client_id == "t1"

    def test_maker_deltas_present(self, engine: MatchingEngine) -> None:
        """Verify maker order deltas are present."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1)
        maker_ids = [delta.client_id for delta in response.order_deltas]
        assert "m1" in maker_ids

    def test_level_deltas_present(self, engine: MatchingEngine) -> None:
        """Verify level deltas are included."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1)
        assert response.level_deltas


class TestMatchTakerOrderMultipleOrders:
    """Taker matching across multiple orders/levels."""

    def test_fifo_within_level(self, engine: MatchingEngine) -> None:
        """Verify FIFO matching within a single level."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        add_maker(engine, "m2", "a2", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        maker_ids = [fill.maker_client_id for fill in response.fills]
        assert maker_ids == ["m1", "m2"]

    def test_partial_last_order(self, engine: MatchingEngine) -> None:
        """Verify partial fill on the last order in a level."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "m2", "a2", Side.SELL, 100, 3)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 4)
        maker_delta = [
            delta for delta in response.order_deltas if delta.client_id == "m2"
        ][0]
        assert maker_delta.status == OrderStatus.PARTIALLY_FILLED
        assert maker_delta.remaining_size == 1

    def test_exhaust_level(self, engine: MatchingEngine) -> None:
        """Verify full consumption removes a price level."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "m2", "a2", Side.SELL, 100, 3)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert engine.get_level(Side.SELL, 100) is None
        assert response.level_deltas[-1].size == 0

    def test_cross_multiple_levels(self, engine: MatchingEngine) -> None:
        """Verify taker can cross multiple price levels."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "m2", "a2", Side.SELL, 105, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 105, 3)
        prices = [fill.price for fill in response.fills]
        sizes = [fill.size for fill in response.fills]
        assert prices == [100, 105]
        assert sizes == [2, 1]

    def test_price_time_priority(self, engine: MatchingEngine) -> None:
        """Verify matching respects price-time priority across levels."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        add_maker(engine, "m2", "a2", Side.SELL, 100, 1)
        add_maker(engine, "m3", "a3", Side.SELL, 101, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 101, 3)
        maker_ids = [fill.maker_client_id for fill in response.fills]
        assert maker_ids == ["m1", "m2", "m3"]

    def test_stop_at_price_limit(self, engine: MatchingEngine) -> None:
        """Verify matching stops at the price limit."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "m2", "a2", Side.SELL, 105, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 102, 5)
        assert len(response.fills) == 1
        remaining = engine.get_order_by_client_id("m2")
        assert remaining is not None
        assert remaining.size == 2


class TestMatchTakerOrderPartialFills:
    """Partial fill scenarios for taker orders."""

    def test_partial_fill_is_success(self, engine: MatchingEngine) -> None:
        """Verify partial fill returns a successful response."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert response.order_deltas[0].status == OrderStatus.PARTIALLY_FILLED

    def test_remainder_discarded(self, engine: MatchingEngine) -> None:
        """Verify taker remainder is not stored in the book."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert response.order_deltas[0].status == OrderStatus.PARTIALLY_FILLED
        assert not engine.has_order("t1")

    def test_taker_remaining_size_zero(self, engine: MatchingEngine) -> None:
        """Verify taker delta remaining_size is zero on partial fill."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 5)
        assert response.order_deltas[0].remaining_size == 0

    def test_filled_size_accurate(self, engine: MatchingEngine) -> None:
        """Verify taker filled_size matches sum of fills."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        add_maker(engine, "m2", "a2", Side.SELL, 101, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 101, 10)
        filled_sum = sum(fill.size for fill in response.fills)
        assert response.order_deltas[0].filled_size == filled_sum


class TestMatchTakerOrderSelfTrade:
    """Self-trade prevention scenarios."""

    def test_self_trade_at_best_raises(self, engine: MatchingEngine) -> None:
        """Verify self-trade at best price raises."""
        add_maker(engine, "m1", "acct", Side.SELL, 100, 1)
        with pytest.raises(SelfTradeError):
            match_taker(engine, "t1", "acct", Side.BUY, 100, 1)

    def test_self_trade_not_at_best_raises(self, engine: MatchingEngine) -> None:
        """Verify self-trade at worse price still raises."""
        add_maker(engine, "m1", "other", Side.SELL, 100, 1)
        add_maker(engine, "m2", "acct", Side.SELL, 105, 1)
        with pytest.raises(SelfTradeError):
            match_taker(engine, "t1", "acct", Side.BUY, 110, 2)

    def test_self_trade_returns_conflicting_id(self, engine: MatchingEngine) -> None:
        """Verify SelfTradeError includes conflicting order ID."""
        add_maker(engine, "m1", "acct", Side.SELL, 100, 1)
        with pytest.raises(SelfTradeError) as excinfo:
            match_taker(engine, "t1", "acct", Side.BUY, 100, 1)
        assert excinfo.value.conflicting_order_id == "m1"

    def test_no_self_trade_different_accounts(self, engine: MatchingEngine) -> None:
        """Verify no self-trade when accounts differ."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "a2", Side.BUY, 100, 1)
        assert response.fills

    def test_self_trade_check_respects_price_limit(
        self, engine: MatchingEngine
    ) -> None:
        """Verify self-trade check respects price limit."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        add_maker(engine, "m2", "acct", Side.SELL, 110, 1)
        response = match_taker(engine, "t1", "acct", Side.BUY, 105, 1)
        assert response.fills

    def test_self_trade_behind_sufficient_liquidity_fills(
        self, engine: MatchingEngine
    ) -> None:
        """Verify same-account liquidity behind enough other liquidity is not a conflict."""
        add_maker(engine, "m1", "other", Side.SELL, 100, 5)
        add_maker(engine, "m2", "acct", Side.SELL, 105, 1)
        response = match_taker(engine, "t1", "acct", Side.BUY, 110, 2)
        assert response.fills
        assert len(response.fills) == 1
        assert response.fills[0].maker_client_id == "m1"
        assert response.order_deltas[0].filled_size == 2

    def test_self_trade_at_same_price_behind_other_account_fills(
        self, engine: MatchingEngine
    ) -> None:
        """Verify same-account maker at same price behind other account fills first."""
        add_maker(engine, "m1", "other", Side.SELL, 100, 1)
        add_maker(engine, "m2", "acct", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "acct", Side.BUY, 100, 1)
        assert response.fills
        assert response.fills[0].maker_client_id == "m1"
        assert response.order_deltas[0].status == OrderStatus.FILLED

    def test_self_trade_reached_after_partial_fill_raises(
        self, engine: MatchingEngine
    ) -> None:
        """Verify self-trade is raised when reached after partial fill."""
        add_maker(engine, "m1", "other", Side.SELL, 100, 1)
        add_maker(engine, "m2", "acct", Side.SELL, 100, 1)
        with pytest.raises(SelfTradeError) as excinfo:
            match_taker(engine, "t1", "acct", Side.BUY, 100, 2)
        assert excinfo.value.conflicting_order_id == "m2"


class TestMatchTakerOrderNoLiquidity:
    """No-liquidity scenarios for taker orders."""

    def test_empty_book_raises(self, engine: MatchingEngine) -> None:
        """Verify empty book raises NoLiquidityError."""
        with pytest.raises(NoLiquidityError):
            match_taker(engine, "t1", "a1", Side.BUY, 100, 1)

    def test_empty_opposite_side_raises(self, engine: MatchingEngine) -> None:
        """Verify missing opposite side raises NoLiquidityError."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        with pytest.raises(NoLiquidityError):
            match_taker(engine, "t1", "a2", Side.BUY, 100, 1)

    def test_no_liquidity_at_price_raises(self, engine: MatchingEngine) -> None:
        """Verify no liquidity within limit raises."""
        add_maker(engine, "s1", "a1", Side.SELL, 110, 1)
        with pytest.raises(NoLiquidityError):
            match_taker(engine, "t1", "a2", Side.BUY, 100, 1)

    def test_all_liquidity_beyond_price_raises(self, engine: MatchingEngine) -> None:
        """Verify liquidity beyond limit raises NoLiquidityError."""
        add_maker(engine, "b1", "a1", Side.BUY, 90, 1)
        with pytest.raises(NoLiquidityError):
            match_taker(engine, "t1", "a2", Side.SELL, 100, 1)


class TestMatchTakerOrderMakerState:
    """Maker state updates after taker matching."""

    def test_fully_filled_maker_removed(self, engine: MatchingEngine) -> None:
        """Verify fully filled maker is removed."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        assert engine.get_order_by_client_id("m1") is None

    def test_partial_filled_maker_remains(self, engine: MatchingEngine) -> None:
        """Verify partially filled maker remains in book."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 5)
        match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        assert engine.get_order_by_client_id("m1") is not None

    def test_maker_size_decremented(self, engine: MatchingEngine) -> None:
        """Verify maker size is reduced after partial fill."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 5)
        match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        order = engine.get_order_by_client_id("m1")
        assert order is not None
        assert order.size == 3

    def test_level_size_decremented(self, engine: MatchingEngine) -> None:
        """Verify level size is reduced after partial fill."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 5)
        match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        level = engine.get_level(Side.SELL, 100)
        assert level == (100, 3, 1)

    def test_empty_level_removed(self, engine: MatchingEngine) -> None:
        """Verify empty level is removed after full fill."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 2)
        match_taker(engine, "t1", "ta", Side.BUY, 100, 2)
        assert engine.get_level(Side.SELL, 100) is None


class TestMatchTakerOrderEdgeCases:
    """Edge cases for taker matching."""

    def test_exact_fill_at_price_limit(self, engine: MatchingEngine) -> None:
        """Verify fills at exact price limit."""
        add_maker(engine, "m1", "a1", Side.SELL, 105, 2)
        response = match_taker(engine, "t1", "ta", Side.BUY, 105, 2)
        assert response.order_deltas[0].status == OrderStatus.FILLED

    def test_price_limit_exclusive(self, engine: MatchingEngine) -> None:
        """Verify limit below best ask yields no liquidity."""
        add_maker(engine, "m1", "a1", Side.SELL, 105, 1)
        with pytest.raises(NoLiquidityError):
            match_taker(engine, "t1", "ta", Side.BUY, 104, 1)

    def test_single_lot_orders(self, engine: MatchingEngine) -> None:
        """Verify single-lot orders match correctly."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1)
        assert response.fills[0].size == 1

    def test_very_large_order(self, engine: MatchingEngine) -> None:
        """Verify very large taker order partially fills available liquidity."""
        add_maker(engine, "m1", "a1", Side.SELL, 100, 10)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 1_000_000)
        assert response.order_deltas[0].status == OrderStatus.PARTIALLY_FILLED
        assert response.order_deltas[0].filled_size == 10

    def test_many_small_makers(self, engine: MatchingEngine) -> None:
        """Verify matching across many small maker orders."""
        for i in range(10):
            add_maker(engine, f"m{i}", f"a{i}", Side.SELL, 100, 1)
        response = match_taker(engine, "t1", "ta", Side.BUY, 100, 10)
        assert len(response.fills) == 10
