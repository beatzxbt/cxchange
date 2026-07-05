"""Typed exceptions for matching engine error conditions.

Exception hierarchy:
- MatchingEngineError (base)
  |-- OrderCrossesSpreadError
  |-- SelfTradeError
  |-- OrderNotFoundError
  |-- OrderAlreadyExistsError
  |-- NoLiquidityError
  |-- InvalidOperationError
"""

from __future__ import annotations

from .types import AccountId, ClientId, Side, Tick


class MatchingEngineError(Exception):
    """Base exception for all matching engine errors."""


class OrderCrossesSpreadError(MatchingEngineError):
    """Raised when a maker order would cross the spread.

    Attributes:
        client_id (ClientId): The order's client ID.
        side (Side): The order side.
        price (Tick): The order price.
        opposite_best (Tick): The best price on the opposite side.
    """

    def __init__(
        self,
        client_id: ClientId,
        side: Side,
        price: Tick,
        opposite_best: Tick,
    ) -> None:
        """Initialize the exception.

        Args:
            client_id (ClientId): The order's client ID.
            side (Side): The order side.
            price (Tick): The order price.
            opposite_best (Tick): The best price on the opposite side.
        """
        self.client_id = client_id
        self.side = side
        self.price = price
        self.opposite_best = opposite_best
        super().__init__(
            f"Order {client_id} at {price} would cross spread "
            f"(opposite best: {opposite_best})"
        )


class SelfTradeError(MatchingEngineError):
    """Raised when an order would trade against the same account.

    Attributes:
        client_id (ClientId): The incoming order's client ID.
        account_id (AccountId): The account that would self-trade.
        conflicting_order_id (ClientId): The resting order's client ID.
    """

    def __init__(
        self,
        client_id: ClientId,
        account_id: AccountId,
        conflicting_order_id: ClientId,
    ) -> None:
        """Initialize the exception.

        Args:
            client_id (ClientId): The incoming order's client ID.
            account_id (AccountId): The account that would self-trade.
            conflicting_order_id (ClientId): The resting order's client ID.
        """
        self.client_id = client_id
        self.account_id = account_id
        self.conflicting_order_id = conflicting_order_id
        super().__init__(
            f"Order {client_id} would self-trade with {conflicting_order_id} "
            f"(account: {account_id})"
        )


class OrderNotFoundError(MatchingEngineError):
    """Raised when operating on a non-existent order.

    Attributes:
        client_id (ClientId): The order's client ID that was not found.
    """

    def __init__(self, client_id: ClientId) -> None:
        """Initialize the exception.

        Args:
            client_id (ClientId): The order's client ID that was not found.
        """
        self.client_id = client_id
        super().__init__(f"Order {client_id} not found")


class OrderAlreadyExistsError(MatchingEngineError):
    """Raised when adding an order with a duplicate client ID.

    Attributes:
        client_id (ClientId): The duplicate client ID.
    """

    def __init__(self, client_id: ClientId) -> None:
        """Initialize the exception.

        Args:
            client_id (ClientId): The duplicate client ID.
        """
        self.client_id = client_id
        super().__init__(f"Order with client ID {client_id} already exists")


class NoLiquidityError(MatchingEngineError):
    """Raised when there is no liquidity for a taker order.

    Attributes:
        client_id (ClientId): The taker order's client ID.
        side (Side): The taker side.
        price (Tick): The limit price.
    """

    def __init__(self, client_id: ClientId, side: Side, price: Tick) -> None:
        """Initialize the exception.

        Args:
            client_id (ClientId): The taker order's client ID.
            side (Side): The taker side.
            price (Tick): The limit price.
        """
        self.client_id = client_id
        self.side = side
        self.price = price
        side_str = "buy" if side == Side.BUY else "sell"
        super().__init__(
            f"No liquidity for {side_str} order {client_id} at price {price}"
        )


class InvalidOperationError(MatchingEngineError):
    """Raised for invalid operations (e.g., amend to same values)."""
