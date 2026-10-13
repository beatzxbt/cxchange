//! Tests for bounded command admission, ordered execution, and response credit reuse.

use matching_core::execution::{
    AdmissionError, ExecutionAdapter, ExecutionConfig, ExecutionInitError, SubmissionId,
};
use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MatchingEngine, MatchingError, OrderId, Price, Quantity, Side,
};

fn spec() -> InstrumentSpec {
    InstrumentSpec {
        symbol: "BTC-USD".to_owned(),
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
    }
}

fn adapter(
    command_capacity: usize,
    response_capacity: usize,
    max_orders_per_level: usize,
    max_levels: usize,
) -> ExecutionAdapter {
    let engine = MatchingEngine::new(
        EngineId(1),
        spec(),
        EngineConfig {
            max_orders_per_level,
            max_levels,
        },
    )
    .expect("valid engine");
    ExecutionAdapter::new(
        engine,
        ExecutionConfig {
            command_capacity,
            response_capacity,
        },
    )
    .expect("valid execution adapter")
}

fn add(client_id: u64, price: u64) -> Command {
    Command::Add {
        account_id: AccountId(10),
        client_id: ClientOrderId(client_id),
        side: Side::Buy,
        price: Price(price),
        size: Quantity(1),
    }
}

#[test]
fn zero_execution_capacity_is_rejected_at_startup() {
    let engine = MatchingEngine::new(
        EngineId(1),
        spec(),
        EngineConfig {
            max_orders_per_level: 2,
            max_levels: 2,
        },
    )
    .expect("valid engine");

    assert_eq!(
        ExecutionAdapter::new(
            engine,
            ExecutionConfig {
                command_capacity: 0,
                response_capacity: 1,
            }
        )
        .err(),
        Some(ExecutionInitError::InvalidConfig)
    );
}

#[test]
fn command_queue_admission_is_bounded_and_failed_admission_does_not_consume_submission_id() {
    let mut adapter = adapter(1, 2, 4, 2);
    let first = adapter
        .try_submit(add(1, 100))
        .expect("first command admitted");
    assert_eq!(first, SubmissionId(1));
    assert_eq!(adapter.queued_len(), 1);
    assert_eq!(
        adapter.try_submit(add(2, 99)),
        Err(AdmissionError::CommandQueueFull)
    );
    assert_eq!(adapter.queued_len(), 1);
    assert_eq!(adapter.engine().best_bid(), None);
    assert_eq!(adapter.engine().validate_invariants(), Ok(()));

    assert_eq!(adapter.process_next(), Some(first));
    let completed = adapter.take_completed().expect("first result is ready");
    assert_eq!(completed.submission_id(), first);
    assert_eq!(completed.outcome(), Ok(()));
    assert_eq!(completed.response().order_deltas[0].order_id.sequence, 1);
    adapter.recycle(completed).expect("return response credit");

    let second = adapter
        .try_submit(add(2, 99))
        .expect("command admitted after queue space is available");
    assert_eq!(second, SubmissionId(2));
    adapter.process_next();
    let completed = adapter.take_completed().expect("second result is ready");
    assert_eq!(completed.response().order_deltas[0].order_id.sequence, 2);
    adapter.recycle(completed).expect("return response credit");
    assert_eq!(adapter.engine().validate_invariants(), Ok(()));
}

#[test]
fn outstanding_output_pressure_rejects_admission_until_response_credit_is_recycled() {
    let mut adapter = adapter(4, 1, 4, 2);
    let first = adapter
        .try_submit(add(1, 100))
        .expect("first command admitted");
    adapter.process_next();
    assert_eq!(adapter.available_response_credits(), 0);
    assert_eq!(
        adapter.try_submit(add(2, 99)),
        Err(AdmissionError::ResponsePoolExhausted)
    );
    assert_eq!(adapter.queued_len(), 0);

    let completed = adapter.take_completed().expect("result is ready");
    assert_eq!(completed.submission_id(), first);
    assert_eq!(adapter.available_response_credits(), 0);
    adapter.recycle(completed).expect("return response credit");
    assert_eq!(adapter.available_response_credits(), 1);
    assert_eq!(adapter.try_submit(add(2, 99)), Ok(SubmissionId(2)));
}

#[test]
fn admitted_commands_complete_once_in_fifo_order_even_when_matching_rejects() {
    let mut adapter = adapter(4, 4, 4, 2);
    let first = adapter
        .try_submit(add(1, 0))
        .expect("invalid trading command is still admitted");
    let second = adapter
        .try_submit(add(2, 100))
        .expect("valid command admitted");
    let third = adapter
        .try_submit(add(3, 99))
        .expect("second valid command admitted");
    assert_eq!(
        (first, second, third),
        (SubmissionId(1), SubmissionId(2), SubmissionId(3))
    );

    assert_eq!(adapter.process_next(), Some(first));
    assert_eq!(adapter.process_next(), Some(second));
    assert_eq!(adapter.process_next(), Some(third));
    assert_eq!(adapter.process_next(), None);
    assert_eq!(adapter.completed_len(), 3);

    let rejected = adapter.take_completed().expect("first completion is ready");
    assert_eq!(rejected.submission_id(), first);
    assert_eq!(rejected.outcome(), Err(MatchingError::InvalidPrice));
    assert!(rejected.response().fills.is_empty());
    adapter.recycle(rejected).expect("return response credit");

    let accepted = adapter
        .take_completed()
        .expect("second completion is ready");
    assert_eq!(accepted.submission_id(), second);
    assert_eq!(accepted.outcome(), Ok(()));
    assert_eq!(accepted.response().order_deltas[0].order_id.sequence, 1);
    adapter.recycle(accepted).expect("return response credit");

    let accepted = adapter.take_completed().expect("third completion is ready");
    assert_eq!(accepted.submission_id(), third);
    assert_eq!(accepted.outcome(), Ok(()));
    assert_eq!(accepted.response().order_deltas[0].order_id.sequence, 2);
    adapter.recycle(accepted).expect("return response credit");
    assert_eq!(adapter.engine().validate_invariants(), Ok(()));
}

#[test]
fn matching_rejection_does_not_consume_engine_order_identity() {
    let mut adapter = adapter(2, 2, 4, 2);
    let rejected = adapter
        .try_submit(add(1, 0))
        .expect("trading rejection is not an admission rejection");
    assert_eq!(rejected, SubmissionId(1));
    assert_eq!(adapter.process_next(), Some(rejected));
    let rejected = adapter.take_completed().expect("rejection result is ready");
    assert_eq!(rejected.submission_id(), SubmissionId(1));
    assert_eq!(rejected.outcome(), Err(MatchingError::InvalidPrice));
    adapter.recycle(rejected).expect("return response credit");

    let valid = adapter
        .try_submit(add(2, 100))
        .expect("valid order admitted");
    assert_eq!(valid, SubmissionId(2));
    assert_eq!(adapter.process_next(), Some(valid));
    let completed = adapter.take_completed().expect("order result is ready");
    assert_eq!(
        completed.response().order_deltas[0].order_id,
        OrderId::new(EngineId(1), 1)
    );
    adapter.recycle(completed).expect("return response credit");
    assert_eq!(adapter.engine().validate_invariants(), Ok(()));
}

#[test]
fn recycling_restores_the_same_preallocated_response_capacity() {
    let mut adapter = adapter(2, 1, 8, 3);
    for (client_id, price) in [(1, 100), (2, 99)] {
        adapter
            .try_submit(add(client_id, price))
            .expect("command admitted");
        adapter.process_next();
        let completed = adapter.take_completed().expect("result is ready");
        let capacities = (
            completed.response().fills.capacity(),
            completed.response().order_deltas.capacity(),
            completed.response().level_deltas.capacity(),
        );
        assert!(capacities.0 >= 8);
        assert!(capacities.1 >= 9);
        assert!(capacities.2 >= 3);
        adapter
            .recycle(completed)
            .expect("response credit recycled");
    }
}

#[test]
fn a_response_credit_can_only_be_recycled_to_its_originating_adapter() {
    let mut first_adapter = adapter(1, 1, 2, 2);
    let mut second_adapter = adapter(1, 1, 2, 2);
    first_adapter
        .try_submit(add(1, 100))
        .expect("command admitted");
    first_adapter.process_next();
    let completed = first_adapter.take_completed().expect("result is ready");

    let completed = second_adapter
        .recycle(completed)
        .expect_err("foreign response must remain recoverable");
    first_adapter
        .recycle(completed)
        .expect("originating adapter accepts its credit");
    assert_eq!(first_adapter.available_response_credits(), 1);
    assert_eq!(second_adapter.available_response_credits(), 1);
}
