# Rust matching-core contract

This document records the implementation baseline for the native Rust matching
core. It guides the first implementation and its compatibility checks; it is
not a promise of latency or a stable external wire protocol.

The Python reference is maintained under reference/python during the migration.
The adapter and differential harness make deliberate differences explicit
instead of treating every Python behavior as a requirement of the core.

## Goals and boundaries

- Match commands for each symbol sequentially under one authoritative state
  owner.
- Preserve price-time priority and deterministic results for a fixed command
  sequence, configuration, and engine identity.
- Validate and secure required storage before mutation. A rejected command
  leaves book state and order/priority counters unchanged.
- Allow a taker to fill all eligible opposite-side liquidity, bounded only by
  admitted live orders and available pre-admission resources.
- Keep identity, numeric, capacity, and response-ordering rules explicit.

The matching core does not provide networking, authentication, risk checks,
settlement, consensus, or durable acknowledgements. These belong to later
integration layers.

## Initial capacity defaults

These configurable defaults define the starting logical limits:

| Resource | Initial limit | Scope |
| --- | ---: | --- |
| Live orders at one level | 128 | One side and price |
| Active price levels | 1,024 | One symbol, bids and asks combined |
| Live resting orders | 131,072 | Derived from 128 orders at each of 1,024 levels |
| Fills in one command | Up to 131,072 | One per reached resting maker |

An empty level does not count toward the active-level limit. Partial fills do
not create new resting orders or levels. Amend capacity is checked against the
resulting book: replacing an order does not inherently require another live
order slot, and removing the last order at the old level can release level
capacity for the destination. Arena, scratch, and response capacity are checked
before removing the original order. Ordered-map allocation has the system-fault
boundary described below.

A maximum sweep can produce 131,072 fills, 131,073 order deltas (one for the
taker and one for each maker), and 1,024 level deltas. Planning, response
storage, and bounded output ownership must handle the complete legal result.
Ordinary commands operate on the used prefix rather than clearing or scanning
maximum-sized buffers.

Logical capacity and operational pressure have different behavior:

- Versioned logical limits produce deterministic capacity rejections.
- Queue or output-buffer pressure applies backpressure before command
  admission. Consumer timing must not decide the result of an admitted command.
- Startup allocation failure or a system fault is not a replay-dependent
  trading rejection.
- A sweep is never silently truncated to satisfy a time budget.

## Identity and numeric types

The initial public Rust types use safe, typed numeric identifiers:

| Type | Representation and scope |
| --- | --- |
| AccountId | Newtype over u64 |
| ClientOrderId | Newtype over u64; unique per account among live orders |
| Price | u64 integer instrument units |
| Quantity | u64 integer instrument units |
| OrderId | Pair of u64 values: engine_id and sequence |
| DecimalScale | Exact decimal value with a u64 mantissa and u8 scale, at most 18 |
| Level aggregate quantity | u128 |

Floating-point values are not accepted by the core. Adapters convert external
decimal strings or other exact decimal forms into instrument units before
submitting commands.

The canonical caller assigns a nonzero engine instance identity. It must be
unique within the identity domain in which engine instances can be active or
restored. The identity is not derived from wall-clock time or a random value.
An order ID combines that engine identity with a monotonically increasing
sequence. Sequence exhaustion never wraps: the core fails the command before
book or counter mutation.

Snapshot version 1 records the next unused exchange-ID and FIFO counters.
The exchange-ID counter starts at 1 and FIFO counter at 0; every accepted add,
amend, or taker advances both once. Restore requires `next_order_id =
next_sequence + 1` and each live order's exchange sequence to equal its FIFO
sequence plus 1. Cancels and rejected commands advance neither counter.

Client order IDs are account-scoped. A live client order ID cannot identify
two live orders for the same account. It may be reused after the earlier order
is terminal. Exchange order IDs, client order IDs, and private storage handles
are separate concepts. Public exchange IDs resolve through an index; arena
slot and generation handles remain private so storage can change without
changing public identity.

Takers never enter the live client-ID index. Their supplied client ID may
repeat a live maker's client ID or a previous taker's; the distinct exchange
order ID identifies each execution. A taker cannot replace a maker's client
mapping. Self-trade prevention uses account ownership, independently of client
IDs.

All arithmetic that can affect validation, price/quantity aggregates, or
capacity calculations is checked. Release tests run with overflow checks
enabled. Aggregate totals use u128, which can represent the maximum live
quantity under the initial order-count and per-order quantity bounds.

## Command behavior

### Maker add

Reject duplicate live client IDs for the same account, invalid input, crossing
prices, exhausted identity sequence, and resulting capacity violations before
changing the book or counters. Accepted makers append to their price level's
FIFO.

### Cancel

Resolve a live public order ID, validate account ownership, unlink the order,
update level aggregates, and remove an empty level. Unknown, stale, or
wrong-owner references cannot affect another order.

### Amend

Validate the complete replacement, resulting capacity, and response/storage
requirements before mutation. A rejected amend preserves the original order
and counters. Every successful amend receives a fresh exchange order ID and
resets FIFO priority by re-entering at the tail of its destination level,
including when the price is unchanged.

### Taker

Visit eligible opposite-side levels from best to worst and orders at each level
in FIFO order. Self-trade checks cover the makers the requested quantity would
actually reach; reaching a maker owned by the taker account rejects the whole
command before mutation.

A taker can fill the entire eligible opposite book. Unfilled quantity is
discarded, matching the Python reference: a partial fill succeeds and does not
rest a remainder, while zero eligible fill returns a no-liquidity error.
Pre-admission resource pressure rejects or delays admission before matching; it
does not limit fills after the command is admitted.

### Rejections and counters

Every rejected command leaves book state, exchange order-ID sequence, and
priority sequence unchanged. This includes validation, ownership,
self-trade, capacity, no-liquidity, and sequence-exhaustion errors. This is a
deliberate normalization of the Python reference's no-liquidity behavior,
which currently advances counters before returning its error. The Python
adapter will make the difference explicit in differential results.

### Queries, snapshots, and responses

Queries return values or controlled views that cannot mutate authoritative
orders or invalidate aggregates. Snapshot restore validates the complete
snapshot before activating state; continued execution preserves identity and
sequencing and reproduces the same public results.

Response ordering is part of the contract:

- Taker order delta precedes maker deltas.
- Amend's old-order replacement delta precedes its new-order open delta.
- Fill events follow execution order.
- Maker add and cancel return the final state of their affected level. Amend
  returns the old level's state after removal, followed by the destination
  level's state after insertion. If price and side are unchanged, both deltas
  remain in that order for the same level: the first is the removal state and
  the second is the final state after the new order is appended.
- A taker emits at most one final level delta per touched price level. Deltas
  are ordered by the first visit to each level during best-to-worst traversal,
  even when a level contains multiple fills.
- A same-price amend still emits an old-order replacement followed by a new
  open order; it does not preserve the old order ID or FIFO position.

## Storage and implementation boundary

The first implementation uses one matching-core library with focused modules
for types/configuration, order storage, price levels/index, matching, and
snapshot/replay. A private ordered price map is the initial price index.
Preallocated arena storage maintains indexed FIFO links, and reusable scratch
and response capacity supports command planning and event construction.

The ordered maps may allocate during commit, including when inserting a new
amend destination. Rust's `BTreeMap` does not provide fallible reservation;
allocator exhaustion here is a process/system fault, not a recoverable command
rejection. Atomicity covers returned errors, not recovery from process failure.
Startup arena, scratch, and output reservations use fallible allocation.
This baseline makes no hard real-time or allocation-free claim. Specialized
indexes, packing, and CPU tuning require benchmark evidence and must preserve
the public contract.

The command path validates input, checks arithmetic, plans the complete
operation, and checks bounded storage and output capacity before commit.
Only the symbol's state owner mutates the authoritative book.

## Conformance and performance evidence

Correctness checks cover capacity boundaries, resulting-state amendments,
ownership, FIFO head/middle/tail cancellation, slot reuse, single and full-book
sweeps, late self-trade conflicts, exact arithmetic, response completeness,
aggregate conservation, rejected-command atomicity, snapshots, and replay.

The latency harness reports absolute p50, p95, p99, and p99.9 plus
p99.9/p50. It measures ordinary operations and sweeps separately, includes
full sweeps in mixed workloads, and measures queue waiting separately from
matching and event delivery. It reports offered/admitted/rejected counts,
slow-consumer behavior, hardware, build settings, occupancy, and actual sample
counts. Warm add/cancel/amend classes target at least one million successful
measured commands across repeated runs; full-capacity and pressure profiles
are exploratory and state their smaller sample sizes. Timing thresholds
will be chosen from a controlled baseline, not shared-CI latency.
