"""Public API exports for the matching engine package."""

from __future__ import annotations

from .engine import MatchingEngine
from .exceptions import (
    InvalidOperationError,
    MatchingEngineError,
    NoLiquidityError,
    OrderAlreadyExistsError,
    OrderCrossesSpreadError,
    OrderNotFoundError,
    SelfTradeError,
)
from .structures import (
    Fill,
    InstrumentSpec,
    LevelDelta,
    LevelSnapshot,
    MatchingResponse,
    Order,
    OrderBookSnapshot,
    OrderDelta,
)
from .types import AccountId, ClientId, Lot, OrderId, OrderStatus, Side, Tick

__all__ = [
    "MatchingEngine",
    "InstrumentSpec",
    "Order",
    "Fill",
    "LevelDelta",
    "OrderDelta",
    "MatchingResponse",
    "LevelSnapshot",
    "OrderBookSnapshot",
    "Tick",
    "Lot",
    "OrderId",
    "ClientId",
    "AccountId",
    "Side",
    "OrderStatus",
    "MatchingEngineError",
    "OrderCrossesSpreadError",
    "SelfTradeError",
    "OrderNotFoundError",
    "OrderAlreadyExistsError",
    "NoLiquidityError",
    "InvalidOperationError",
]
