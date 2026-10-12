//! Boundary and rejection-atomicity checks for the public matching engine.

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MatchingEngine, MatchingError, MatchingResponse, OrderId, OrderStatus, Price, Quantity, Side,
};

fn spec(max_size: Option<u64>) -> InstrumentSpec {
    InstrumentSpec {
        symbol: "EDGE".to_owned(),
        tick_size: DecimalScale::new(1, 0).unwrap(),
        lot_size: DecimalScale::new(1, 0).unwrap(),
        min_size: Quantity(1),
        max_size: max_size.map(Quantity),
    }
}

fn engine(config: EngineConfig, max_size: Option<u64>) -> MatchingEngine {
    MatchingEngine::new(EngineId(17), spec(max_size), config).unwrap()
}

fn response(config: EngineConfig) -> MatchingResponse {
    MatchingResponse::with_capacity(&config).unwrap()
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

fn take(account: u64, client: u64, side: Side, limit: u64, size: u64) -> Command {
    Command::Take {
        account_id: AccountId(account),
        client_id: ClientOrderId(client),
        side,
        limit_price: Price(limit),
        size: Quantity(size),
    }
}

#[test]
fn default_level_limit_counts_both_sides_and_released_level_is_reusable() {
    let config = EngineConfig::default();
    assert_eq!(config.max_orders_per_level, 128);
    assert_eq!(config.max_levels, 1_024);
    let mut engine = engine(config, Some(1));
    let mut out = response(config);

    for offset in 0..512_u64 {
        engine
            .execute_into(
                &add(offset + 1, offset + 1, Side::Buy, 1_000 - offset, 1),
                &mut out,
            )
            .unwrap();
        engine
            .execute_into(
                &add(offset + 513, offset + 513, Side::Sell, 2_000 + offset, 1),
                &mut out,
            )
            .unwrap();
    }
    let before = engine.export_snapshot();
    assert_eq!(before.bids.len(), 512);
    assert_eq!(before.asks.len(), 512);
    assert_eq!(engine.best_bid(), Some(Price(1_000)));
    assert_eq!(engine.best_ask(), Some(Price(2_000)));

    assert_eq!(
        engine.execute_into(&add(2_000, 2_000, Side::Buy, 1_001, 1), &mut out),
        Err(MatchingError::CapacityExceeded)
    );
    assert_eq!(out, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);

    let removed = before.asks[0].orders[0].order_id;
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(513),
                order_id: removed,
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(out.level_deltas[0].order_count, 0);
    engine
        .execute_into(&add(2_000, 2_000, Side::Buy, 1_001, 1), &mut out)
        .unwrap();
    assert_eq!(out.order_deltas[0].order_id.sequence, before.next_order_id);
    assert_eq!(engine.export_snapshot().bids.len(), 513);
    assert_eq!(engine.export_snapshot().asks.len(), 511);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn default_order_limit_is_per_price_level_on_each_side() {
    let config = EngineConfig::default();
    let mut engine = engine(config, Some(1));
    let mut out = response(config);
    for index in 0..128_u64 {
        engine
            .execute_into(&add(index + 1, index + 1, Side::Buy, 100, 1), &mut out)
            .unwrap();
        engine
            .execute_into(&add(index + 129, index + 129, Side::Sell, 200, 1), &mut out)
            .unwrap();
    }
    let before = engine.export_snapshot();
    assert_eq!(
        engine.get_level(Side::Buy, Price(100)).unwrap().order_count,
        128
    );
    assert_eq!(
        engine
            .get_level(Side::Sell, Price(200))
            .unwrap()
            .order_count,
        128
    );

    for (side, price) in [(Side::Buy, 100), (Side::Sell, 200)] {
        assert_eq!(
            engine.execute_into(&add(500, 500, side, price, 1), &mut out),
            Err(MatchingError::CapacityExceeded)
        );
        assert_eq!(out, MatchingResponse::default());
        assert_eq!(engine.export_snapshot(), before);
    }
    engine
        .execute_into(&add(500, 500, Side::Buy, 99, 1), &mut out)
        .unwrap();
    assert_eq!(out.order_deltas[0].order_id.sequence, before.next_order_id);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn amendment_uses_resulting_level_count_when_old_level_has_a_sibling() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut engine = engine(config, None);
    let mut out = response(config);
    engine
        .execute_into(&add(1, 1, Side::Buy, 100, 2), &mut out)
        .unwrap();
    let moving = out.order_deltas[0].order_id;
    engine
        .execute_into(&add(2, 2, Side::Buy, 100, 3), &mut out)
        .unwrap();
    let sibling = out.order_deltas[0].order_id;
    engine
        .execute_into(&add(3, 3, Side::Sell, 200, 4), &mut out)
        .unwrap();
    let before = engine.export_snapshot();
    let amend = Command::Amend {
        account_id: AccountId(1),
        order_id: moving,
        price: Price(101),
        size: Quantity(7),
    };

    assert_eq!(
        engine.execute_into(&amend, &mut out),
        Err(MatchingError::CapacityExceeded)
    );
    assert_eq!(out, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(2),
                order_id: sibling,
            },
            &mut out,
        )
        .unwrap();
    engine.execute_into(&amend, &mut out).unwrap();
    assert_eq!(out.order_deltas[0].status, OrderStatus::Replaced);
    assert_eq!(out.order_deltas[1].order_id.sequence, before.next_order_id);
    assert_eq!(out.level_deltas[0].size, 0);
    assert_eq!(out.level_deltas[1].size, 7);
    assert_eq!(engine.get_order(moving), None);
    assert_eq!(
        engine.get_level(Side::Buy, Price(101)).unwrap().order_count,
        1
    );
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn insufficient_output_for_multifill_preserves_book_counters_and_empty_response() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut engine = engine(config, None);
    let mut out = response(config);
    engine
        .execute_into(&add(1, 1, Side::Sell, 100, 2), &mut out)
        .unwrap();
    engine
        .execute_into(&add(2, 2, Side::Sell, 101, 3), &mut out)
        .unwrap();
    let before = engine.export_snapshot();
    let mut short = MatchingResponse {
        fills: Vec::with_capacity(1),
        order_deltas: Vec::with_capacity(3),
        level_deltas: Vec::with_capacity(2),
    };
    let command = take(3, 3, Side::Buy, 101, 5);
    assert_eq!(
        engine.execute_into(&command, &mut short),
        Err(MatchingError::OutputCapacity)
    );
    assert_eq!(short, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);

    let mut short_deltas = MatchingResponse {
        fills: Vec::with_capacity(2),
        order_deltas: Vec::with_capacity(2),
        level_deltas: Vec::with_capacity(2),
    };
    assert_eq!(
        engine.execute_into(&command, &mut short_deltas),
        Err(MatchingError::OutputCapacity)
    );
    assert_eq!(short_deltas, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);

    engine.execute_into(&command, &mut out).unwrap();
    assert_eq!(out.fills.len(), 2);
    assert_eq!(out.order_deltas.len(), 3);
    assert_eq!(out.order_deltas[0].order_id.sequence, before.next_order_id);
    assert_eq!(engine.best_ask(), None);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn insufficient_output_for_amendment_preserves_original_and_client_index() {
    let config = EngineConfig {
        max_orders_per_level: 1,
        max_levels: 1,
    };
    let mut engine = engine(config, None);
    let mut out = response(config);
    engine
        .execute_into(&add(1, 1, Side::Buy, 100, 2), &mut out)
        .unwrap();
    let original = out.order_deltas[0].order_id;
    let before = engine.export_snapshot();
    let amend = Command::Amend {
        account_id: AccountId(1),
        order_id: original,
        price: Price(101),
        size: Quantity(3),
    };
    let mut short = MatchingResponse {
        fills: Vec::new(),
        order_deltas: Vec::with_capacity(2),
        level_deltas: Vec::with_capacity(1),
    };

    assert_eq!(
        engine.execute_into(&amend, &mut short),
        Err(MatchingError::OutputCapacity)
    );
    assert_eq!(short, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);
    assert_eq!(
        engine
            .get_order_by_client_id(AccountId(1), ClientOrderId(1))
            .unwrap()
            .order_id,
        original
    );

    engine.execute_into(&amend, &mut out).unwrap();
    assert_eq!(out.order_deltas.len(), 2);
    assert_eq!(out.level_deltas.len(), 2);
    assert_eq!(out.order_deltas[1].order_id.sequence, before.next_order_id);
    assert_eq!(engine.get_order(original), None);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn rejection_identity_and_stale_ids_leave_state_unchanged() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 1,
    };
    let mut engine = engine(config, None);
    let mut out = response(config);
    engine
        .execute_into(&add(1, 7, Side::Buy, 100, 1), &mut out)
        .unwrap();
    let first = out.order_deltas[0].order_id;
    let before = engine.export_snapshot();
    let cases = [
        (
            add(1, 7, Side::Buy, 100, 1),
            MatchingError::DuplicateClientId,
        ),
        (
            Command::Cancel {
                account_id: AccountId(2),
                order_id: first,
            },
            MatchingError::WrongOwner,
        ),
        (
            Command::Cancel {
                account_id: AccountId(1),
                order_id: OrderId::new(EngineId(18), first.sequence),
            },
            MatchingError::WrongEngine,
        ),
        (
            Command::Amend {
                account_id: AccountId(1),
                order_id: OrderId::new(EngineId(17), first.sequence + 1),
                price: Price(101),
                size: Quantity(1),
            },
            MatchingError::OrderNotFound,
        ),
    ];
    for (command, expected) in cases {
        assert_eq!(engine.execute_into(&command, &mut out), Err(expected));
        assert_eq!(out, MatchingResponse::default());
        assert_eq!(engine.export_snapshot(), before);
    }

    engine
        .execute_into(&add(2, 7, Side::Buy, 100, 1), &mut out)
        .unwrap();
    assert_eq!(out.order_deltas[0].order_id.sequence, 2);
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(1),
                order_id: first,
            },
            &mut out,
        )
        .unwrap();
    engine
        .execute_into(&add(1, 7, Side::Buy, 100, 1), &mut out)
        .unwrap();
    assert_eq!(out.order_deltas[0].order_id.sequence, 3);
    assert_eq!(engine.get_order(first), None);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn maximum_u64_quantities_accumulate_and_decrement_in_u128() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 1,
    };
    let mut engine = engine(config, None);
    let mut out = response(config);
    engine
        .execute_into(&add(1, 1, Side::Sell, u64::MAX, u64::MAX), &mut out)
        .unwrap();
    engine
        .execute_into(&add(2, 2, Side::Sell, u64::MAX, u64::MAX), &mut out)
        .unwrap();
    let total = u128::from(u64::MAX) * 2;
    assert_eq!(out.level_deltas[0].size, total);
    assert_eq!(
        engine.get_level(Side::Sell, Price(u64::MAX)).unwrap().size,
        total
    );
    let before = engine.export_snapshot();
    assert_eq!(
        engine.execute_into(&add(3, 3, Side::Buy, u64::MAX, 1), &mut out),
        Err(MatchingError::WouldCross)
    );
    assert_eq!(out, MatchingResponse::default());
    assert_eq!(engine.export_snapshot(), before);

    engine
        .execute_into(&take(3, 3, Side::Buy, u64::MAX, u64::MAX), &mut out)
        .unwrap();
    assert_eq!(out.fills.len(), 1);
    assert_eq!(out.fills[0].size, Quantity(u64::MAX));
    assert_eq!(out.level_deltas[0].size, u128::from(u64::MAX));
    assert_eq!(out.level_deltas[0].order_count, 1);
    assert_eq!(engine.validate_invariants(), Ok(()));
}

#[test]
fn final_valid_identity_is_allocated_then_exhaustion_is_atomic() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 1,
    };
    let fresh = engine(config, None);
    let mut snapshot = fresh.export_snapshot();
    snapshot.next_order_id = u64::MAX - 1;
    snapshot.next_sequence = u64::MAX - 2;
    let mut engine = MatchingEngine::from_snapshot(snapshot).unwrap();
    let mut out = response(config);

    engine
        .execute_into(&add(1, 1, Side::Sell, 100, 1), &mut out)
        .unwrap();
    let last = out.order_deltas[0].order_id;
    assert_eq!(last.sequence, u64::MAX - 1);
    assert_eq!(engine.get_order(last).unwrap().sequence, u64::MAX - 2);
    let before = engine.export_snapshot();
    assert_eq!(before.next_order_id, u64::MAX);
    assert_eq!(before.next_sequence, u64::MAX - 1);

    for command in [
        add(2, 2, Side::Sell, 100, 1),
        take(2, 2, Side::Buy, 100, 1),
        Command::Amend {
            account_id: AccountId(1),
            order_id: last,
            price: Price(101),
            size: Quantity(1),
        },
    ] {
        assert_eq!(
            engine.execute_into(&command, &mut out),
            Err(MatchingError::CounterExhausted)
        );
        assert_eq!(out, MatchingResponse::default());
        assert_eq!(engine.export_snapshot(), before);
    }
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(1),
                order_id: last,
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(out.order_deltas[0].status, OrderStatus::Cancelled);
    assert_eq!(engine.validate_invariants(), Ok(()));
}
