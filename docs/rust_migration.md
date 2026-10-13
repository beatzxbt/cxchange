# Rust migration

The matching engine now uses a native Rust core. Source, tests, development
commands, and CI require no Python installation or bindings.

## Completed design stages

1. **Typed boundary.** Exact numeric units, errors, configuration, and identity
   rules are defined in the [matching-core contract](rust_matching_core_contract.md).
2. **Storage and price index.** Private generation-checked arena slots,
   free-slot reuse, indexed FIFO links, ordered bid/ask levels, and checked
   aggregate quantities keep public identity independent of storage.
3. **Maker operations.** Add, cancel, and amend enforce account ownership,
   resulting-state capacity, FIFO rules, and rejection atomicity.
4. **Taker matching.** Price-time traversal and reached-only self-trade
   detection produce deterministic events and complete eligible-book sweeps.
5. **Snapshots and replay.** Validated, versioned snapshots preserve identities
   and reproduce subsequent public results after serialized recovery.
6. **Execution and measurement.** Bounded admission and response ownership
   separate backpressure from matching results. The latency harness measures
   queue wait, matching, and delivery.
7. **Reference retirement.** Shared-domain comparison was used during the
   rewrite. A subsequent [test review](python_test_review.md) selected useful
   behavioral gaps for native tests; the Python implementation, tests,
   comparator, dependency files, and CI job were then removed.

## Contract decisions

The Rust core accepts typed integer IDs and exact instrument units. External
string or decimal parsing belongs to future caller adapters. Numeric
account/client IDs, account-scoped live client-ID uniqueness, engine-scoped
exchange IDs, ownership checks, and logical capacity limits are explicit.

Rejected commands, including no-liquidity takers, consume no exchange-ID or
priority counters. Successful amendments receive a fresh exchange ID and reset
FIFO priority. Transient takers never enter the live client-ID index. These
rules are tested directly against the native contract.

The native suite includes an independent vector-based matching model,
directed boundary scenarios, full-capacity delivery, and recovery at every
boundary of a mixed command trace. Test selection prioritizes observable
correctness and meaningful state transitions. Python object mechanics and
superseded contract behavior are not requirements of the Rust engine.

## Tooling

The default Make targets run native checks:

- **make format**
- **make typecheck**
- **make test**
- **make test-release**

**make replay-release** builds the command replay CLI. The replay and latency
binaries are **cxchange-replay** and **cxchange-latency** in the matching-core
crate. The library API and replay format are not yet stable release interfaces;
snapshots have an explicit version.

Continuous integration checks formatting, Clippy, debug tests, and release
tests with overflow checks. Shared CI is a correctness gate. See
[correctness evidence](correctness.md) for the coverage map and reproduction
commands.

## Remaining integration and performance work

Networking, authentication, risk, settlement, and durable acknowledgement
belong to later integration layers.

The performance harness reports absolute tail latencies and p99.9/p50,
including legitimate full sweeps and ordinary commands queued behind them.
Performance thresholds require a reproducible, controlled baseline; current
workstation results and limitations are recorded in
[performance methodology](performance.md). The ordered price maps may allocate,
so the core makes no hard real-time or allocation-free claim.
