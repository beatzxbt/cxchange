//! End-to-end snapshot continuation, JSON replay, and execution-pool audit tests.
//!
//! A fixed mixed trace supplies results and snapshots at every command boundary.
//! Each restored continuation must reproduce the uninterrupted execution; CLI
//! checks cross the serialized process boundary, and adapter checks retain
//! response ownership under pressure. These are behavioral integration checks,
//! separate from the flat-vector oracle in `audit_matching.rs`.

use std::io::Write;
use std::process::{Command as ProcessCommand, Stdio};

use matching_core::execution::{AdmissionError, ExecutionAdapter, ExecutionConfig};
use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, EngineSnapshot,
    InstrumentSpec, MatchingEngine, MatchingError, MatchingResponse, OrderId, Price, Quantity,
    Side,
};
use serde::Deserialize;
use serde_json::json;

const ENGINE_ID: EngineId = EngineId(217);

fn spec() -> InstrumentSpec {
    InstrumentSpec {
        symbol: "INTEGRATION".into(),
        tick_size: DecimalScale::new(1, 2).unwrap(),
        lot_size: DecimalScale::new(1, 3).unwrap(),
        min_size: Quantity(1),
        max_size: None,
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        max_orders_per_level: 8,
        max_levels: 8,
    }
}

fn engine() -> MatchingEngine {
    MatchingEngine::new(ENGINE_ID, spec(), config()).unwrap()
}

fn id(sequence: u64) -> OrderId {
    OrderId::new(ENGINE_ID, sequence)
}

fn add(account: u64, client: u64, side: Side, price: u64, size: u64) -> Command {
    Command::Add {
        account_id: AccountId(account),
        client_id: ClientOrderId(client),
        side,
        price: Price(price),
        size: Quantity(size),
    }
}

fn take(account: u64, client: u64, side: Side, price: u64, size: u64) -> Command {
    Command::Take {
        account_id: AccountId(account),
        client_id: ClientOrderId(client),
        side,
        limit_price: Price(price),
        size: Quantity(size),
    }
}

fn cancel(account: u64, sequence: u64) -> Command {
    Command::Cancel {
        account_id: AccountId(account),
        order_id: id(sequence),
    }
}

fn amend(account: u64, sequence: u64, price: u64, size: u64) -> Command {
    Command::Amend {
        account_id: AccountId(account),
        order_id: id(sequence),
        price: Price(price),
        size: Quantity(size),
    }
}

/// Mixes successful transitions with duplicate, self-trade, crossing, stale-ID,
/// wrong-owner, and no-liquidity rejections for recovery-boundary comparisons.
fn mixed_trace() -> Vec<Command> {
    vec![
        add(1, 10, Side::Sell, 101, 10),
        add(2, 10, Side::Sell, 101, 7),
        add(3, 30, Side::Sell, 102, 5),
        take(4, 40, Side::Buy, 101, 6),
        add(1, 10, Side::Buy, 99, 1),
        take(1, 50, Side::Buy, 102, 10),
        amend(1, 1, 101, 8),
        take(4, 41, Side::Buy, 101, 9),
        cancel(1, 5),
        add(1, 10, Side::Buy, 100, 3),
        add(5, 55, Side::Sell, 100, 2),
        cancel(2, 2),
        add(6, 60, Side::Sell, 103, 4),
        take(7, 70, Side::Buy, 103, 9),
        amend(1, 7, 100, 5),
        cancel(8, 10),
        take(1, 90, Side::Sell, 100, 2),
        add(8, 80, Side::Buy, 100, 4),
        take(9, 90, Side::Sell, 100, 6),
        cancel(8, 11),
        take(9, 91, Side::Sell, 100, 1),
        add(10, 100, Side::Sell, 120, 2),
    ]
}

/// Copies a successful reusable response so later commands cannot overwrite expectations.
fn execute(
    engine: &mut MatchingEngine,
    response: &mut MatchingResponse,
    command: &Command,
) -> Result<MatchingResponse, MatchingError> {
    engine
        .execute_into(command, response)
        .map(|()| response.clone())
}

/// Records uninterrupted outcomes and `N + 1` states, including the initial book.
///
/// `snapshots[i]` is the state before command `i`, or the final state at `N`.
fn baseline(
    trace: &[Command],
) -> (
    Vec<Result<MatchingResponse, MatchingError>>,
    Vec<EngineSnapshot>,
) {
    let mut engine = engine();
    let mut buffer = MatchingResponse::with_capacity(&config()).unwrap();
    let mut results = Vec::with_capacity(trace.len());
    let mut snapshots = vec![engine.export_snapshot()];
    for command in trace {
        results.push(execute(&mut engine, &mut buffer, command));
        engine.validate_invariants().unwrap();
        snapshots.push(engine.export_snapshot());
    }
    (results, snapshots)
}

#[test]
fn serialized_restore_at_every_boundary_preserves_all_following_outcomes_and_state() {
    let trace = mixed_trace();
    let (expected, snapshots) = baseline(&trace);
    assert_eq!(expected[4], Err(MatchingError::DuplicateClientId));
    assert_eq!(expected[5], Err(MatchingError::SelfTrade));
    assert_eq!(expected[10], Err(MatchingError::WouldCross));
    assert_eq!(expected[11], Err(MatchingError::OrderNotFound));
    assert_eq!(expected[15], Err(MatchingError::WrongOwner));
    assert_eq!(expected[16], Err(MatchingError::SelfTrade));
    assert_eq!(expected[20], Err(MatchingError::NoLiquidity));
    assert_eq!(snapshots[4], snapshots[5]);
    assert_eq!(snapshots[5], snapshots[6]);
    assert_eq!(snapshots[10], snapshots[11]);
    assert_eq!(snapshots[11], snapshots[12]);
    assert_eq!(snapshots[15], snapshots[16]);
    assert_eq!(snapshots[16], snapshots[17]);
    assert_eq!(snapshots[20], snapshots[21]);

    let maker_ids: Vec<_> = expected[7]
        .as_ref()
        .unwrap()
        .fills
        .iter()
        .map(|fill| fill.maker_order_id)
        .collect();
    assert_eq!(maker_ids, vec![id(2), id(5)]);
    assert_eq!(snapshots[10].bids[0].orders[0].client_id, ClientOrderId(10));
    assert_eq!(snapshots[10].bids[0].orders[0].order_id, id(7));

    for boundary in 0..=trace.len() {
        let encoded = serde_json::to_vec(&snapshots[boundary]).unwrap();
        let decoded: EngineSnapshot = serde_json::from_slice(&encoded).unwrap();
        let mut restored = MatchingEngine::from_snapshot(decoded).unwrap();
        let mut buffer = MatchingResponse::with_capacity(&config()).unwrap();
        assert_eq!(
            restored.export_snapshot(),
            snapshots[boundary],
            "boundary {boundary}"
        );
        for index in boundary..trace.len() {
            let actual = execute(&mut restored, &mut buffer, &trace[index]);
            assert_eq!(
                actual, expected[index],
                "boundary {boundary}, command {index}"
            );
            assert_eq!(
                restored.export_snapshot(),
                snapshots[index + 1],
                "boundary {boundary}, command {index}"
            );
            restored.validate_invariants().unwrap();
        }
    }
}

#[test]
fn restore_rejects_individually_corrupt_identity_priority_fifo_and_counters() {
    let mut source = engine();
    let mut output = MatchingResponse::with_capacity(&config()).unwrap();
    source
        .execute_into(&add(1, 1, Side::Buy, 100, 1), &mut output)
        .unwrap();
    source
        .execute_into(&add(2, 2, Side::Buy, 100, 1), &mut output)
        .unwrap();
    source.execute_into(&cancel(2, 2), &mut output).unwrap();
    let valid = source.export_snapshot();
    assert_eq!(
        MatchingEngine::from_snapshot(valid.clone())
            .unwrap()
            .export_snapshot(),
        valid
    );

    let mut wrong_engine = valid.clone();
    wrong_engine.bids[0].orders[0].order_id.engine_id = EngineId(999);
    let mut wrong_order_id = valid.clone();
    wrong_order_id.bids[0].orders[0].order_id = id(2);
    let mut wrong_priority = valid.clone();
    wrong_priority.bids[0].orders[0].sequence = 1;
    let mut wrong_counters = valid.clone();
    wrong_counters.next_order_id += 1;
    for corrupt in [wrong_engine, wrong_order_id, wrong_priority, wrong_counters] {
        assert_eq!(
            MatchingEngine::from_snapshot(corrupt).err(),
            Some(MatchingError::InvalidSnapshot)
        );
    }

    let mut fifo_source = engine();
    fifo_source
        .execute_into(&add(1, 1, Side::Buy, 100, 1), &mut output)
        .unwrap();
    fifo_source
        .execute_into(&add(2, 2, Side::Buy, 100, 1), &mut output)
        .unwrap();
    let mut bad_fifo = fifo_source.export_snapshot();
    bad_fifo.bids[0].orders.swap(0, 1);
    assert_eq!(
        MatchingEngine::from_snapshot(bad_fifo).err(),
        Some(MatchingError::InvalidSnapshot)
    );
}

/// Decodes the CLI's mutually exclusive response/error fields for comparison.
#[derive(Clone, Debug, Deserialize, PartialEq)]
struct ReplayOutcome {
    response: Option<MatchingResponse>,
    error: Option<String>,
}

/// Typed CLI output preserves wide aggregates without a generic JSON value conversion.
#[derive(Debug, Deserialize, PartialEq)]
struct ReplayOutput {
    results: Vec<ReplayOutcome>,
    snapshot: EngineSnapshot,
}

/// Runs the actual replay binary via stdin and decodes its complete output.
fn replay(
    trace: &[Command],
    roundtrip_after: Option<usize>,
    instrument: &InstrumentSpec,
) -> ReplayOutput {
    let input = serde_json::to_vec(&json!({
        "engine_id": ENGINE_ID,
        "spec": instrument,
        "config": config(),
        "commands": trace,
        "roundtrip_after": roundtrip_after,
    }))
    .unwrap();
    let mut child = ProcessCommand::new(env!("CARGO_BIN_EXE_cxchange-replay"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&input).unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "replay failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn stdin_replay_zero_middle_and_end_restore_match_library_outcomes() {
    let trace = mixed_trace();
    let (expected, snapshots) = baseline(&trace);
    let expected_outcomes: Vec<_> = expected
        .into_iter()
        .map(|result| match result {
            Ok(response) => ReplayOutcome {
                response: Some(response),
                error: None,
            },
            Err(error) => ReplayOutcome {
                response: None,
                error: Some(error.code().into()),
            },
        })
        .collect();
    for boundary in [None, Some(0), Some(trace.len() / 2), Some(trace.len())] {
        assert_eq!(
            replay(&trace, boundary, &spec()),
            ReplayOutput {
                results: expected_outcomes.clone(),
                snapshot: snapshots.last().unwrap().clone(),
            },
            "CLI boundary {boundary:?}"
        );
    }
}

#[test]
fn stdin_replay_emits_exact_aggregate_above_u64_max() {
    let trace = vec![
        add(1, 1, Side::Sell, 100, u64::MAX),
        add(2, 2, Side::Sell, 100, u64::MAX),
    ];
    let output = replay(&trace, Some(1), &spec());
    assert_eq!(output.results.len(), 2);
    assert_eq!(
        output.results[1].response.as_ref().unwrap().level_deltas[0].size,
        u128::from(u64::MAX) * 2
    );
    assert_eq!(output.snapshot.asks[0].orders.len(), 2);
}

#[test]
fn adapter_pool_pressure_preserves_admitted_results_and_full_book_sweep() {
    let book_config = EngineConfig {
        max_orders_per_level: 128,
        max_levels: 2,
    };
    let mut direct = MatchingEngine::new(ENGINE_ID, spec(), book_config).unwrap();
    let mut adapter = ExecutionAdapter::new(
        MatchingEngine::new(ENGINE_ID, spec(), book_config).unwrap(),
        ExecutionConfig {
            command_capacity: 2,
            response_capacity: 2,
        },
    )
    .unwrap();
    let mut buffer = MatchingResponse::with_capacity(&book_config).unwrap();
    for index in 0..256_u64 {
        let command = add(index + 1, index + 1, Side::Sell, 100 + index / 128, 1);
        let expected = execute(&mut direct, &mut buffer, &command);
        let admitted = adapter.try_submit(command).unwrap();
        assert_eq!(admitted.0, index + 1);
        assert_eq!(adapter.process_next(), Some(admitted));
        let completed = adapter.take_completed().unwrap();
        assert_eq!(completed.submission_id(), admitted);
        assert_eq!(
            completed.outcome(),
            expected.as_ref().map(|_| ()).map_err(|error| *error)
        );
        assert_eq!(completed.response(), expected.as_ref().unwrap());
        adapter.recycle(completed).unwrap();
    }
    assert_eq!(adapter.engine().export_snapshot(), direct.export_snapshot());

    let sweep = take(999, 999, Side::Buy, 101, 256);
    let expected = execute(&mut direct, &mut buffer, &sweep).unwrap();
    let sweep_id = adapter.try_submit(sweep).unwrap();
    let held = adapter.take_completed();
    assert!(held.is_none());
    let blocked = add(1000, 1000, Side::Buy, 90, 1);
    let second_id = adapter.try_submit(blocked).unwrap();
    assert_eq!(
        adapter.try_submit(add(1001, 1001, Side::Buy, 89, 1)),
        Err(AdmissionError::CommandQueueFull)
    );
    assert_eq!(adapter.process_next(), Some(sweep_id));
    let completed = adapter.take_completed().unwrap();
    assert_eq!(completed.outcome(), Ok(()));
    assert_eq!(completed.response(), &expected);
    assert_eq!(completed.response().fills.len(), 256);
    assert_eq!(completed.response().order_deltas.len(), 257);
    assert_eq!(completed.response().level_deltas.len(), 2);
    assert_eq!(adapter.available_response_credits(), 0);
    assert_eq!(
        adapter.try_submit(add(1001, 1001, Side::Buy, 89, 1)),
        Err(AdmissionError::ResponsePoolExhausted)
    );
    let second_expected = execute(&mut direct, &mut buffer, &blocked).unwrap();
    assert_eq!(adapter.process_next(), Some(second_id));
    let second = adapter.take_completed().unwrap();
    assert_eq!(second.outcome(), Ok(()));
    assert_eq!(second.response(), &second_expected);
    adapter.recycle(second).unwrap();
    adapter.recycle(completed).unwrap();
    let next_command = add(1001, 1001, Side::Buy, 89, 1);
    let next_expected = execute(&mut direct, &mut buffer, &next_command).unwrap();
    let next = adapter.try_submit(next_command).unwrap();
    assert_eq!(next.0, second_id.0 + 1);
    adapter.process_next();
    let next_result = adapter.take_completed().unwrap();
    assert_eq!(next_result.outcome(), Ok(()));
    assert_eq!(next_result.response(), &next_expected);
    adapter.recycle(next_result).unwrap();
    assert_eq!(adapter.engine().export_snapshot(), direct.export_snapshot());
}

#[test]
fn foreign_response_cannot_be_recycled_and_dropped_response_keeps_credit_leased() {
    let adapter_config = ExecutionConfig {
        command_capacity: 1,
        response_capacity: 1,
    };
    let mut first = ExecutionAdapter::new(engine(), adapter_config).unwrap();
    let mut second = ExecutionAdapter::new(engine(), adapter_config).unwrap();
    let submission = first.try_submit(add(1, 1, Side::Buy, 100, 1)).unwrap();
    assert_eq!(first.process_next(), Some(submission));
    let foreign = first.take_completed().unwrap();
    let foreign = second.recycle(foreign).unwrap_err();
    assert_eq!(first.available_response_credits(), 0);
    assert_eq!(second.available_response_credits(), 1);
    first.recycle(foreign).unwrap();
    assert_eq!(first.available_response_credits(), 1);

    let submission = first.try_submit(add(2, 2, Side::Buy, 100, 1)).unwrap();
    assert_eq!(first.process_next(), Some(submission));
    drop(first.take_completed().unwrap());
    assert_eq!(first.available_response_credits(), 0);
    assert_eq!(
        first.try_submit(add(3, 3, Side::Buy, 100, 1)),
        Err(AdmissionError::ResponsePoolExhausted)
    );
}
