# Matching correctness evidence

The native suite checks behavior at several independent boundaries. Test counts
alone are not an acceptance criterion: assertions compare complete responses,
live state, aggregates, identity counters, and continuation after rejection or
recovery.

The suite passes 73 tests in each debug and release run, including the
documentation example. Both builds use overflow checks. CI checks the native
workspace on Linux. An independent native model provides ongoing behavioral
comparison without a second language implementation.

| Area | Evidence |
| --- | --- |
| Primitive validation and storage | Unit tests in `types.rs` and `arena.rs`: exact units, limits, response reservation, deterministic reuse, stale generations, and generation exhaustion |
| Ordinary matching | `tests/core.rs`: maker operations, ownership, FIFO, amendments, partial fills, reached-only self-trade prevention, and rejected-command atomicity |
| Independent native oracle | `tests/audit_matching.rs`: a simple vector-based book independently computes every response and query after three seeded 300-command traces; directed cases cover both sides |
| Boundary behavior | `tests/audit_edges.rs`: 128 orders per level, 1,024 combined levels, resulting-state amend capacity, insufficient output storage, stale identities, exact maximum quantities, and counter exhaustion |
| Maximum legal result | `tests/core.rs` and `tests/default_sweep_execution.rs`: 131,072 makers swept through the core and execution adapter; every delivered fill and maker delta is checked, including held-output ownership while a later command executes |
| Snapshot validation | `tests/snapshot.rs`: malformed schema, levels, ordering, identities, instrument rules, capacities, counters, and partial remainders below minimum order size |
| Recovery continuity | `tests/audit_integration.rs`: serialize and restore at all 23 boundaries of a mixed trace, then compare every following result and snapshot |
| Replay serialization | `tests/audit_integration.rs`: stdin CLI replay at zero, middle, and end boundaries; exact level aggregates above `u64::MAX` survive JSON |
| Admission and delivery | `tests/execution.rs` and `tests/audit_integration.rs`: FIFO execution, slow consumers, bounded credits, foreign recycling, dropped leases, and complete configured-book sweeps |
| Client identity | `tests/identity_contract.rs`: transient takers cannot replace a live maker's account-scoped client mapping |
| Selected legacy gaps | `tests/behavior_gaps.rs`: amend rejection atomicity and exact size boundaries, identical-value amend priority reset, limited bid/ask depth after state changes, and post-fill client-ID reuse |
| Public usage | The crate's documentation example exercises reusable command output, a partial fill, and serialized recovery |

## Reproduce

```sh
make format
make typecheck
make test
make test-release
```

All current checks use Rust. The replay CLI is exercised by native integration
tests; `make replay-release` also builds it for standalone command traces.

## Legacy test selection

During the initial rewrite, 218 Python tests and 300,000 shared-domain
comparison commands passed. Those are historical evidence, not current CI
gates or a claim of one-to-one test migration.

A Sol 6 xhigh agent subsequently reviewed every legacy test module against the
native contract and suite. The [test review](python_test_review.md) records
which scenarios were already covered, which additional cases were useful, and
which implementation details or superseded behaviors were discarded. Selected
behavioral gaps were added as native tests before removing the Python source,
tests, dependency files, comparator, and CI job.

## Review process and limits

Implementation components received separate Sol 6 high reviews. Three Sol 6
xhigh agents then independently added common-case, boundary, and integrated
tests. Their findings were checked against executing code before changes were
accepted. Snapshot validation was strengthened to reject impossible identity
and counter combinations; zero-boundary CLI recovery now passes through JSON.

This evidence is not a formal proof. Allocator/process failure recovery,
network delivery, authentication, risk, settlement, and durable command
acknowledgement are outside this in-process core. Returned command rejections
are atomic; the ordered maps' allocator-failure boundary is documented in the
[contract](rust_matching_core_contract.md). Latency measurements and correctness
gates are separate; see [performance methodology](performance.md).
