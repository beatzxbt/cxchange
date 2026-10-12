//! Open-loop latency harness for bounded command execution and result delivery.
//!
//! It measures add/cancel/amend churn, FIFO cancel positions, near-full book
//! updates, full sweeps, commands scheduled behind sweeps, and slow consumers.

use matching_core::execution::{
    AdmissionError, CompletedResponse, ExecutionAdapter, ExecutionConfig, ExecutionTimings,
    SubmissionId,
};
use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MAX_LIVE_ORDERS, MatchingEngine, MatchingError, MatchingResponse, OrderId, Price, Quantity,
    Side,
};
use std::collections::VecDeque;
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::time::{Duration, Instant};

const PER_LEVEL: usize = 32;
const LEVELS: usize = 32;
const DEFAULT_SAMPLES: usize = 1_000_000;
const PROFILE_SAMPLES: usize = 10_000;
const DEFAULT_RATE: u64 = 50_000;
const MIXED_BURST_COMMANDS: usize = 24;
const MIXED_RESPONSE_CREDITS: usize = 4;

fn main() {
    if let Err(error) = run() {
        eprintln!("cxchange-latency: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if args.help {
        print_help();
        return Ok(());
    }
    let samples = args
        .samples
        .unwrap_or(if args.quick { 1_000 } else { DEFAULT_SAMPLES });
    let sweep_samples = args
        .sweep_samples
        .unwrap_or(if args.quick { 1 } else { 10 });
    if samples == 0 || sweep_samples == 0 || args.rate == 0 {
        return Err("sample counts and rate must be positive".to_owned());
    }
    let interval_ns = (1_000_000_000 / args.rate).max(1);
    let profiles = samples.min(PROFILE_SAMPLES);
    let cold_samples = samples.min(1_000);
    let mut reports = vec![run_add_cancel(
        "add_cancel_empty_book_churn",
        cold_samples,
        interval_ns,
        false,
    )];
    reports.push(run_add_cancel(
        "add_cancel_with_retained_anchor_churn",
        samples,
        interval_ns,
        true,
    ));
    reports.push(run_amend(samples, interval_ns));
    reports.push(run_near_full(profiles, interval_ns));
    reports.push(run_cancel_positions(profiles, interval_ns));
    reports.push(run_full_sweep(sweep_samples));
    reports.push(run_mixed_sweep());
    reports.push(run_slow_consumer());

    let json = render_json(&reports, samples, sweep_samples, args.rate, interval_ns);
    if let Some(path) = args.output {
        fs::write(&path, json)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    } else {
        println!("{json}");
    }
    Ok(())
}

#[derive(Default)]
struct Args {
    samples: Option<usize>,
    sweep_samples: Option<usize>,
    rate: u64,
    output: Option<PathBuf>,
    quick: bool,
    help: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        rate: DEFAULT_RATE,
        ..Args::default()
    };
    let mut values = std::env::args().skip(1);
    while let Some(flag) = values.next() {
        match flag.as_str() {
            "--samples" => args.samples = Some(parse_value("--samples", values.next())?),
            "--full-sweep-samples" => {
                args.sweep_samples = Some(parse_value("--full-sweep-samples", values.next())?)
            }
            "--rate" => args.rate = parse_value("--rate", values.next())?,
            "--output" => {
                args.output = Some(values.next().ok_or("--output requires a path")?.into())
            }
            "--quick" => args.quick = true,
            "--help" | "-h" => args.help = true,
            _ => return Err(format!("unknown option {flag}; use --help")),
        }
    }
    Ok(args)
}

fn parse_value<T: std::str::FromStr>(name: &str, value: Option<String>) -> Result<T, String> {
    value
        .ok_or_else(|| format!("{name} requires a value"))?
        .parse()
        .map_err(|_| format!("invalid value for {name}"))
}

fn print_help() {
    println!(
        "Usage: cargo run --release -p matching-core --bin cxchange-latency -- [OPTIONS]\n\
         --samples N              measured commands per ordinary class (default 1000000)\n\
         --full-sweep-samples N   full-sweep samples (default 10)\n\
         --rate HZ                scheduled arrival rate (default 50000)\n\
         --output PATH            write JSON to PATH instead of stdout\n\
         --quick                  use 1000 ordinary and 1 sweep sample\n\
         --help                   show this help"
    );
}

struct Report {
    name: &'static str,
    sample_intent: &'static str,
    engine_config: EngineConfig,
    command_capacity: usize,
    response_capacity: usize,
    requested_commands_per_class: Option<usize>,
    report_tail: bool,
    metrics: Vec<Stats>,
}

fn run_add_cancel(name: &'static str, samples: usize, interval_ns: u64, warm: bool) -> Report {
    let config = book_config();
    let mut engine = engine(1, config);
    let mut next_sequence = 1u64;
    if warm {
        let mut response = response_for(&config);
        execute(
            &mut engine,
            add_command(10, 1, Side::Buy, 100, 1),
            &mut response,
        )
        .expect("warm-up anchor is valid");
        next_sequence = response.order_deltas[0].order_id.sequence + 1;
    }
    let mut adapter = adapter(engine, 32, 32);
    let mut stats = vec![Stats::new("add"), Stats::new("cancel")];
    stats[0].reserve_latency(samples);
    stats[1].reserve_latency(samples);
    let mut next_client = 10_000u64;
    let mut pending_cancel = None;
    drive(
        &mut adapter,
        samples.saturating_mul(2),
        interval_ns,
        &mut stats,
        |event, scheduled, adapter, outstanding, stats| {
            if event % 2 == 0 {
                if pending_cancel.is_some() {
                    stats[0].skipped_dependencies += 1;
                    next_client += 1;
                    return;
                }
                match submit(
                    adapter,
                    outstanding,
                    stats,
                    Some(0),
                    add_command(10, next_client, Side::Buy, 100, 1),
                    scheduled,
                ) {
                    Ok(_) => {
                        pending_cancel = Some(OrderId::new(EngineId(1), next_sequence));
                        next_sequence += 1;
                    }
                    Err(_) => pending_cancel = None,
                }
                next_client += 1;
            } else if let Some(order_id) = pending_cancel {
                if submit(
                    adapter,
                    outstanding,
                    stats,
                    Some(1),
                    Command::Cancel {
                        account_id: AccountId(10),
                        order_id,
                    },
                    scheduled,
                )
                .is_ok()
                {
                    pending_cancel = None;
                }
            } else {
                stats[1].skipped_dependencies += 1;
            }
        },
    );
    validate(&adapter);
    Report {
        name,
        sample_intent: if warm {
            "primary_target"
        } else {
            "exploratory"
        },
        engine_config: config,
        command_capacity: 32,
        response_capacity: 32,
        requested_commands_per_class: Some(samples),
        report_tail: true,
        metrics: stats,
    }
}

fn run_amend(samples: usize, interval_ns: u64) -> Report {
    let config = book_config();
    let mut engine = engine(2, config);
    let mut response = response_for(&config);
    execute(
        &mut engine,
        add_command(10, 1, Side::Buy, 100, 1),
        &mut response,
    )
    .expect("initial amend target is valid");
    let mut current_sequence = response.order_deltas[0].order_id.sequence;
    let mut next_sequence = current_sequence + 1;
    let mut adapter = adapter(engine, 32, 32);
    let mut stats = vec![Stats::new("amend")];
    stats[0].reserve_latency(samples);
    drive(
        &mut adapter,
        samples,
        interval_ns,
        &mut stats,
        |_, scheduled, adapter, outstanding, stats| {
            let command = Command::Amend {
                account_id: AccountId(10),
                order_id: OrderId::new(EngineId(2), current_sequence),
                price: Price(100),
                size: Quantity(1),
            };
            if submit(adapter, outstanding, stats, Some(0), command, scheduled).is_ok() {
                current_sequence = next_sequence;
                next_sequence += 1;
            }
        },
    );
    validate(&adapter);
    Report {
        name: "amend_one_order_with_retained_anchor",
        sample_intent: "primary_target",
        engine_config: config,
        command_capacity: 32,
        response_capacity: 32,
        requested_commands_per_class: Some(samples),
        report_tail: true,
        metrics: stats,
    }
}

fn run_near_full(samples: usize, interval_ns: u64) -> Report {
    let config = EngineConfig::default();
    let mut engine = engine(3, config);
    let mut response = response_for(&config);
    let mut by_level = (0..config.max_levels)
        .map(|_| VecDeque::with_capacity(config.max_orders_per_level))
        .collect::<Vec<_>>();
    let mut next_client = 1u64;
    let mut next_sequence = 1u64;
    for (level, orders) in by_level.iter_mut().enumerate() {
        for _ in 0..config.max_orders_per_level {
            execute(
                &mut engine,
                add_command(20, next_client, Side::Buy, 100 + level as u64, 1),
                &mut response,
            )
            .expect("near-full seed is valid");
            orders.push_back(response.order_deltas[0].order_id);
            next_client += 1;
            next_sequence += 1;
        }
    }
    debug_assert_eq!(next_sequence - 1, MAX_LIVE_ORDERS as u64);
    let mut adapter = adapter(engine, 8, 8);
    let mut stats = vec![Stats::new("cancel_near_full"), Stats::new("add_near_full")];
    stats[0].reserve_latency(samples);
    stats[1].reserve_latency(samples);
    let mut pending_level = None;
    drive(
        &mut adapter,
        samples.saturating_mul(2),
        interval_ns,
        &mut stats,
        |event, scheduled, adapter, outstanding, stats| {
            let level = (event / 2) % config.max_levels;
            if event % 2 == 0 {
                if pending_level.is_some() {
                    stats[0].skipped_dependencies += 1;
                    return;
                }
                let Some(order_id) = by_level[level].front().copied() else {
                    stats[0].skipped_dependencies += 1;
                    return;
                };
                let command = Command::Cancel {
                    account_id: AccountId(20),
                    order_id,
                };
                if submit(adapter, outstanding, stats, Some(0), command, scheduled).is_ok() {
                    by_level[level].pop_front();
                    pending_level = Some(level);
                }
            } else if let Some(level) = pending_level {
                let command = add_command(20, next_client, Side::Buy, 100 + level as u64, 1);
                if submit(adapter, outstanding, stats, Some(1), command, scheduled).is_ok() {
                    by_level[level].push_back(OrderId::new(EngineId(3), next_sequence));
                    next_client += 1;
                    next_sequence += 1;
                    pending_level = None;
                }
            } else {
                stats[1].skipped_dependencies += 1;
            }
        },
    );
    validate(&adapter);
    Report {
        name: "full_book_churn_at_default_capacity",
        sample_intent: "exploratory",
        engine_config: config,
        command_capacity: 8,
        response_capacity: 8,
        requested_commands_per_class: Some(samples),
        report_tail: true,
        metrics: stats,
    }
}

fn run_cancel_positions(samples: usize, interval_ns: u64) -> Report {
    let config = EngineConfig::default();
    let mut engine = engine(4, config);
    let mut response = response_for(&config);
    let mut live = VecDeque::with_capacity(config.max_orders_per_level);
    let mut client_id = 1u64;
    for level in 0..config.max_levels {
        for _ in 0..config.max_orders_per_level {
            execute(
                &mut engine,
                add_command(30, client_id, Side::Buy, 100 + level as u64, 1),
                &mut response,
            )
            .expect("full-book FIFO seed is valid");
            if level == 0 {
                live.push_back(response.order_deltas[0].order_id);
            }
            client_id += 1;
        }
    }
    let mut adapter = adapter(engine, 8, 8);
    let mut stats = vec![
        Stats::new("cancel_head"),
        Stats::new("cancel_middle"),
        Stats::new("cancel_tail"),
        Stats::new("replacement_add"),
    ];
    for stat in &mut stats {
        stat.reserve_latency(samples);
    }
    let mut next_client = MAX_LIVE_ORDERS as u64 + 1;
    let mut next_sequence = MAX_LIVE_ORDERS as u64 + 1;
    let mut pending_add = false;
    drive(
        &mut adapter,
        samples.saturating_mul(2),
        interval_ns,
        &mut stats,
        |event, scheduled, adapter, outstanding, stats| {
            if event % 2 == 0 {
                let class = (event / 2) % 3;
                if pending_add {
                    stats[class].skipped_dependencies += 1;
                    return;
                }
                let position = match class {
                    0 => 0,
                    1 => live.len() / 2,
                    _ => live.len().saturating_sub(1),
                };
                let Some(order_id) = live.get(position).copied() else {
                    stats[class].skipped_dependencies += 1;
                    pending_add = false;
                    return;
                };
                let command = Command::Cancel {
                    account_id: AccountId(30),
                    order_id,
                };
                if submit(adapter, outstanding, stats, Some(class), command, scheduled).is_ok() {
                    live.remove(position);
                    pending_add = true;
                } else {
                    pending_add = false;
                }
            } else if pending_add {
                let command = add_command(30, next_client, Side::Buy, 100, 1);
                if submit(adapter, outstanding, stats, Some(3), command, scheduled).is_ok() {
                    live.push_back(OrderId::new(EngineId(4), next_sequence));
                    next_client += 1;
                    next_sequence += 1;
                    pending_add = false;
                }
            }
        },
    );
    let mut aggregate = Stats::new("cancel_head_middle_tail");
    for stat in stats.iter().take(3) {
        aggregate.merge(stat.clone());
    }
    stats.push(aggregate);
    validate(&adapter);
    Report {
        name: "fifo_cancel_positions_at_default_full_book",
        sample_intent: "exploratory",
        engine_config: config,
        command_capacity: 8,
        response_capacity: 8,
        requested_commands_per_class: None,
        report_tail: true,
        metrics: stats,
    }
}

fn run_full_sweep(samples: usize) -> Report {
    let mut stats = Stats::new("full_sweep");
    stats.reserve_latency(samples);
    let config = EngineConfig::default();
    for sample in 0..samples {
        let mut engine = engine(5 + sample as u64, config);
        let mut seed_response = response_for(&config);
        seed_full_ask_book(&mut engine, &mut seed_response, config);
        let mut adapter = adapter(engine, 1, 1);
        let scheduled = Instant::now();
        let id = adapter
            .try_submit_at(sweep_command(config), scheduled)
            .expect("sweep has a reserved response credit");
        assert_eq!(adapter.process_next(), Some(id));
        let completed = adapter.take_completed().expect("sweep result is ready");
        stats.attempted += 1;
        stats.admitted += 1;
        stats.observe(&completed);
        stats.observe_occupancy(1, 1);
        adapter
            .recycle(completed)
            .expect("return full-sweep response");
        validate(&adapter);
    }
    Report {
        name: "full_sweep_at_default_capacity",
        sample_intent: "exploratory",
        engine_config: config,
        command_capacity: 1,
        response_capacity: 1,
        requested_commands_per_class: Some(samples),
        report_tail: false,
        metrics: vec![stats],
    }
}

fn run_mixed_sweep() -> Report {
    let config = EngineConfig::default();
    let mut engine = engine(6, config);
    let mut seed_response = response_for(&config);
    seed_full_ask_book(&mut engine, &mut seed_response, config);
    let command_capacity = MIXED_BURST_COMMANDS;
    let mut adapter = adapter(engine, command_capacity, MIXED_RESPONSE_CREDITS);
    let mut stats = vec![
        Stats::new("full_sweep"),
        Stats::new("add_scheduled_during_full_sweep"),
        Stats::new("cancel_scheduled_during_full_sweep"),
    ];
    stats[0].reserve_latency(1);
    stats[1].reserve_latency(MIXED_BURST_COMMANDS / 2);
    stats[2].reserve_latency(MIXED_BURST_COMMANDS / 2);
    let mut outstanding = VecDeque::with_capacity(MIXED_RESPONSE_CREDITS);
    let start = Instant::now();
    let sweep_id = submit(
        &mut adapter,
        &mut outstanding,
        &mut stats,
        Some(0),
        sweep_command(config),
        start,
    )
    .expect("sweep admitted");
    assert_eq!(adapter.process_next(), Some(sweep_id));

    let mut next_sequence = MAX_LIVE_ORDERS as u64 + 2;
    let mut next_client = 10_000u64;
    let mut planned_orders = VecDeque::with_capacity(MIXED_BURST_COMMANDS / 2);
    let scheduled = start;
    for index in 0..MIXED_BURST_COMMANDS {
        if index < MIXED_BURST_COMMANDS / 2 {
            if submit(
                &mut adapter,
                &mut outstanding,
                &mut stats,
                Some(1),
                add_command(40, next_client, Side::Buy, 1, 1),
                scheduled,
            )
            .is_ok()
            {
                planned_orders.push_back(OrderId::new(EngineId(6), next_sequence));
                next_sequence += 1;
            }
            next_client += 1;
        } else if let Some(order_id) = planned_orders.front().copied() {
            let _ = submit(
                &mut adapter,
                &mut outstanding,
                &mut stats,
                Some(2),
                Command::Cancel {
                    account_id: AccountId(40),
                    order_id,
                },
                scheduled,
            );
            planned_orders.pop_front();
        } else {
            stats[2].skipped_dependencies += 1;
        }
    }
    while adapter.process_next().is_some() {}
    drain(&mut adapter, &mut outstanding, &mut stats);
    validate(&adapter);
    Report {
        name: "single_thread_scheduled_arrival_catch_up_behind_default_full_sweep",
        sample_intent: "exploratory",
        engine_config: config,
        command_capacity,
        response_capacity: MIXED_RESPONSE_CREDITS,
        requested_commands_per_class: None,
        report_tail: false,
        metrics: stats,
    }
}

fn run_slow_consumer() -> Report {
    let config = book_config();
    let mut adapter = adapter(engine(7, config), 32, 32);
    let mut stats = vec![
        Stats::new("add_while_holding_results"),
        Stats::new("cancel_under_held_credit_pressure"),
    ];
    stats[0].reserve_latency(32);
    stats[1].reserve_latency(64);
    let mut outstanding = VecDeque::with_capacity(32);
    let start = Instant::now();
    for index in 0..32 {
        submit(
            &mut adapter,
            &mut outstanding,
            &mut stats,
            Some(0),
            add_command(50, index as u64 + 1, Side::Buy, 100 + index as u64, 1),
            scheduled_at(start, index, 1_000),
        )
        .expect("fill the response pool");
    }
    for _ in 0..32 {
        let _ = adapter.process_next();
    }
    for index in 0..64 {
        let _ = submit(
            &mut adapter,
            &mut outstanding,
            &mut stats,
            Some(1),
            Command::Cancel {
                account_id: AccountId(50),
                order_id: OrderId::new(EngineId(7), index as u64 + 1),
            },
            Instant::now(),
        );
    }
    drain(&mut adapter, &mut outstanding, &mut stats);
    validate(&adapter);
    Report {
        name: "held_response_credit_pressure_check",
        sample_intent: "exploratory",
        engine_config: config,
        command_capacity: 32,
        response_capacity: 32,
        requested_commands_per_class: None,
        report_tail: false,
        metrics: stats,
    }
}

fn seed_full_ask_book(
    engine: &mut MatchingEngine,
    response: &mut MatchingResponse,
    config: EngineConfig,
) {
    let mut client_id = 1u64;
    for level in 0..config.max_levels {
        for _ in 0..config.max_orders_per_level {
            execute(
                engine,
                add_command(20, client_id, Side::Sell, 2 + level as u64, 1),
                response,
            )
            .expect("full-sweep maker is valid");
            client_id += 1;
        }
    }
}

fn sweep_command(config: EngineConfig) -> Command {
    Command::Take {
        account_id: AccountId(10),
        client_id: ClientOrderId(u64::MAX),
        side: Side::Buy,
        limit_price: Price(config.max_levels as u64 + 2),
        size: Quantity(config.validate().expect("valid engine configuration") as u64),
    }
}

fn drive<F>(
    adapter: &mut ExecutionAdapter,
    event_count: usize,
    interval_ns: u64,
    stats: &mut [Stats],
    mut on_arrival: F,
) where
    F: FnMut(usize, Instant, &mut ExecutionAdapter, &mut VecDeque<Tracked>, &mut [Stats]),
{
    let mut outstanding = VecDeque::with_capacity(adapter.config().response_capacity);
    let start = Instant::now();
    let mut next = 0usize;
    while next < event_count || adapter.queued_len() > 0 {
        let now = Instant::now();
        while next < event_count {
            let arrival = scheduled_at(start, next, interval_ns);
            if arrival > now {
                break;
            }
            on_arrival(next, arrival, adapter, &mut outstanding, stats);
            next += 1;
        }
        if adapter.process_next().is_some() {
            drain(adapter, &mut outstanding, stats);
            continue;
        }
        if next < event_count {
            while Instant::now() < scheduled_at(start, next, interval_ns) {
                std::hint::spin_loop();
            }
        }
    }
    debug_assert!(outstanding.is_empty());
}

fn scheduled_at(start: Instant, index: usize, interval_ns: u64) -> Instant {
    let offset = u128::from(interval_ns).saturating_mul(index as u128);
    start
        .checked_add(Duration::from_nanos(offset.min(u128::from(u64::MAX)) as u64))
        .unwrap_or(start)
}

#[derive(Clone, Copy)]
struct Tracked {
    id: SubmissionId,
    metric: Option<usize>,
}

fn submit(
    adapter: &mut ExecutionAdapter,
    outstanding: &mut VecDeque<Tracked>,
    stats: &mut [Stats],
    metric: Option<usize>,
    command: Command,
    scheduled: Instant,
) -> Result<SubmissionId, AdmissionError> {
    if let Some(index) = metric {
        stats[index].attempted += 1;
    }
    match adapter.try_submit_at(command, scheduled) {
        Ok(id) => {
            if let Some(index) = metric {
                stats[index].admitted += 1;
                stats[index].observe_occupancy(
                    adapter.queued_len(),
                    adapter.config().response_capacity - adapter.available_response_credits(),
                );
            }
            outstanding.push_back(Tracked { id, metric });
            Ok(id)
        }
        Err(error) => {
            if let Some(index) = metric {
                stats[index].admission_rejected += 1;
                stats[index].observe_occupancy(
                    adapter.queued_len(),
                    adapter.config().response_capacity - adapter.available_response_credits(),
                );
                match error {
                    AdmissionError::CommandQueueFull => stats[index].queue_full += 1,
                    AdmissionError::ResponsePoolExhausted => stats[index].response_pool_full += 1,
                    AdmissionError::SubmissionIdExhausted => stats[index].submission_id_full += 1,
                }
            }
            Err(error)
        }
    }
}

fn drain(adapter: &mut ExecutionAdapter, outstanding: &mut VecDeque<Tracked>, stats: &mut [Stats]) {
    while let Some(completed) = adapter.take_completed() {
        let tracked = outstanding
            .pop_front()
            .expect("each admitted command has a tracker");
        assert_eq!(tracked.id, completed.submission_id());
        if let Some(index) = tracked.metric {
            stats[index].observe(&completed);
        }
        adapter
            .recycle(completed)
            .expect("return response pool credit");
    }
}

fn validate(adapter: &ExecutionAdapter) {
    assert_eq!(
        adapter.queued_len(),
        0,
        "all admitted commands are processed"
    );
    assert_eq!(adapter.completed_len(), 0, "all results are delivered");
    assert_eq!(
        adapter.available_response_credits(),
        adapter.config().response_capacity,
        "all response credits are recycled"
    );
    adapter
        .engine()
        .validate_invariants()
        .expect("workload preserves matching invariants");
}

#[derive(Clone)]
struct Latencies {
    scheduled_to_delivery: Vec<u64>,
    queue_wait: Vec<u64>,
    execution: Vec<u64>,
    delivery_wait: Vec<u64>,
    scheduled_to_start: Vec<u64>,
}

impl Latencies {
    fn new() -> Self {
        Self {
            scheduled_to_delivery: Vec::new(),
            queue_wait: Vec::new(),
            execution: Vec::new(),
            delivery_wait: Vec::new(),
            scheduled_to_start: Vec::new(),
        }
    }

    fn push(&mut self, timings: ExecutionTimings) {
        self.scheduled_to_delivery
            .push(duration_ns(timings.scheduled_to_delivery));
        self.queue_wait.push(duration_ns(timings.queue_wait));
        self.execution.push(duration_ns(timings.execution));
        self.delivery_wait.push(duration_ns(timings.delivery_wait));
        self.scheduled_to_start
            .push(duration_ns(timings.scheduled_to_start));
    }

    fn merge(&mut self, other: &mut Self) {
        self.scheduled_to_delivery
            .append(&mut other.scheduled_to_delivery);
        self.queue_wait.append(&mut other.queue_wait);
        self.execution.append(&mut other.execution);
        self.delivery_wait.append(&mut other.delivery_wait);
        self.scheduled_to_start
            .append(&mut other.scheduled_to_start);
    }
}

#[derive(Clone)]
struct Stats {
    name: &'static str,
    attempted: u64,
    admitted: u64,
    admission_rejected: u64,
    matching_rejected: u64,
    queue_full: u64,
    response_pool_full: u64,
    submission_id_full: u64,
    skipped_dependencies: u64,
    fills: u64,
    order_deltas: u64,
    level_deltas: u64,
    zero_fills: u64,
    single_fills: u64,
    multiple_fills: u64,
    full_sweeps: u64,
    zero_level_changes: u64,
    one_level_change: u64,
    multiple_level_changes: u64,
    peak_queue_depth: usize,
    peak_response_credits_in_use: usize,
    latency: Latencies,
}

impl Stats {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            attempted: 0,
            admitted: 0,
            admission_rejected: 0,
            matching_rejected: 0,
            queue_full: 0,
            response_pool_full: 0,
            submission_id_full: 0,
            skipped_dependencies: 0,
            fills: 0,
            order_deltas: 0,
            level_deltas: 0,
            zero_fills: 0,
            single_fills: 0,
            multiple_fills: 0,
            full_sweeps: 0,
            zero_level_changes: 0,
            one_level_change: 0,
            multiple_level_changes: 0,
            peak_queue_depth: 0,
            peak_response_credits_in_use: 0,
            latency: Latencies::new(),
        }
    }

    fn reserve_latency(&mut self, samples: usize) {
        self.latency.scheduled_to_delivery.reserve(samples);
        self.latency.queue_wait.reserve(samples);
        self.latency.execution.reserve(samples);
        self.latency.delivery_wait.reserve(samples);
        self.latency.scheduled_to_start.reserve(samples);
    }

    fn observe_occupancy(&mut self, queue_depth: usize, response_credits_in_use: usize) {
        self.peak_queue_depth = self.peak_queue_depth.max(queue_depth);
        self.peak_response_credits_in_use = self
            .peak_response_credits_in_use
            .max(response_credits_in_use);
    }

    fn observe(&mut self, completed: &CompletedResponse) {
        if completed.outcome().is_err() {
            self.matching_rejected += 1;
        } else {
            self.latency.push(completed.timings());
        }
        let response = completed.response();
        self.fills += response.fills.len() as u64;
        self.order_deltas += response.order_deltas.len() as u64;
        self.level_deltas += response.level_deltas.len() as u64;
        match response.fills.len() {
            0 => self.zero_fills += 1,
            1 => self.single_fills += 1,
            _ => self.multiple_fills += 1,
        }
        if response.fills.len() == MAX_LIVE_ORDERS {
            self.full_sweeps += 1;
        }
        match response.level_deltas.len() {
            0 => self.zero_level_changes += 1,
            1 => self.one_level_change += 1,
            _ => self.multiple_level_changes += 1,
        }
    }

    fn merge(&mut self, mut other: Self) {
        self.attempted += other.attempted;
        self.admitted += other.admitted;
        self.admission_rejected += other.admission_rejected;
        self.matching_rejected += other.matching_rejected;
        self.queue_full += other.queue_full;
        self.response_pool_full += other.response_pool_full;
        self.submission_id_full += other.submission_id_full;
        self.skipped_dependencies += other.skipped_dependencies;
        self.fills += other.fills;
        self.order_deltas += other.order_deltas;
        self.level_deltas += other.level_deltas;
        self.zero_fills += other.zero_fills;
        self.single_fills += other.single_fills;
        self.multiple_fills += other.multiple_fills;
        self.full_sweeps += other.full_sweeps;
        self.zero_level_changes += other.zero_level_changes;
        self.one_level_change += other.one_level_change;
        self.multiple_level_changes += other.multiple_level_changes;
        self.peak_queue_depth = self.peak_queue_depth.max(other.peak_queue_depth);
        self.peak_response_credits_in_use = self
            .peak_response_credits_in_use
            .max(other.peak_response_credits_in_use);
        self.latency.merge(&mut other.latency);
    }
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn engine(id: u64, config: EngineConfig) -> MatchingEngine {
    MatchingEngine::new(
        EngineId(id),
        InstrumentSpec {
            symbol: "LATENCY".to_owned(),
            tick_size: DecimalScale {
                mantissa: 1,
                scale: 2,
            },
            lot_size: DecimalScale {
                mantissa: 1,
                scale: 3,
            },
            min_size: Quantity(1),
            max_size: Some(Quantity(1_000_000)),
        },
        config,
    )
    .expect("valid benchmark engine configuration")
}

fn adapter(
    engine: MatchingEngine,
    command_capacity: usize,
    response_capacity: usize,
) -> ExecutionAdapter {
    ExecutionAdapter::new(
        engine,
        ExecutionConfig {
            command_capacity,
            response_capacity,
        },
    )
    .expect("bounded benchmark storage is available")
}

fn book_config() -> EngineConfig {
    EngineConfig {
        max_orders_per_level: PER_LEVEL,
        max_levels: LEVELS,
    }
}

fn response_for(config: &EngineConfig) -> MatchingResponse {
    MatchingResponse::with_capacity(config).expect("response fits configured book")
}

fn execute(
    engine: &mut MatchingEngine,
    command: Command,
    response: &mut MatchingResponse,
) -> Result<(), MatchingError> {
    engine.execute_into(&command, response)
}

fn add_command(account: u64, client: u64, side: Side, price: u64, size: u64) -> Command {
    Command::Add {
        account_id: AccountId(account),
        client_id: ClientOrderId(client),
        side,
        price: Price(price),
        size: Quantity(size),
    }
}

fn render_json(
    reports: &[Report],
    samples: usize,
    sweep_samples: usize,
    rate: u64,
    interval_ns: u64,
) -> String {
    let mode = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let cpu_model = cpu_model().map_or_else(|| "null".to_owned(), |value| json_string(&value));
    let rustc_version =
        rustc_version().map_or_else(|| "null".to_owned(), |value| json_string(&value));
    let logical_cpus = std::thread::available_parallelism().map_or(0, usize::from);
    let effective_rate = 1_000_000_000.0 / interval_ns as f64;
    let mut out = String::new();
    writeln!(&mut out, "{{").unwrap();
    writeln!(&mut out, "  \"benchmark\": \"cxchange-latency\",").unwrap();
    writeln!(
        &mut out,
        "  \"version\": \"{}\",",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(&mut out, "  \"build_mode\": \"{mode}\",").unwrap();
    writeln!(
        &mut out,
        "  \"platform\": {{\"os\": \"{}\", \"arch\": \"{}\"}},",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"environment\": {{\"cpu_model\": {cpu_model}, \"logical_cpus\": {logical_cpus}, \"rustc\": {rustc_version}}},"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"schedule\": {{\"requested_primary_samples_per_class\": {samples}, \"full_sweep_requested_samples\": {sweep_samples}, \"profile_sample_cap\": {PROFILE_SAMPLES}, \"requested_arrival_rate_hz\": {rate}, \"effective_arrival_rate_hz\": {effective_rate:.3}, \"arrival_interval_ns\": {interval_ns}, \"mode\": \"open_loop_scheduled_offsets\"}},"
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"scheduler_note\": \"ordinary scenarios use one thread to catch up scheduled arrivals while preserving original timestamps; the mixed sweep scenario blocks the producer during the full-sweep call, then submits due arrivals; queue_wait begins at adapter admission while scheduled_to_start and scheduled_to_delivery include producer/admission lag; no concurrent producer is modeled\","
    )
    .unwrap();
    writeln!(
        &mut out,
        "  \"profile_notes\": {{\"primary_target\": \"only retained-anchor Add/Cancel and retained-anchor Amend use the requested primary sample count; all other profiles are exploratory\", \"near_full_and_fifo_profiles\": \"seed the default 128 orders per level by 1024 levels book (131072 live orders) and cap ordinary profile arrivals at 10000\", \"mixed_sweep\": \"same-time arrivals scheduled at sweep start, caught up after sweep completion; small response pool intentionally shows admission pressure\", \"held_credit_pressure\": \"one-time saturation check, not sustained slow-consumer latency or throughput evidence\", \"tail_percentiles\": \"p99.9 is omitted for profiles with fewer than 1000 valid completed samples and for explicitly non-tail profiles; matching rejections are counted separately and excluded from valid-operation latency\"}},"
    )
    .unwrap();
    writeln!(&mut out, "  \"reports\": [").unwrap();
    for (index, report) in reports.iter().enumerate() {
        let peak_queue = report
            .metrics
            .iter()
            .map(|metric| metric.peak_queue_depth)
            .max()
            .unwrap_or(0);
        let peak_credits = report
            .metrics
            .iter()
            .map(|metric| metric.peak_response_credits_in_use)
            .max()
            .unwrap_or(0);
        writeln!(
            &mut out,
            "    {{\"name\": \"{}\", \"sample_intent\": \"{}\", \"engine_config\": {{\"max_orders_per_level\": {}, \"max_levels\": {}, \"max_live_orders\": {}}}, \"command_capacity\": {}, \"response_capacity\": {}, \"requested_commands_per_class\": {}, \"tail_percentiles_enabled\": {}, \"adapter_occupancy\": {{\"queue_depth_start\": 0, \"queue_depth_peak\": {}, \"queue_depth_end\": 0, \"response_credits_in_use_start\": 0, \"response_credits_in_use_peak\": {}, \"response_credits_in_use_end\": 0}}, \"metrics\": [",
            report.name,
            report.sample_intent,
            report.engine_config.max_orders_per_level,
            report.engine_config.max_levels,
            report.engine_config.validate().unwrap_or(0),
            report.command_capacity,
            report.response_capacity,
            report
                .requested_commands_per_class
                .map_or_else(|| "null".to_owned(), |value| value.to_string()),
            report.report_tail,
            peak_queue,
            peak_credits,
        )
        .unwrap();
        for (metric_index, metric) in report.metrics.iter().enumerate() {
            write_stats(&mut out, metric, report.report_tail);
            writeln!(
                &mut out,
                "{}",
                if metric_index + 1 == report.metrics.len() {
                    ""
                } else {
                    ","
                }
            )
            .unwrap();
        }
        writeln!(
            &mut out,
            "    ]}}{}",
            if index + 1 == reports.len() { "" } else { "," }
        )
        .unwrap();
    }
    writeln!(&mut out, "  ]").unwrap();
    writeln!(&mut out, "}}").unwrap();
    out
}

fn write_stats(out: &mut String, stats: &Stats, report_tail: bool) {
    writeln!(
        out,
        "      {{\"class\": \"{}\", \"attempted\": {}, \"admitted\": {}, \"admission_rejected\": {}, \"matching_rejected\": {}, \"skipped_dependencies\": {}, \"rejection_causes\": {{\"command_queue_full\": {}, \"response_pool_exhausted\": {}, \"submission_id_exhausted\": {}}}, \"adapter_occupancy_peak\": {{\"queue_depth\": {}, \"response_credits_in_use\": {}}},",
        stats.name,
        stats.attempted,
        stats.admitted,
        stats.admission_rejected,
        stats.matching_rejected,
        stats.skipped_dependencies,
        stats.queue_full,
        stats.response_pool_full,
        stats.submission_id_full,
        stats.peak_queue_depth,
        stats.peak_response_credits_in_use
    )
    .unwrap();
    writeln!(
        out,
        "       \"classification\": {{\"fills_total\": {}, \"order_deltas_total\": {}, \"level_deltas_total\": {}, \"zero_fill_commands\": {}, \"single_fill_commands\": {}, \"multi_fill_commands\": {}, \"full_sweep_commands\": {}, \"zero_level_change_commands\": {}, \"one_level_change_commands\": {}, \"multiple_level_change_commands\": {}}},",
        stats.fills,
        stats.order_deltas,
        stats.level_deltas,
        stats.zero_fills,
        stats.single_fills,
        stats.multiple_fills,
        stats.full_sweeps,
        stats.zero_level_changes,
        stats.one_level_change,
        stats.multiple_level_changes
    )
    .unwrap();
    writeln!(out, "       \"valid_operation_latency_ns\": {{").unwrap();
    write_latency_components(out, &stats.latency, report_tail);
    write!(out, "       }}}}").unwrap();
}

fn write_latency_components(out: &mut String, latency: &Latencies, report_tail: bool) {
    let entries = [
        ("scheduled_to_delivery", &latency.scheduled_to_delivery),
        ("queue_wait", &latency.queue_wait),
        ("execution", &latency.execution),
        ("delivery_wait", &latency.delivery_wait),
        ("scheduled_to_start", &latency.scheduled_to_start),
    ];
    for (index, (name, values)) in entries.iter().enumerate() {
        write!(out, "         \"{name}\": ").unwrap();
        write_percentiles(out, values, report_tail);
        writeln!(out, "{}", if index + 1 == entries.len() { "" } else { "," }).unwrap();
    }
}

fn write_percentiles(out: &mut String, values: &[u64], report_tail: bool) {
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let p50 = percentile(&sorted, 50, 100);
    let p95 = percentile(&sorted, 95, 100);
    let p99 = percentile(&sorted, 99, 100);
    let p99_9 = if report_tail && sorted.len() >= 1_000 {
        percentile(&sorted, 999, 1_000)
    } else {
        None
    };
    let ratio = p99_9
        .zip(p50)
        .and_then(|(tail, median)| (median > 0).then_some(tail as f64 / median as f64));
    write!(
        out,
        "{{\"samples\": {}, \"p50\": {}, \"p95\": {}, \"p99\": {}, \"p99_9\": {}, \"p99_9_over_p50\": {}}}",
        sorted.len(),
        optional_u64(p50),
        optional_u64(p95),
        optional_u64(p99),
        optional_u64(p99_9),
        ratio.map_or_else(|| "null".to_owned(), |value| format!("{value:.4}"))
    )
    .unwrap();
}

fn percentile(sorted: &[u64], numerator: usize, denominator: usize) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = numerator
        .saturating_mul(sorted.len())
        .div_ceil(denominator)
        .max(1);
    sorted.get(rank - 1).copied()
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |number| number.to_string())
}

fn rustc_version() -> Option<String> {
    let output = ProcessCommand::new("rustc")
        .arg("--version")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn cpu_model() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = ProcessCommand::new("sysctl")
            .args(["-n", "machdep.cpu.brand_string"])
            .output()
            .ok()?;
        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let data = fs::read_to_string("/proc/cpuinfo").ok()?;
        data.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            matches!(name.trim(), "model name" | "Hardware").then(|| value.trim().to_owned())
        })
    }
    #[cfg(not(target_os = "linux"))]
    None
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character.is_control() => {
                write!(out, "\\u{:04x}", character as u32).unwrap();
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        let values = (1_u64..=1000).collect::<Vec<_>>();
        assert_eq!(percentile(&values, 50, 100), Some(500));
        assert_eq!(percentile(&values, 95, 100), Some(950));
        assert_eq!(percentile(&values, 999, 1_000), Some(999));
        assert_eq!(percentile(&[1, 2, 3], 50, 100), Some(2));
    }

    #[test]
    fn tail_percentile_is_null_when_too_small_or_median_is_zero() {
        let mut too_small = String::new();
        write_percentiles(&mut too_small, &[1, 2, 3], true);
        assert!(too_small.contains("\"p99_9\": null"));
        assert!(too_small.contains("\"p99_9_over_p50\": null"));

        let zeroes = vec![0; 1000];
        let mut zero_median = String::new();
        write_percentiles(&mut zero_median, &zeroes, true);
        assert!(zero_median.contains("\"p50\": 0"));
        assert!(zero_median.contains("\"p99_9\": 0"));
        assert!(zero_median.contains("\"p99_9_over_p50\": null"));
    }

    #[test]
    fn matching_rejections_are_excluded_from_valid_operation_latency() {
        let mut adapter = adapter(engine(90, book_config()), 1, 1);
        let id = adapter
            .try_submit(add_command(10, 1, Side::Buy, 0, 1))
            .expect("invalid trading command is admitted");
        assert_eq!(adapter.process_next(), Some(id));
        let completed = adapter.take_completed().expect("matching result is ready");
        assert_eq!(completed.outcome(), Err(MatchingError::InvalidPrice));

        let mut stats = Stats::new("invalid_price");
        stats.observe(&completed);
        assert_eq!(stats.matching_rejected, 1);
        assert!(stats.latency.execution.is_empty());
    }
}
