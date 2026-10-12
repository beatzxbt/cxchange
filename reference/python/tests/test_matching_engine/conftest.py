"""Pytest fixtures for matching engine tests.

Provides shared setup for instrument specifications and engine instances.
"""

from __future__ import annotations

import pytest

from cxchange.engine.engine import MatchingEngine
from cxchange.engine.structures import InstrumentSpec


@pytest.fixture
def spec() -> InstrumentSpec:
    """Create a basic instrument spec for tests.

    Returns:
        InstrumentSpec: Test instrument specification.
    """
    return InstrumentSpec(
        base="BTC",
        quote="USD",
        symbol="BTC-USD",
        tick_size=1.0,
        lot_size=1.0,
    )


@pytest.fixture
def engine(spec: InstrumentSpec) -> MatchingEngine:
    """Create a matching engine with the test spec.

    Args:
        spec (InstrumentSpec): Instrument specification fixture.

    Returns:
        MatchingEngine: New engine instance.
    """
    return MatchingEngine(spec)
