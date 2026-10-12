"""Tests for matching engine snapshot export and import."""

from __future__ import annotations

from cxchange.engine.engine import MatchingEngine
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


class TestExportSnapshot:
    """Tests for export_snapshot behavior."""

    def test_empty_book(self, engine: MatchingEngine) -> None:
        """Verify empty book exports with no levels."""
        snapshot = engine.export_snapshot()
        assert snapshot.bids == []
        assert snapshot.asks == []
        assert snapshot.next_order_id == 1
        assert snapshot.next_timestamp == 0

    def test_with_bids(self, engine: MatchingEngine) -> None:
        """Verify bids are included in snapshot."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        assert len(snapshot.bids) == 1
        assert snapshot.asks == []

    def test_with_asks(self, engine: MatchingEngine) -> None:
        """Verify asks are included in snapshot."""
        add_maker(engine, "s1", "a1", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        assert len(snapshot.asks) == 1
        assert snapshot.bids == []

    def test_with_both_sides(self, engine: MatchingEngine) -> None:
        """Verify snapshot includes both bid and ask sides."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        assert len(snapshot.bids) == 1
        assert len(snapshot.asks) == 1

    def test_order_preserved_in_levels(self, engine: MatchingEngine) -> None:
        """Verify FIFO order is preserved in snapshot levels."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        orders = snapshot.bids[0].orders
        assert [order.client_id for order in orders] == ["b1", "b2"]

    def test_next_order_id_captured(self, engine: MatchingEngine) -> None:
        """Verify next_order_id is captured in snapshot."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 101, 1)
        snapshot = engine.export_snapshot()
        assert snapshot.next_order_id == 3

    def test_next_timestamp_captured(self, engine: MatchingEngine) -> None:
        """Verify next_timestamp is captured in snapshot."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 101, 1)
        snapshot = engine.export_snapshot()
        assert snapshot.next_timestamp == 2


class TestFromSnapshot:
    """Tests for from_snapshot behavior."""

    def test_restore_empty(self, engine: MatchingEngine) -> None:
        """Verify restoring empty snapshot yields empty book."""
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        assert restored.get_best_bid() is None
        assert restored.get_best_ask() is None

    def test_restore_bids(self, engine: MatchingEngine) -> None:
        """Verify bid levels are restored."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        order = restored.get_order_by_client_id("b1")
        assert order is not None
        assert order.price == 100

    def test_restore_asks(self, engine: MatchingEngine) -> None:
        """Verify ask levels are restored."""
        add_maker(engine, "s1", "a1", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        order = restored.get_order_by_client_id("s1")
        assert order is not None
        assert order.price == 110

    def test_restore_both_sides(self, engine: MatchingEngine) -> None:
        """Verify both sides are restored."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        assert restored.get_best_bid() == 100
        assert restored.get_best_ask() == 110

    def test_order_ids_preserved(self, engine: MatchingEngine) -> None:
        """Verify internal order IDs are preserved after restore."""
        response = engine.add_maker_order("b1", "a1", Side.BUY, 100, 1)
        order_id = response.order_deltas[0].order_id
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        order = restored.get_order_by_client_id("b1")
        assert order is not None
        assert order.order_id == order_id

    def test_client_ids_preserved(self, engine: MatchingEngine) -> None:
        """Verify client IDs are preserved after restore."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        assert restored.has_order("b1")

    def test_next_counters_restored(self, engine: MatchingEngine) -> None:
        """Verify counters are restored for subsequent orders."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        response = restored.add_maker_order("b2", "a2", Side.BUY, 101, 1)
        assert response.order_deltas[0].order_id == snapshot.next_order_id


class TestSnapshotRoundTrip:
    """Tests for snapshot round-trip consistency."""

    def test_empty_roundtrip(self, engine: MatchingEngine) -> None:
        """Verify empty snapshot round-trip is identical."""
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        roundtrip = restored.export_snapshot()
        assert snapshot == roundtrip

    def test_single_order_roundtrip(self, engine: MatchingEngine) -> None:
        """Verify single-order round-trip preserves state."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        roundtrip = restored.export_snapshot()
        assert snapshot == roundtrip

    def test_complex_book_roundtrip(self, engine: MatchingEngine) -> None:
        """Verify complex book round-trip preserves state."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "b2", "a2", Side.BUY, 101, 2)
        add_maker(engine, "s1", "a3", Side.SELL, 110, 1)
        add_maker(engine, "s2", "a4", Side.SELL, 111, 2)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        roundtrip = restored.export_snapshot()
        assert snapshot == roundtrip

    def test_after_operations_roundtrip(self, engine: MatchingEngine) -> None:
        """Verify round-trip after operations preserves state."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 2)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 2)
        engine.cancel_maker_order("b1")
        engine.add_maker_order("b2", "a3", Side.BUY, 101, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        roundtrip = restored.export_snapshot()
        assert snapshot == roundtrip

    def test_state_identical_after_roundtrip(self, engine: MatchingEngine) -> None:
        """Verify key state is identical after round-trip."""
        add_maker(engine, "b1", "a1", Side.BUY, 100, 1)
        add_maker(engine, "s1", "a2", Side.SELL, 110, 1)
        snapshot = engine.export_snapshot()
        restored = MatchingEngine.from_snapshot(snapshot, engine.spec)
        assert (restored.get_best_bid(), restored.get_best_ask()) == (
            engine.get_best_bid(),
            engine.get_best_ask(),
        )
        assert restored.export_snapshot() == snapshot
