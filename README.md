# CXchange

CXchange is building a native Rust matching core for deterministic, price-time
priority order matching.

The Rust core is an in-process library. Networking, authentication, risk checks,
settlement, and durable command acknowledgement belong to later integration
layers.

## Status

The Rust core implements maker add/cancel/amend, taker matching, validated
snapshots, replay, and bounded command/output ownership. The
native API and replay format are not yet stable release interfaces. The
[matching-core contract](docs/rust_matching_core_contract.md) records the
implementation baseline; the [migration notes](docs/rust_migration.md) describe
the stages and validation approach.

## Design

Each symbol has one mutable state owner. Public exchange IDs resolve through
an ordered index to private arena handles; linked FIFO queues allow cancellation
without scanning a price level. Client IDs are scoped to an account and kept
separate from exchange IDs and storage handles.

The default book permits 128 orders per level and 1,024 active levels across
both sides. Commands plan their complete results before committing, including
a legal sweep of all 131,072 makers. Reusable output buffers and admission
credits keep slow consumers from changing the result of admitted commands.
Ordered maps may allocate; this baseline makes no hard real-time claim.

| Component | Responsibility |
| --- | --- |
| `types.rs` | Exact units, IDs, commands, responses, errors, and logical limits |
| `arena.rs`, `book.rs` | Private storage, FIFO links, ordered prices, and indexes |
| `engine.rs` | Validation, planning, matching, and immutable queries |
| `snapshot.rs` | Versioned state export and complete restore validation |
| `execution.rs` | Bounded FIFO admission and reusable response ownership |
| Replay and latency binaries | Command replay, recovery, and measurement |

See [correctness evidence](docs/correctness.md) for the independent model,
boundary tests, and recovery checks. See
[performance methodology](docs/performance.md) for measurement limits.

## Quickstart

Install a stable Rust toolchain and make. To run the native workspace checks:

    make format
    make typecheck
    make test
    make test-release

The release test target enables overflow checks. Development and CI use Rust
only. The legacy test review and selected additional coverage are recorded in
[the test review](docs/python_test_review.md).

## Development targets

| Target | Purpose |
| --- | --- |
| make format | Format Rust workspace code |
| make typecheck | Run Clippy across workspace targets with warnings denied |
| make docs | Build public and internal rustdoc with warnings denied |
| make test | Run Rust workspace tests |
| make test-release | Run release tests with overflow checks enabled |
| make replay-release | Build the release command replay CLI |

Run `make docs` to build both the public API and private storage/planning
documentation at `target/doc/matching_core/index.html`. The crate overview
includes a command/recovery example. Run `cargo doc --open` for the public API
view.
