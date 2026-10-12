"""Type aliases and enums for the matching engine.

Defines integer-based types for prices and sizes to avoid floating point
precision issues. All arithmetic in the matching engine uses these types.
"""

from __future__ import annotations

from enum import IntEnum
from typing import TypeAlias

Tick: TypeAlias = int
Lot: TypeAlias = int
OrderId: TypeAlias = int
ClientId: TypeAlias = str
AccountId: TypeAlias = str


class Side(IntEnum):
    """Order side enumeration.

    Uses 1/-1 for easy sign arithmetic in P&L calculations.
    """

    BUY = 1
    SELL = -1


class OrderStatus(IntEnum):
    """Order lifecycle status enumeration."""

    OPEN = 1
    PARTIALLY_FILLED = 2
    FILLED = 3
    CANCELLED = 4
    REJECTED = 5
    REPLACED = 6
