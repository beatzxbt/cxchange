"""Tests for core matching engine msgspec structures."""

from __future__ import annotations

import pytest

from cxchange.matching_engine.structures import (
    Fill,
    InstrumentSpec,
    LevelDelta,
    MatchingResponse,
    Order,
    OrderDelta,
)
from cxchange.matching_engine.types import OrderStatus, Side


class TestInstrumentSpec:
    """Tests for InstrumentSpec creation and immutability."""

    def test_creation(self) -> None:
        """Verify InstrumentSpec fields are stored correctly."""
        spec = InstrumentSpec(
            base="BTC",
            quote="USD",
            symbol="BTC-USD",
            tick_size=0.5,
            lot_size=0.01,
            min_size=1,
            max_size=0,
        )
        assert spec.base == "BTC"
        assert spec.quote == "USD"
        assert spec.symbol == "BTC-USD"
        assert spec.tick_size == 0.5
        assert spec.lot_size == 0.01
        assert spec.min_size == 1
        assert spec.max_size == 0

    def test_immutable(self) -> None:
        """Verify InstrumentSpec is immutable."""
        spec = InstrumentSpec(
            base="ETH",
            quote="USD",
            symbol="ETH-USD",
            tick_size=1.0,
            lot_size=0.1,
        )
        with pytest.raises(AttributeError):
            setattr(spec, "symbol", "ETH-USD-PERP")

    def test_symbol_format(self) -> None:
        """Verify symbol matches base and quote conventions."""
        spec = InstrumentSpec(
            base="SOL",
            quote="USD",
            symbol="SOL-USD",
            tick_size=0.01,
            lot_size=1.0,
        )
        assert spec.symbol == f"{spec.base}-{spec.quote}"


class TestOrder:
    """Tests for Order mutability and field storage."""

    def test_creation_all_fields(self) -> None:
        """Verify Order stores all fields correctly."""
        order = Order(
            order_id=1,
            client_id="c1",
            account_id="a1",
            price=100,
            size=5,
            side=Side.BUY,
            timestamp=10,
        )
        assert order.order_id == 1
        assert order.client_id == "c1"
        assert order.account_id == "a1"
        assert order.price == 100
        assert order.size == 5
        assert order.side == Side.BUY
        assert order.timestamp == 10

    def test_mutable_size(self) -> None:
        """Verify Order size can be modified."""
        order = Order(
            order_id=2,
            client_id="c2",
            account_id="a2",
            price=101,
            size=7,
            side=Side.SELL,
            timestamp=11,
        )
        order.size = 3
        assert order.size == 3

    def test_other_fields_immutable(self) -> None:
        """Verify non-size fields are immutable after creation."""
        order = Order(
            order_id=3,
            client_id="c3",
            account_id="a3",
            price=99,
            size=9,
            side=Side.BUY,
            timestamp=12,
        )
        with pytest.raises(AttributeError):
            order.order_id = 10


class TestFill:
    """Tests for Fill creation and immutability."""

    def test_creation(self) -> None:
        """Verify Fill fields are stored correctly."""
        fill = Fill(
            taker_order_id=10,
            maker_order_id=20,
            taker_client_id="t1",
            maker_client_id="m1",
            taker_account_id="ta",
            maker_account_id="ma",
            price=100,
            size=2,
            taker_side=Side.BUY,
        )
        assert fill.taker_order_id == 10
        assert fill.maker_order_id == 20
        assert fill.price == 100
        assert fill.size == 2

    def test_immutable(self) -> None:
        """Verify Fill is immutable."""
        fill = Fill(
            taker_order_id=11,
            maker_order_id=21,
            taker_client_id="t2",
            maker_client_id="m2",
            taker_account_id="ta2",
            maker_account_id="ma2",
            price=101,
            size=1,
            taker_side=Side.SELL,
        )
        with pytest.raises(AttributeError):
            setattr(fill, "size", 3)

    def test_all_fields_accessible(self) -> None:
        """Verify Fill exposes all fields for access."""
        fill = Fill(
            taker_order_id=12,
            maker_order_id=22,
            taker_client_id="t3",
            maker_client_id="m3",
            taker_account_id="ta3",
            maker_account_id="ma3",
            price=102,
            size=4,
            taker_side=Side.BUY,
        )
        assert fill.taker_client_id == "t3"
        assert fill.maker_client_id == "m3"
        assert fill.taker_account_id == "ta3"
        assert fill.maker_account_id == "ma3"


class TestMatchingResponse:
    """Tests for MatchingResponse container behavior."""

    def test_empty_response(self) -> None:
        """Verify MatchingResponse accepts empty lists."""
        response = MatchingResponse(fills=[], order_deltas=[], level_deltas=[])
        assert response.fills == []
        assert response.order_deltas == []
        assert response.level_deltas == []

    def test_with_fills(self) -> None:
        """Verify MatchingResponse stores fills."""
        fill = Fill(
            taker_order_id=13,
            maker_order_id=23,
            taker_client_id="t4",
            maker_client_id="m4",
            taker_account_id="ta4",
            maker_account_id="ma4",
            price=103,
            size=5,
            taker_side=Side.SELL,
        )
        response = MatchingResponse(fills=[fill], order_deltas=[], level_deltas=[])
        assert response.fills == [fill]

    def test_with_deltas(self) -> None:
        """Verify MatchingResponse stores order and level deltas."""
        order_delta = OrderDelta(
            order_id=1,
            client_id="c1",
            status=OrderStatus.OPEN,
            remaining_size=5,
        )
        level_delta = LevelDelta(price=100, size=5, side=Side.BUY, order_count=1)
        response = MatchingResponse(
            fills=[],
            order_deltas=[order_delta],
            level_deltas=[level_delta],
        )
        assert response.order_deltas == [order_delta]
        assert response.level_deltas == [level_delta]
