"""Tests for matching engine type aliases and enums."""

from __future__ import annotations

from cxchange.matching_engine.types import OrderStatus, Side


class TestSide:
    """Tests for Side enum values and behavior."""

    def test_buy_value(self) -> None:
        """Verify BUY enum value is 1."""
        assert Side.BUY == 1

    def test_sell_value(self) -> None:
        """Verify SELL enum value is -1."""
        assert Side.SELL == -1

    def test_arithmetic(self) -> None:
        """Verify Side supports simple arithmetic operations."""
        assert Side.BUY * 100 == 100


class TestOrderStatus:
    """Tests for OrderStatus enum values."""

    def test_all_values_unique(self) -> None:
        """Verify all order status values are unique."""
        values = {status.value for status in OrderStatus}
        assert len(values) == len(OrderStatus)

    def test_ordering(self) -> None:
        """Verify status ordering is monotonic by lifecycle stage."""
        assert OrderStatus.OPEN < OrderStatus.PARTIALLY_FILLED < OrderStatus.FILLED
