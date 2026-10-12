# Rust migration

The matching engine is moving from the Python reference to a native Rust
matching core. The Python implementation and its tests are retained under
reference/python while the Rust contract, implementation, and replay checks
are completed. This migration does not add Python bindings.

## Migration sequence

Each stage builds on the contracts and evidence from the previous stages:

1. **Workspace and typed boundary.** Establish the Rust workspace, command and
   event types, exact numeric units, errors, configuration, and the identity
   rules in the [matching-core contract](rust_matching_core_contract.md).
2. **Order storage.** Add private arena slots, generation-checked handles,
   free-slot reuse, and indexed FIFO links. Public IDs remain independent of
   storage details.
3. **Levels and price index.** Add ordered bid/ask levels and checked aggregate
   quantities. Keep the first price index behind a private component boundary.
4. **Maker operations.** Implement add, cancel, and amend with account
   ownership, resulting-state capacity checks, FIFO rules, and rejection
   atomicity.
5. **Taker matching.** Implement price-time traversal, self-trade detection,
   full eligible-book fills, deterministic fill events, and complete sweep
   responses.
6. **Snapshots and replay.** Validate snapshots before activation and verify
   that restored engines preserve identity and reproduce public results.
7. **Reference comparison.** Run shared command sequences through Rust and
   Python, compare normalized events and state, and document intentional
   differences in the Python adapter.
8. **Execution and latency harness.** Add bounded admission/output ownership
   and measure queue wait, matching, and event delivery separately.
9. **Integration cutover.** Replace the Python engine at its caller only after
   the core, snapshot, and reference evidence is reviewable. Networking, risk,
   settlement, and durable acknowledgement remain separate work.

## Compatibility method

The Rust core accepts typed integer IDs and exact instrument units. The
differential harness generates commands in shared integer ranges, maps native
numeric IDs to string IDs for the Python reference, then normalizes reference
event identities and counters before comparing complete results with Rust.
External decimal parsing belongs to future caller adapters.
Compatibility tests focus on command results, event order, snapshots, FIFO
priority, aggregates, and the state after both accepted and rejected commands.

The native boundary introduces numeric account/client IDs, account-scoped live
client-ID uniqueness, explicit engine-scoped exchange IDs, ownership checks,
and logical capacity limits. The shared differential workload uses inputs
valid in both implementations; native tests cover the new boundary rules.
A no-liquidity rejection no longer consumes order-ID or priority counters.
The comparator normalizes this deliberate counter difference and compares
every event and final live order. Successful amendments retain the Python
rule of receiving a fresh exchange ID and resetting FIFO priority.

## Tooling

The default Make targets run native Rust checks:

- **make format**
- **make typecheck**
- **make test**
- **make test-release**

Python reference checks remain explicit:

- **make python-reference-format**
- **make python-reference-typecheck**
- **make python-reference-test**
- **make differential**

The differential target builds the release replay binary first. The replay
and latency harnesses are **cxchange-replay** and
**cxchange-latency** binaries in the matching-core crate. The replay format and
Rust library API remain implementation details; snapshots have an explicit
version, but the API is not yet a stable release interface.

Continuous integration checks Rust formatting, Clippy, debug tests, and
release tests with overflow checks. A separate reference job installs Python
3.14.8, runs the Python suite, builds the release replay CLI, and compares
results. Shared CI is a correctness gate, not a latency gate.

## Acceptance and limits

Correctness gates include typed input validation, counter stability on
rejection, FIFO order, account-scoped client order IDs, level/order capacity
boundaries, checked aggregates, full-sweep completeness, deterministic
responses, and snapshot/replay continuation.

The performance harness will report absolute tail latencies and p99.9/p50,
including legitimate full sweeps and the ordinary commands queued behind
them. It will record the actual hardware, workload, occupancy, build mode, and
sample count. Performance thresholds will be set only after a reproducible
baseline exists. The initial ordered price map may allocate, so the core makes
no hard real-time or allocation-free claim.
