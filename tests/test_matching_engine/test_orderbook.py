"""Tests for OrderBook behavior and price-time ordering."""

from __future__ import annotations

from cxchange.matching_engine.orderbook import OrderBook
from cxchange.matching_engine.structures import Order
from cxchange.matching_engine.types import Side


def make_order(
    order_id: int,
    side: Side,
    price: int,
    size: int = 1,
    client_id: str | None = None,
    account_id: str = "acct",
) -> Order:
    """Create an Order for testing.

    Args:
        order_id (int): Internal order ID.
        side (Side): Order side.
        price (int): Price in ticks.
        size (int): Size in lots.
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


class TestOrderBookCreation:
    """Tests for empty OrderBook initialization."""

    def test_empty_book(self) -> None:
        """Verify a new book has no levels."""
        book = OrderBook()
        assert book.get_best_bid() is None
        assert book.get_best_ask() is None

    def test_no_best_bid(self) -> None:
        """Verify empty book has no best bid."""
        book = OrderBook()
        assert book.get_best_bid() is None

    def test_no_best_ask(self) -> None:
        """Verify empty book has no best ask."""
        book = OrderBook()
        assert book.get_best_ask() is None


class TestOrderBookBestPrices:
    """Tests for best bid/ask price queries."""

    def test_single_bid(self) -> None:
        """Verify best bid from single bid level."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert book.get_best_bid() == 100

    def test_single_ask(self) -> None:
        """Verify best ask from single ask level."""
        book = OrderBook()
        book.add_order(make_order(2, Side.SELL, 105))
        assert book.get_best_ask() == 105

    def test_multiple_bids_returns_highest(self) -> None:
        """Verify best bid returns highest price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.add_order(make_order(2, Side.BUY, 105))
        assert book.get_best_bid() == 105

    def test_multiple_asks_returns_lowest(self) -> None:
        """Verify best ask returns lowest price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 110))
        book.add_order(make_order(2, Side.SELL, 108))
        assert book.get_best_ask() == 108

    def test_best_updates_on_remove(self) -> None:
        """Verify best price updates after removal."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.add_order(make_order(2, Side.BUY, 105))
        book.remove_order(2)
        assert book.get_best_bid() == 100


class TestOrderBookAddOrder:
    """Tests for adding orders to the OrderBook."""

    def test_add_buy_creates_level(self) -> None:
        """Verify adding a buy order creates a bid level."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert book.get_level(Side.BUY, 100) is not None

    def test_add_sell_creates_level(self) -> None:
        """Verify adding a sell order creates an ask level."""
        book = OrderBook()
        book.add_order(make_order(2, Side.SELL, 105))
        assert book.get_level(Side.SELL, 105) is not None

    def test_add_to_existing_level(self) -> None:
        """Verify adding to an existing level increments count."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.add_order(make_order(2, Side.BUY, 100))
        level = book.get_level(Side.BUY, 100)
        assert level is not None
        assert level.order_count == 2

    def test_level_delta_returned(self) -> None:
        """Verify add_order returns a LevelDelta."""
        book = OrderBook()
        delta = book.add_order(make_order(1, Side.BUY, 100, size=3))
        assert delta.price == 100
        assert delta.size == 3
        assert delta.side == Side.BUY
        assert delta.order_count == 1

    def test_order_to_level_updated(self) -> None:
        """Verify order_to_level mapping is updated on add."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert book.order_to_level[1] == (Side.BUY, 100)


class TestOrderBookRemoveOrder:
    """Tests for removing orders from the OrderBook."""

    def test_remove_existing(self) -> None:
        """Verify removing an existing order returns it."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        order, delta = book.remove_order(1)
        assert order is not None
        assert delta is not None
        assert book.get_order(1) is None

    def test_remove_nonexistent(self) -> None:
        """Verify removing a missing order returns None tuple."""
        book = OrderBook()
        order, delta = book.remove_order(99)
        assert order is None
        assert delta is None

    def test_level_removed_when_empty(self) -> None:
        """Verify empty levels are removed on last order removal."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.remove_order(1)
        assert book.get_level(Side.BUY, 100) is None

    def test_level_kept_when_others_remain(self) -> None:
        """Verify levels remain when other orders exist."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.add_order(make_order(2, Side.BUY, 100))
        book.remove_order(1)
        level = book.get_level(Side.BUY, 100)
        assert level is not None
        assert level.order_count == 1


class TestOrderBookGetOrder:
    """Tests for order lookups in the OrderBook."""

    def test_get_existing(self) -> None:
        """Verify retrieving an existing order returns it."""
        book = OrderBook()
        order = make_order(1, Side.BUY, 100)
        book.add_order(order)
        assert book.get_order(1) is order

    def test_get_nonexistent(self) -> None:
        """Verify retrieving a missing order returns None."""
        book = OrderBook()
        assert book.get_order(99) is None

    def test_get_after_remove(self) -> None:
        """Verify order lookup after removal returns None."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.remove_order(1)
        assert book.get_order(1) is None


class TestOrderBookIteration:
    """Tests for order book level iteration ordering."""

    def test_iter_bids_descending(self) -> None:
        """Verify bids iterate from highest to lowest price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        book.add_order(make_order(2, Side.BUY, 105))
        book.add_order(make_order(3, Side.BUY, 102))
        prices = [level.price for level in book.iter_levels(Side.BUY)]
        assert prices == [105, 102, 100]

    def test_iter_asks_ascending(self) -> None:
        """Verify asks iterate from lowest to highest price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 110))
        book.add_order(make_order(2, Side.SELL, 105))
        book.add_order(make_order(3, Side.SELL, 108))
        prices = [level.price for level in book.iter_levels(Side.SELL)]
        assert prices == [105, 108, 110]

    def test_iter_empty_side(self) -> None:
        """Verify iteration over empty side yields no levels."""
        book = OrderBook()
        prices = [level.price for level in book.iter_levels(Side.BUY)]
        assert prices == []

    def test_iter_up_to_price(self) -> None:
        """Verify iteration respects price limits."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100))
        book.add_order(make_order(2, Side.SELL, 105))
        book.add_order(make_order(3, Side.SELL, 110))
        ask_prices = [level.price for level in book.iter_levels_up_to(Side.SELL, 105)]
        assert ask_prices == [100, 105]

        book.add_order(make_order(4, Side.BUY, 100))
        book.add_order(make_order(5, Side.BUY, 102))
        book.add_order(make_order(6, Side.BUY, 105))
        bid_prices = [level.price for level in book.iter_levels_up_to(Side.BUY, 102)]
        assert bid_prices == [105, 102]


class TestOrderBookWouldCross:
    """Tests for spread crossing detection."""

    def test_buy_no_asks_no_cross(self) -> None:
        """Verify buy order does not cross with empty asks."""
        book = OrderBook()
        assert not book.would_cross(Side.BUY, 100)

    def test_buy_below_best_ask_no_cross(self) -> None:
        """Verify buy below best ask does not cross."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 105))
        assert not book.would_cross(Side.BUY, 104)

    def test_buy_at_best_ask_crosses(self) -> None:
        """Verify buy at best ask crosses."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 105))
        assert book.would_cross(Side.BUY, 105)

    def test_buy_above_best_ask_crosses(self) -> None:
        """Verify buy above best ask crosses."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 105))
        assert book.would_cross(Side.BUY, 106)

    def test_sell_no_bids_no_cross(self) -> None:
        """Verify sell order does not cross with empty bids."""
        book = OrderBook()
        assert not book.would_cross(Side.SELL, 100)

    def test_sell_above_best_bid_no_cross(self) -> None:
        """Verify sell above best bid does not cross."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert not book.would_cross(Side.SELL, 101)

    def test_sell_at_best_bid_crosses(self) -> None:
        """Verify sell at best bid crosses."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert book.would_cross(Side.SELL, 100)

    def test_sell_below_best_bid_crosses(self) -> None:
        """Verify sell below best bid crosses."""
        book = OrderBook()
        book.add_order(make_order(1, Side.BUY, 100))
        assert book.would_cross(Side.SELL, 99)


class TestOrderBookSelfTrade:
    """Tests for self-trade detection in the OrderBook."""

    def test_no_self_trade_empty_book(self) -> None:
        """Verify no self-trade detected in empty book."""
        book = OrderBook()
        assert book.find_self_trade(Side.BUY, 100, 1, "acct") is None

    def test_no_self_trade_different_account(self) -> None:
        """Verify no self-trade with different accounts."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100, account_id="other"))
        assert book.find_self_trade(Side.BUY, 100, 1, "acct") is None

    def test_self_trade_detected_at_best(self) -> None:
        """Verify self-trade detected at best price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100, account_id="acct"))
        conflict = book.find_self_trade(Side.BUY, 100, 1, "acct")
        assert conflict == "c1"

    def test_self_trade_detected_at_worse_price(self) -> None:
        """Verify self-trade detected beyond best price."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100, account_id="other"))
        book.add_order(make_order(2, Side.SELL, 103, account_id="acct"))
        conflict = book.find_self_trade(Side.BUY, 105, 2, "acct")
        assert conflict == "c2"

    def test_self_trade_respects_price_limit(self) -> None:
        """Verify self-trade check respects the price limit."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 110, account_id="acct"))
        conflict = book.find_self_trade(Side.BUY, 105, 1, "acct")
        assert conflict is None

    def test_self_trade_returns_first_conflict(self) -> None:
        """Verify self-trade returns first conflicting order."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100, account_id="acct"))
        book.add_order(make_order(2, Side.SELL, 105, account_id="acct"))
        conflict = book.find_self_trade(Side.BUY, 110, 10, "acct")
        assert conflict == "c1"

    def test_self_trade_behind_sufficient_liquidity_not_reached(self) -> None:
        """Verify same-account maker is not a conflict if not reached."""
        book = OrderBook()
        book.add_order(make_order(1, Side.SELL, 100, account_id="other"))
        book.add_order(make_order(2, Side.SELL, 105, account_id="acct"))
        conflict = book.find_self_trade(Side.BUY, 110, 1, "acct")
        assert conflict is None
