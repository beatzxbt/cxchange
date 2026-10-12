"""Compare native Rust replay with the preserved Python matching reference.

Run from the repository root with ``make differential``. Seeded command traces
compare every response and final resting state, normalizing deliberately changed
identity/counter rules. The production engine has no Python dependency.
"""

from __future__ import annotations

import argparse
import json
import random
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "reference/python/src"))

from cxchange.engine.engine import MatchingEngine
from cxchange.engine.exceptions import (
    MatchingEngineError,
    NoLiquidityError,
    OrderAlreadyExistsError,
    OrderCrossesSpreadError,
    OrderNotFoundError,
    SelfTradeError,
)
from cxchange.engine.structures import InstrumentSpec
from cxchange.engine.types import Side


class ReferenceTrace:
    """Generate and normalize a deterministic reference execution.

    Attributes:
        commands (list[dict[str, Any]]): Native command inputs.
        results (list[dict[str, Any]]): Expected normalized command results.
    """

    def __init__(self, seed: int) -> None:
        """Initialize a trace generator.

        Args:
            seed (int): Reproducible random generator seed.
        """
        self.rng = random.Random(seed)
        self.engine = MatchingEngine(InstrumentSpec("BTC", "USD", "BTC-USD", 1, 1))
        self.commands: list[dict[str, Any]] = []
        self.results: list[dict[str, Any]] = []
        self.identities: dict[int, int] = {}
        self.python_by_native_id: dict[int, int] = {}
        self.priorities: dict[int, int] = {}
        self.next_id = 1
        self.next_priority = 0
        self.next_client = 1

    @staticmethod
    def identity(sequence: int) -> dict[str, int]:
        """Build a native public order identity.

        Args:
            sequence (int): Monotonic accepted-order sequence.

        Returns:
            dict[str, int]: Engine-scoped native order ID.
        """
        return {"engine_id": 1, "sequence": sequence}

    def apply(self, command: dict[str, Any]) -> None:
        """Execute and normalize one supported reference command.

        Args:
            command (dict[str, Any]): Native command representation.
        """
        self.commands.append(command)
        operation = command["type"]
        side = Side.BUY if command.get("side") == "buy" else Side.SELL
        try:
            if operation == "add":
                response = self.engine.add_maker_order(
                    str(command["client_id"]),
                    str(command["account_id"]),
                    side,
                    command["price"],
                    command["size"],
                )
            elif operation == "take":
                response = self.engine.match_taker_order(
                    str(command["client_id"]),
                    str(command["account_id"]),
                    side,
                    command["limit_price"],
                    command["size"],
                )
            else:
                native_id = command["order_id"]["sequence"]
                python_id = self.python_by_native_id.get(native_id, -1)
                order = self.engine.get_order(python_id)
                client_id = order.client_id if order else "missing-order"
                if operation == "cancel":
                    response = self.engine.cancel_maker_order(client_id)
                else:
                    response = self.engine.amend_maker_order(
                        client_id, command["price"], command["size"]
                    )
        except MatchingEngineError as error:
            codes: dict[type[MatchingEngineError], str] = {
                NoLiquidityError: "no_liquidity",
                OrderAlreadyExistsError: "duplicate_client_id",
                OrderCrossesSpreadError: "would_cross",
                OrderNotFoundError: "order_not_found",
                SelfTradeError: "self_trade",
            }
            code = codes[type(error)]
            self.results.append({"response": None, "error": code})
            return

        if operation in ("add", "amend", "take"):
            index = 1 if operation == "amend" else 0
            python_id = response.order_deltas[index].order_id
            self.identities[python_id] = self.next_id
            self.python_by_native_id[self.next_id] = python_id
            self.priorities[python_id] = self.next_priority
            self.next_id += 1
            self.next_priority += 1

        fills = [
            {
                "taker_order_id": self.identity(self.identities[fill.taker_order_id]),
                "maker_order_id": self.identity(self.identities[fill.maker_order_id]),
                "taker_client_id": int(fill.taker_client_id),
                "maker_client_id": int(fill.maker_client_id),
                "taker_account_id": int(fill.taker_account_id),
                "maker_account_id": int(fill.maker_account_id),
                "price": fill.price,
                "size": fill.size,
                "taker_side": "buy" if fill.taker_side == Side.BUY else "sell",
            }
            for fill in response.fills
        ]
        order_deltas = [
            {
                "order_id": self.identity(self.identities[delta.order_id]),
                "client_id": int(delta.client_id),
                "status": delta.status.name.lower(),
                "remaining_size": delta.remaining_size,
                "filled_size": delta.filled_size,
                "fill_price": delta.fill_price or None,
            }
            for delta in response.order_deltas
        ]
        level_deltas = [
            {
                "price": delta.price,
                "size": delta.size,
                "side": "buy" if delta.side == Side.BUY else "sell",
                "order_count": delta.order_count,
            }
            for delta in response.level_deltas
        ]
        self.results.append(
            {
                "response": {
                    "fills": fills,
                    "order_deltas": order_deltas,
                    "level_deltas": level_deltas,
                },
                "error": None,
            }
        )

    def generate(self, count: int) -> None:
        """Generate reproducible valid-domain and business-rejection scenarios.

        Args:
            count (int): Number of commands to generate.
        """
        for _ in range(count):
            snapshot = self.engine.export_snapshot()
            orders = [
                order
                for level in snapshot.bids + snapshot.asks
                for order in level.orders
            ]
            choice = self.rng.random()
            account = self.rng.randrange(1, 9)
            if orders and choice < 0.35:
                order = self.rng.choice(orders)
                command = {
                    "type": "cancel" if choice < 0.18 else "amend",
                    "account_id": int(order.account_id),
                    "order_id": self.identity(self.identities[order.order_id]),
                }
                if command["type"] == "amend":
                    command.update(
                        price=self.rng.randrange(90, 111),
                        size=self.rng.randrange(1, 30),
                    )
            else:
                taking = choice > 0.7
                command = {
                    "type": "take" if taking else "add",
                    "account_id": account,
                    "client_id": self.next_client,
                    "side": self.rng.choice(["buy", "sell"]),
                    "size": self.rng.randrange(1, 80 if taking else 30),
                    "limit_price" if taking else "price": self.rng.randrange(90, 111),
                }
                self.next_client += 1
            self.apply(command)

    def final_orders(self) -> list[dict[str, Any]]:
        """Normalize the final resting book in public price-time order.

        Returns:
            list[dict[str, Any]]: Expected native resting-order values.
        """
        snapshot = self.engine.export_snapshot()
        return [
            {
                "order_id": self.identity(self.identities[order.order_id]),
                "client_id": int(order.client_id),
                "account_id": int(order.account_id),
                "price": order.price,
                "size": order.size,
                "side": "buy" if order.side == Side.BUY else "sell",
                "sequence": self.priorities[order.order_id],
            }
            for level in snapshot.bids + snapshot.asks
            for order in level.orders
        ]


def main() -> None:
    """Compare complete Rust responses and final state for several seeded traces."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--commands", type=int, default=10_000)
    parser.add_argument("--seeds", type=int, nargs="+", default=[7, 41, 2026])
    parser.add_argument(
        "--binary", type=Path, default=ROOT / "target/release/cxchange-replay"
    )
    args = parser.parse_args()
    if args.commands < 1:
        parser.error("--commands must be positive")
    for seed in args.seeds:
        trace = ReferenceTrace(seed)
        trace.generate(args.commands)
        request = {
            "engine_id": 1,
            "spec": {
                "symbol": "BTC-USD",
                "tick_size": {"mantissa": 1, "scale": 0},
                "lot_size": {"mantissa": 1, "scale": 0},
                "min_size": 1,
                "max_size": None,
            },
            "config": {"max_orders_per_level": 128, "max_levels": 1024},
            "commands": trace.commands,
            "roundtrip_after": len(trace.commands) // 2,
        }
        completed = subprocess.run(
            [str(args.binary)],
            input=json.dumps(request),
            capture_output=True,
            text=True,
            check=True,
        )
        native = json.loads(completed.stdout)
        if len(native["results"]) != len(trace.results):
            raise AssertionError(f"seed {seed}: command result count differs")
        for index, (expected, actual) in enumerate(
            zip(trace.results, native["results"], strict=True)
        ):
            if expected != actual:
                raise AssertionError(
                    f"seed {seed}, command {index}: {trace.commands[index]}\n"
                    f"Python: {expected}\nRust: {actual}"
                )
        actual_orders = [
            order
            for level in native["snapshot"]["bids"] + native["snapshot"]["asks"]
            for order in level["orders"]
        ]
        if actual_orders != trace.final_orders():
            raise AssertionError(f"seed {seed}: final resting state differs")
        if (
            native["snapshot"]["next_order_id"] != trace.next_id
            or native["snapshot"]["next_sequence"] != trace.next_priority
        ):
            raise AssertionError(f"seed {seed}: normalized sequence counters differ")
        print(
            f"seed {seed}: {len(trace.commands):,} responses, snapshot continuation, and final book agree"
        )


if __name__ == "__main__":
    main()
