# Matching correctness evidence

The native suite checks behavior at several independent boundaries. Test counts
alone are not an acceptance criterion: assertions compare complete responses,
live state, aggregates, identity counters, and continuation after rejection or
recovery.

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
| Python comparison | `tools/differential.py`: compare every response/error, final live order, and normalized counters across seeded traces with a serialized restore halfway through |
| Public usage | The crate's documentation example exercises reusable command output, a partial fill, and serialized recovery |

## Reproduce

```sh
make format
make typecheck
make test
make test-release
make python-reference-test
make python-reference-typecheck
make differential
```

The default differential run executes 10,000 commands for each of seeds 7, 41,
and 2026. For a larger comparison:

```sh
make replay-release
uv run --directory reference/python --extra dev python ../../tools/differential.py --commands 100000
```

The comparator preserves event order and values. It normalizes exchange IDs
and priorities only to account for Python consuming counters on no-liquidity
rejections. Its generated workload uses globally unique client IDs and valid
shared input ranges. Native tests separately check account-scoped IDs,
ownership, input rejection, and capacities that Python does not enforce.

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
