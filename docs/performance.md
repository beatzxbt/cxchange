# Matching core execution and latency harness

## Bounded execution adapter

`matching_core::execution::ExecutionAdapter` owns one `MatchingEngine` and
processes admitted commands in FIFO order. Configure a bounded command queue
and a fixed response pool at startup. Each admission reserves a queue entry and
a response credit before assigning its monotonic `SubmissionId`.

Admission failures are returned as `AdmissionError`. They do not call the
matching engine or change book state. A command that is admitted always runs;
a canonical matching rejection is returned in its completed result and does
not prevent later commands from completing.

Consumers take completed responses and return them with `recycle`. A completed
response owns its pool credit. Dropping it without recycling permanently loses
that credit until the adapter is dropped, so an adapter with exhausted credits
continues to fail closed with `ResponsePoolExhausted`. Keep completed results
short lived and recycle them after reading their immutable response data.

The adapter reserves queue storage and reusable response buffers before it is
ready. The matching engine can still allocate inside its ordered indexes, so
this is a bounded adapter and response path, not a claim that all matching
operations are allocation free.

## Run the harness

The harness prints JSON to stdout by default:

```sh
cargo run --release -p matching-core --bin cxchange-latency -- --quick
```

For a measurement run with at least one million offered commands in each warm
ordinary class, save the report to a file:

```sh
cargo run --release -p matching-core --bin cxchange-latency -- \
  --samples 1100000 --rate 50000 --output latency.json
```

Options:

- `--samples N` requests N commands per warm Add, Cancel, and Amend class. The
  default is 1,000,000. The empty-book Add/Cancel profile is capped at 1,000
  commands per class.
- `--full-sweep-samples N` controls the separate full-book sweep count. Its
  default is 10.
- `--rate HZ` schedules ordinary arrivals at the requested open-loop rate.
  The report also gives the effective rate after converting it to a whole
  nanosecond interval.
- `--output PATH` writes JSON to a file instead of stdout.
- `--quick` uses 1,000 ordinary commands per requested class and one full-book
  sweep. It is useful for checking the harness and JSON shape, not for stable
  tail-latency comparisons.

The JSON records the engine configuration and queue/response-pool capacities
for every profile. It includes requested, admitted, admission-rejected, and
matching-rejected counts; queue and response-credit occupancy at start, peak,
and end; fill/level classification; platform, CPU, and Rust toolchain metadata;
and latency distributions for scheduled-to-start, queue wait, execution,
delivery wait, and scheduled-to-delivery.

Occupancy telemetry currently covers the adapter queue and response credits.
Initial live-book occupancy follows the profile setup in the table below;
live-book peak and final counts are not instrumented. This distinction matters
when interpreting capacity and queue effects.

Latency percentiles include only commands that completed with a successful
matching outcome. Matching rejections are counted separately and excluded from
valid-operation latency distributions. Admission rejections have no execution
latency. Check `admitted`, `matching_rejected`, `admission_rejected`, and the
per-component `samples` before comparing runs. A requested count is a target,
not a guarantee that the adapter admitted that many commands under load.

p50, p95, and p99 are reported when there are samples. p99.9 and its ratio to
p50 are reported only when the profile enables tail reporting and has at least
1,000 successful samples. Full-sweep, mixed-sweep, and held-credit pressure
profiles explicitly omit p99.9 because their sample counts are intended for
functional characterization rather than tail claims.

## Workload profiles

| Profile | Book and adapter setup | Purpose |
| --- | --- | --- |
| Empty-book Add/Cancel | Small 32-level configuration, starts empty, capped at 1,000 commands per class | Measures churn without a retained maker |
| Retained-anchor Add/Cancel | Small 32-level configuration, one resting order retained | Measures churn with a nonempty level |
| Retained-anchor Amend | Small 32-level configuration, repeatedly amends one resting order | Measures the amend path |
| Full-book churn | Default configuration: 128 orders per level × 1,024 levels = 131,072 live orders; up to 10,000 arrivals per class | Measures cancel/add churn at the default capacity |
| FIFO cancel positions | Default-capacity full book, with a 128-order target level; reports head, middle, tail, replacement adds, and a cancel-position aggregate separately | Compares order removal positions at full occupancy |
| Full sweep | Default-capacity full ask book, swept by one taker; default 10 samples | Confirms and measures 131,072 fills; p99.9 is disabled |
| Mixed sweep catch-up | Default-capacity full book; a same-time arrival burst is scheduled during the sweep; four response credits | Shows producer catch-up and explicit response-pool admission pressure |
| Held-credit pressure | Holds 32 completed Add results, then attempts 64 Cancels before recycling | One-time response-credit saturation check, not a sustained slow-consumer throughput test |

The ordinary Add/Cancel and Amend profiles use the small configuration so the
default million-command runs stay practical. The full-capacity profiles state
their own default engine configuration in the output rather than inheriting
that small limit.

## Timing model and limits

Ordinary arrivals are scheduled from a start `Instant`, independently of
command completion. The single-threaded harness catches up arrivals whose
scheduled offsets elapsed while it was running an earlier command and passes
their original timestamps into the adapter. Producer bookkeeping and clock
reads are part of scheduled-to-start and scheduled-to-delivery measurements.
There is no concurrent producer in this baseline.

The mixed-sweep profile deliberately executes the full sweep before the same
producer can submit arrivals scheduled at sweep start. After the sweep returns,
it catches up the due burst. For these commands, queue wait starts at adapter
admission and does not include time spent inside the blocking sweep. The
scheduled-to-start and scheduled-to-delivery values include that producer
delay. The four-credit pool keeps the sweep result and admits only a small
number of later commands, so the remaining arrivals are recorded as admission
rejections rather than matching rejections.

No latency threshold is used in shared CI. CI checks adapter and engine
behavior; benchmark results are host-, build-, and load-dependent and should be
compared with their recorded environment and actual sample counts.

The mixed profile admits only three post-sweep Adds with its four credits;
Cancels encounter pressure. It provides no mixed Cancel latency distribution
or mixed-workload tail estimate. Full-capacity exploratory profiles also need
larger repeated samples before their p99.9 values can support regression gates.

## Recorded workstation baseline

Two sequential release runs on 2026-10-12 used an Apple M2, 16 GiB RAM,
macOS 26.6.2, Rust 1.95.0, and 50,000 scheduled arrivals/second. Each primary
class requested 1,100,000 commands and completed over one million successfully
in each run. All profiles recorded zero matching rejections. Admission pressure
and skipped dependencies remain in the results.

| Run | Operation | Successful samples | Admission rejections | Matching p50 ns | Matching p99.9 ns | p99.9/p50 | Scheduled delivery p99.9 ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | Add | 1,077,986 | 11,638 | 42 | 1,708 | 40.67 | 3.792250 |
| 1 | Cancel | 1,077,986 | 10,376 | 42 | 2,041 | 48.60 | 3.791625 |
| 1 | Amend | 1,078,460 | 21,540 | 84 | 3,209 | 38.20 | 2.648917 |
| 2 | Add | 1,079,749 | 10,691 | 42 | 1,666 | 39.67 | 1.598583 |
| 2 | Cancel | 1,079,748 | 9,561 | 42 | 2,083 | 49.60 | 1.600666 |
| 2 | Amend | 1,077,192 | 22,808 | 84 | 3,250 | 38.69 | 3.284375 |

The scheduled delivery tails are much larger than matching tails. They include
producer lag, harness bookkeeping, and workstation scheduling. The 42–84 ns
matching medians also make ratios sensitive to measurement granularity. These
results establish a baseline for later work; they do not establish stable
production latency or a regression threshold.

Raw reports preserve all profiles, counts, and timing components:

- [Run 1](benchmarks/apple-m2-run-1.json)
- [Run 2](benchmarks/apple-m2-run-2.json)
- [Environment, build command/profile, source commits, and measurement scope](benchmarks/apple-m2-environment.json)

Each report includes ten separate default-capacity sweeps. Full-book ordinary
churn and FIFO-position samples remain exploratory. Larger repeated samples
and a controlled execution environment are needed before setting their latency
gates.
