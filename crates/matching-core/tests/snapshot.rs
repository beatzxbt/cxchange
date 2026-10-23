//! Snapshot schema validation and deterministic continuation after recovery.
//!
//! Valid fixtures exercise empty/complex state and partial remainders; mutated
//! fixtures isolate malformed identities, ordering, bounds, and capacities.
//! Run with `cargo test -p matching-core --test snapshot`.

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, EngineSnapshot,
    InstrumentSpec, LevelSnapshot, MatchingEngine, MatchingError, MatchingResponse, Order, OrderId,
    Price, Quantity, SNAPSHOT_VERSION, Side,
};

fn spec(min_size: u64, max_size: Option<u64>) -> InstrumentSpec {
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
        min_size: Quantity(min_size),
        max_size: max_size.map(Quantity),
    }
}

fn config() -> EngineConfig {
    EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    }
}

fn new_engine(engine_id: u64, instrument: InstrumentSpec) -> MatchingEngine {
    MatchingEngine::new(EngineId(engine_id), instrument, config()).expect("valid engine")
}

fn new_engine_with_config(
    engine_id: u64,
    instrument: InstrumentSpec,
    engine_config: EngineConfig,
) -> MatchingEngine {
    MatchingEngine::new(EngineId(engine_id), instrument, engine_config).expect("valid engine")
}

fn run(engine: &mut MatchingEngine, command: Command) -> MatchingResponse {
    let mut response =
        MatchingResponse::with_capacity(&engine.config()).expect("response capacity");
    engine
        .execute_into(&command, &mut response)
        .expect("command succeeds");
    response
}

fn valid_snapshot() -> EngineSnapshot {
    let mut engine = new_engine(7, spec(1, Some(100)));
    run(
        &mut engine,
        Command::Add {
            account_id: AccountId(1),
            client_id: ClientOrderId(11),
            side: Side::Buy,
            price: Price(100),
            size: Quantity(10),
        },
    );
    run(
        &mut engine,
        Command::Add {
            account_id: AccountId(2),
            client_id: ClientOrderId(22),
            side: Side::Sell,
            price: Price(110),
            size: Quantity(11),
        },
    );
    engine.export_snapshot()
}

fn invalid_snapshot(snapshot: EngineSnapshot) {
    assert_eq!(
        MatchingEngine::from_snapshot(snapshot).err(),
        Some(MatchingError::InvalidSnapshot)
    );
}

#[expect(
    clippy::too_many_arguments,
    reason = "Snapshot fixtures vary each order field independently"
)]
fn order(
    engine_id: u64,
    order_sequence: u64,
    client_id: u64,
    account_id: u64,
    price: u64,
    size: u64,
    side: Side,
    priority_sequence: u64,
) -> Order {
    Order {
        order_id: OrderId::new(EngineId(engine_id), order_sequence),
        client_id: ClientOrderId(client_id),
        account_id: AccountId(account_id),
        price: Price(price),
        size: Quantity(size),
        side,
        sequence: priority_sequence,
    }
}

fn append_order(snapshot: &mut EngineSnapshot, level_side: Side, order: Order) {
    let levels = match level_side {
        Side::Buy => &mut snapshot.bids,
        Side::Sell => &mut snapshot.asks,
    };
    let level = levels
        .iter_mut()
        .find(|level| level.price == order.price)
        .expect("target level exists");
    level.orders.push(order);
}

#[test]
fn empty_engine_snapshot_round_trips_through_serde_and_restore() {
    let engine = new_engine(77, spec(1, Some(100)));
    let snapshot = engine.export_snapshot();

    assert_eq!(snapshot.version, SNAPSHOT_VERSION);
    assert_eq!(snapshot.engine_id, EngineId(77));
    assert_eq!(snapshot.next_order_id, 1);
    assert_eq!(snapshot.next_sequence, 0);
    assert!(snapshot.bids.is_empty());
    assert!(snapshot.asks.is_empty());

    let serialized = serde_json::to_string(&snapshot).expect("snapshot serializes");
    let decoded: EngineSnapshot = serde_json::from_str(&serialized).expect("snapshot deserializes");
    assert_eq!(decoded, snapshot);

    let restored = MatchingEngine::from_snapshot(decoded).expect("empty snapshot restores");
    assert_eq!(restored.export_snapshot(), snapshot);
    assert_eq!(restored.validate_invariants(), Ok(()));
}

#[test]
fn complex_snapshot_preserves_fifo_state_and_continued_results() {
    let mut original = new_engine_with_config(
        77,
        spec(1, Some(100)),
        EngineConfig {
            max_orders_per_level: 2,
            max_levels: 4,
        },
    );
    let mut add = |account_id, client_id, side, price, size| {
        run(
            &mut original,
            Command::Add {
                account_id: AccountId(account_id),
                client_id: ClientOrderId(client_id),
                side,
                price: Price(price),
                size: Quantity(size),
            },
        );
    };
    add(1, 11, Side::Buy, 100, 10);
    add(2, 22, Side::Buy, 100, 11);
    add(3, 33, Side::Buy, 90, 5);
    add(4, 44, Side::Sell, 110, 8);
    add(5, 55, Side::Sell, 120, 4);

    let snapshot = original.export_snapshot();
    assert_eq!(snapshot.bids.len(), 2);
    assert_eq!(snapshot.bids[0].price, Price(100));
    assert_eq!(snapshot.bids[0].orders.len(), 2);
    assert_eq!(snapshot.bids[0].orders[0].account_id, AccountId(1));
    assert_eq!(snapshot.bids[0].orders[1].account_id, AccountId(2));
    assert_eq!(snapshot.asks[0].price, Price(110));

    let restored =
        MatchingEngine::from_snapshot(snapshot.clone()).expect("complex snapshot restores");
    assert_eq!(restored.export_snapshot(), snapshot);
    assert_eq!(restored.validate_invariants(), Ok(()));

    let replaced = original
        .get_order_by_client_id(AccountId(1), ClientOrderId(11))
        .expect("maker exists")
        .order_id;
    let amend = Command::Amend {
        account_id: AccountId(1),
        order_id: replaced,
        price: Price(100),
        size: Quantity(9),
    };
    let mut original_response = MatchingResponse::with_capacity(&original.config()).unwrap();
    let mut restored_response = MatchingResponse::with_capacity(&restored.config()).unwrap();
    original
        .execute_into(&amend, &mut original_response)
        .expect("original amend succeeds");
    let mut restored = restored;
    restored
        .execute_into(&amend, &mut restored_response)
        .expect("restored amend succeeds");
    assert_eq!(restored_response, original_response);

    let take = Command::Take {
        account_id: AccountId(99),
        client_id: ClientOrderId(99),
        side: Side::Buy,
        limit_price: Price(120),
        size: Quantity(10),
    };
    original
        .execute_into(&take, &mut original_response)
        .expect("original take succeeds");
    restored
        .execute_into(&take, &mut restored_response)
        .expect("restored take succeeds");
    assert_eq!(restored_response, original_response);
    assert_eq!(restored.export_snapshot(), original.export_snapshot());
}

#[test]
fn partially_filled_remainder_below_minimum_size_restores() {
    let mut engine = new_engine(7, spec(5, Some(100)));
    run(
        &mut engine,
        Command::Add {
            account_id: AccountId(1),
            client_id: ClientOrderId(11),
            side: Side::Sell,
            price: Price(110),
            size: Quantity(10),
        },
    );
    run(
        &mut engine,
        Command::Take {
            account_id: AccountId(2),
            client_id: ClientOrderId(22),
            side: Side::Buy,
            limit_price: Price(110),
            size: Quantity(7),
        },
    );

    let snapshot = engine.export_snapshot();
    assert_eq!(snapshot.asks[0].orders[0].size, Quantity(3));
    let restored =
        MatchingEngine::from_snapshot(snapshot.clone()).expect("partial remainder restores");
    assert_eq!(restored.export_snapshot(), snapshot);
    assert_eq!(restored.validate_invariants(), Ok(()));
}

#[test]
fn restore_rejects_invalid_schema_identity_specification_and_configuration() {
    let mut snapshot = valid_snapshot();
    snapshot.version += 1;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.engine_id = EngineId(0);
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.spec.min_size = Quantity(0);
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.config.max_orders_per_level = 0;
    invalid_snapshot(snapshot);
}

#[test]
fn restore_rejects_empty_unsorted_zero_price_and_wrong_side_levels() {
    let mut snapshot = valid_snapshot();
    snapshot.bids[0].orders.clear();
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.bids[0].price = Price(0);
    snapshot.bids[0].orders[0].price = Price(0);
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.bids[0].orders[0].side = Side::Sell;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.config.max_levels = 3;
    snapshot.bids.push(LevelSnapshot {
        price: Price(105),
        orders: vec![order(7, 3, 33, 3, 105, 1, Side::Buy, 2)],
    });
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);
}

#[test]
fn restore_rejects_duplicate_order_client_and_priority_ids_and_bad_fifo_order() {
    let mut snapshot = valid_snapshot();
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 1, 33, 3, 100, 1, Side::Buy, 2),
    );
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 3, 11, 1, 100, 1, Side::Buy, 2),
    );
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 3, 33, 3, 100, 1, Side::Buy, 1),
    );
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    let first_bid = snapshot.bids[0].orders[0];
    snapshot.bids[0].orders[0].sequence = 2;
    snapshot.asks[0].orders[0].sequence = 0;
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 3, 33, 3, 100, 1, Side::Buy, 1),
    );
    snapshot.bids[0].orders[0].order_id = first_bid.order_id;
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);
}

#[test]
fn restore_rejects_invalid_order_counters_and_crossed_books() {
    let mut snapshot = valid_snapshot();
    snapshot.next_order_id = 0;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.bids[0].orders[0].order_id.sequence = snapshot.next_order_id;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.asks[0].orders[0].sequence = snapshot.next_sequence;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.asks[0].price = Price(100);
    snapshot.asks[0].orders[0].price = Price(100);
    invalid_snapshot(snapshot);
}

#[test]
fn restore_rejects_per_level_and_combined_level_capacity_overflow() {
    let mut snapshot = valid_snapshot();
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 3, 33, 3, 100, 1, Side::Buy, 2),
    );
    append_order(
        &mut snapshot,
        Side::Buy,
        order(7, 4, 44, 4, 100, 1, Side::Buy, 3),
    );
    snapshot.next_order_id = 5;
    snapshot.next_sequence = 4;
    invalid_snapshot(snapshot);

    let mut snapshot = valid_snapshot();
    snapshot.asks.push(LevelSnapshot {
        price: Price(120),
        orders: vec![order(7, 3, 33, 3, 120, 1, Side::Sell, 2)],
    });
    snapshot.next_order_id = 4;
    snapshot.next_sequence = 3;
    invalid_snapshot(snapshot);
}

#[test]
fn maximum_counters_and_large_aggregate_quantities_serde_round_trip() {
    let mut snapshot = valid_snapshot();
    snapshot.spec.max_size = None;
    snapshot.bids[0].orders[0].size = Quantity(u64::MAX);
    snapshot.bids[0].orders.push(order(
        7,
        u64::MAX - 1,
        33,
        3,
        100,
        u64::MAX,
        Side::Buy,
        u64::MAX - 2,
    ));
    snapshot.next_order_id = u64::MAX;
    snapshot.next_sequence = u64::MAX - 1;

    let serialized = serde_json::to_string(&snapshot).expect("maximum counters serialize");
    let decoded: EngineSnapshot =
        serde_json::from_str(&serialized).expect("maximum counters decode");
    assert_eq!(decoded, snapshot);

    let mut restored = MatchingEngine::from_snapshot(decoded).expect("maximum counters restore");
    assert_eq!(
        restored
            .get_level(Side::Buy, Price(100))
            .expect("bid exists")
            .size,
        u128::from(u64::MAX) * 2
    );
    assert_eq!(restored.export_snapshot(), snapshot);

    let before = restored.export_snapshot();
    let mut response = MatchingResponse::with_capacity(&restored.config()).unwrap();
    let result = restored.execute_into(
        &Command::Amend {
            account_id: AccountId(1),
            order_id: before.bids[0].orders[0].order_id,
            price: Price(100),
            size: Quantity(1),
        },
        &mut response,
    );
    assert_eq!(result, Err(MatchingError::CounterExhausted));
    assert_eq!(restored.export_snapshot(), before);
}
