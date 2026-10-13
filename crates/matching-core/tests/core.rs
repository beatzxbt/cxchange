//! Public command contracts, storage transitions, and maximum-capacity matching.
//!
//! Directed fixtures check response order, ownership, rejection atomicity,
//! FIFO replacement, partial fills, and a complete default-book sweep. Run with
//! `cargo test -p matching-core --test core` in debug or release mode.

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MatchingEngine, MatchingError, MatchingResponse, OrderId, OrderStatus, Price, Quantity, Side,
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

fn engine(max_orders_per_level: usize, max_levels: usize) -> MatchingEngine {
    MatchingEngine::new(
        EngineId(1),
        spec(),
        EngineConfig {
            max_orders_per_level,
            max_levels,
        },
    )
    .expect("valid test engine")
}

fn response(config: &EngineConfig) -> MatchingResponse {
    MatchingResponse::with_capacity(config).expect("valid response capacity")
}

fn add(account_id: u64, client_id: u64, side: Side, price: u64, size: u64) -> Command {
    Command::Add {
        account_id: AccountId(account_id),
        client_id: ClientOrderId(client_id),
        side,
        price: Price(price),
        size: Quantity(size),
    }
}

#[test]
fn maker_add_cancel_queries_and_account_scoped_client_identity_work_together() {
    let config = EngineConfig {
        max_orders_per_level: 3,
        max_levels: 4,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);

    engine
        .execute_into(&add(10, 7, Side::Buy, 100, 3), &mut result)
        .expect("first maker opens");
    let first = result.order_deltas[0].order_id;
    engine
        .execute_into(&add(20, 7, Side::Buy, 100, 4), &mut result)
        .expect("another account can use the same client ID");
    let second = result.order_deltas[0].order_id;

    assert_eq!(engine.best_bid(), Some(Price(100)));
    assert_eq!(engine.depth(Side::Buy, 0), Vec::new());
    assert_eq!(engine.depth(Side::Buy, 1)[0].size, 7);
    assert_eq!(
        engine.get_order(first).expect("first order is live").size,
        Quantity(3)
    );
    assert_eq!(
        engine
            .get_order_by_client_id(AccountId(20), ClientOrderId(7))
            .expect("account scoped lookup succeeds")
            .order_id,
        second
    );

    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(10),
                order_id: first,
            },
            &mut result,
        )
        .expect("owner can cancel");
    assert_eq!(result.order_deltas[0].status, OrderStatus::Cancelled);
    assert_eq!(result.level_deltas[0].size, 4);
    assert!(!engine.has_order(AccountId(10), ClientOrderId(7)));
    assert!(engine.has_order(AccountId(20), ClientOrderId(7)));
    engine
        .execute_into(&add(10, 7, Side::Sell, 110, 1), &mut result)
        .expect("terminal client ID can be reused");
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn add_rejects_duplicate_crossing_and_capacity_without_consuming_identity() {
    let config = EngineConfig {
        max_orders_per_level: 1,
        max_levels: 1,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Sell, 110, 1), &mut result)
        .expect("initial ask opens");

    assert_eq!(
        engine.execute_into(&add(10, 1, Side::Sell, 111, 1), &mut result),
        Err(MatchingError::DuplicateClientId)
    );
    assert_eq!(
        engine.execute_into(&add(20, 2, Side::Buy, 110, 1), &mut result),
        Err(MatchingError::WouldCross)
    );
    assert_eq!(
        engine.execute_into(&add(20, 2, Side::Sell, 111, 1), &mut result),
        Err(MatchingError::CapacityExceeded)
    );
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(10),
                order_id: OrderId::new(EngineId(1), 1),
            },
            &mut result,
        )
        .expect("remove initial ask");
    engine
        .execute_into(&add(20, 2, Side::Buy, 100, 1), &mut result)
        .expect("failed commands did not consume an order ID");
    assert_eq!(result.order_deltas[0].order_id.sequence, 2);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn invalid_price_and_size_are_rejected_before_identity_allocation() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    assert_eq!(
        engine.execute_into(&add(10, 1, Side::Buy, 0, 1), &mut result),
        Err(MatchingError::InvalidPrice)
    );
    assert_eq!(
        engine.execute_into(&add(10, 1, Side::Buy, 100, 0), &mut result),
        Err(MatchingError::InvalidSize)
    );
    assert_eq!(
        engine.execute_into(&add(10, 1, Side::Buy, 100, 1_000_001), &mut result),
        Err(MatchingError::InvalidSize)
    );
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 1), &mut result)
        .expect("invalid commands did not consume identity");
    assert_eq!(result.order_deltas[0].order_id.sequence, 1);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn cancel_and_amend_validate_owner_and_engine_domain() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 1), &mut result)
        .expect("maker opens");
    let order_id = result.order_deltas[0].order_id;

    assert_eq!(
        engine.execute_into(
            &Command::Cancel {
                account_id: AccountId(20),
                order_id,
            },
            &mut result,
        ),
        Err(MatchingError::WrongOwner)
    );
    assert_eq!(
        engine.execute_into(
            &Command::Amend {
                account_id: AccountId(10),
                order_id: OrderId::new(EngineId(2), order_id.sequence),
                price: Price(101),
                size: Quantity(1),
            },
            &mut result,
        ),
        Err(MatchingError::WrongEngine)
    );
    assert!(engine.get_order(order_id).is_some());
}

#[test]
fn canceling_a_middle_fifo_node_keeps_neighbor_links_correct() {
    let config = EngineConfig {
        max_orders_per_level: 3,
        max_levels: 1,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    let mut order_ids = Vec::new();
    for (account_id, client_id) in [(10, 1), (20, 2), (30, 3)] {
        engine
            .execute_into(&add(account_id, client_id, Side::Buy, 100, 1), &mut result)
            .expect("maker opens");
        order_ids.push(result.order_deltas[0].order_id);
    }
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(20),
                order_id: order_ids[1],
            },
            &mut result,
        )
        .expect("middle FIFO node cancels");
    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(40),
                client_id: ClientOrderId(4),
                side: Side::Sell,
                limit_price: Price(100),
                size: Quantity(2),
            },
            &mut result,
        )
        .expect("remaining FIFO nodes match");
    assert_eq!(result.fills[0].maker_order_id, order_ids[0]);
    assert_eq!(result.fills[1].maker_order_id, order_ids[2]);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn reused_arena_slots_do_not_make_stale_public_ids_live_again() {
    let config = EngineConfig {
        max_orders_per_level: 1,
        max_levels: 1,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 1), &mut result)
        .expect("first maker opens");
    let stale_id = result.order_deltas[0].order_id;
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(10),
                order_id: stale_id,
            },
            &mut result,
        )
        .expect("first maker cancels");
    engine
        .execute_into(&add(20, 2, Side::Buy, 99, 1), &mut result)
        .expect("recycled slot accepts new order");
    assert_ne!(result.order_deltas[0].order_id, stale_id);
    assert_eq!(
        engine.execute_into(
            &Command::Cancel {
                account_id: AccountId(20),
                order_id: stale_id,
            },
            &mut result,
        ),
        Err(MatchingError::OrderNotFound)
    );
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn same_price_amend_gets_a_fresh_id_and_loses_fifo_priority() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 1,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 2), &mut result)
        .expect("first bid opens");
    let original = result.order_deltas[0].order_id;
    engine
        .execute_into(&add(20, 2, Side::Buy, 100, 2), &mut result)
        .expect("second bid opens");
    let second = result.order_deltas[0].order_id;

    engine
        .execute_into(
            &Command::Amend {
                account_id: AccountId(10),
                order_id: original,
                price: Price(100),
                size: Quantity(4),
            },
            &mut result,
        )
        .expect("same-price replacement fits resulting level capacity");
    let replacement = result.order_deltas[1].order_id;
    assert_ne!(replacement, original);
    assert_eq!(result.order_deltas[0].status, OrderStatus::Replaced);
    assert_eq!(result.level_deltas.len(), 2);
    assert_eq!(result.level_deltas[0].size, 2);
    assert_eq!(result.level_deltas[1].size, 6);

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(30),
                client_id: ClientOrderId(3),
                side: Side::Sell,
                limit_price: Price(100),
                size: Quantity(1),
            },
            &mut result,
        )
        .expect("sell taker matches the best bid");
    assert_eq!(result.fills[0].maker_order_id, second);
    assert_eq!(engine.get_order(original), None);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn amend_capacity_uses_the_resulting_book_and_rejection_preserves_original() {
    let config = EngineConfig {
        max_orders_per_level: 1,
        max_levels: 2,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 1), &mut result)
        .expect("first bid opens");
    let original = result.order_deltas[0].order_id;
    engine
        .execute_into(&add(20, 2, Side::Buy, 101, 1), &mut result)
        .expect("second level opens");
    let second = result.order_deltas[0].order_id;

    assert_eq!(
        engine.execute_into(
            &Command::Amend {
                account_id: AccountId(10),
                order_id: original,
                price: Price(101),
                size: Quantity(1),
            },
            &mut result,
        ),
        Err(MatchingError::CapacityExceeded)
    );
    assert_eq!(engine.get_order(original).unwrap().price, Price(100));
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(20),
                order_id: second,
            },
            &mut result,
        )
        .expect("release one active level");
    engine
        .execute_into(&add(30, 3, Side::Buy, 99, 1), &mut result)
        .expect("rejected amend did not advance order counter");
    assert_eq!(result.order_deltas[0].order_id.sequence, 3);

    engine
        .execute_into(
            &Command::Amend {
                account_id: AccountId(10),
                order_id: original,
                price: Price(102),
                size: Quantity(1),
            },
            &mut result,
        )
        .expect("removing old level releases capacity for replacement");
    assert_eq!(result.level_deltas[0].size, 0);
    assert_eq!(result.level_deltas[1].order_count, 1);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn taker_matches_price_time_order_and_discards_unfilled_remainder() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 3,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Sell, 100, 2), &mut result)
        .expect("best ask opens");
    let first = result.order_deltas[0].order_id;
    engine
        .execute_into(&add(20, 2, Side::Sell, 100, 1), &mut result)
        .expect("FIFO second ask opens");
    let second = result.order_deltas[0].order_id;
    engine
        .execute_into(&add(30, 3, Side::Sell, 101, 2), &mut result)
        .expect("worse ask opens");

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(99),
                client_id: ClientOrderId(500),
                side: Side::Buy,
                limit_price: Price(101),
                size: Quantity(6),
            },
            &mut result,
        )
        .expect("partial liquidity is successful");
    assert_eq!(result.fills.len(), 3);
    assert_eq!(result.fills[0].maker_order_id, first);
    assert_eq!(result.fills[1].maker_order_id, second);
    assert_eq!(result.fills[2].price, Price(101));
    assert_eq!(result.order_deltas[0].status, OrderStatus::PartiallyFilled);
    assert_eq!(result.order_deltas[0].remaining_size, Quantity(0));
    assert_eq!(result.order_deltas[0].filled_size, Quantity(5));
    assert_eq!(result.level_deltas.len(), 2);
    assert_eq!(result.level_deltas[0].price, Price(100));
    assert_eq!(result.level_deltas[0].size, 0);
    assert_eq!(result.level_deltas[1].price, Price(101));
    assert_eq!(result.level_deltas[1].size, 0);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn late_self_trade_and_empty_taker_are_atomic_and_do_not_advance_counters() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 3,
    };
    let mut engine = engine(config.max_orders_per_level, config.max_levels);
    let mut result = response(&config);
    engine
        .execute_into(&add(20, 1, Side::Sell, 100, 2), &mut result)
        .expect("external ask opens");
    engine
        .execute_into(&add(10, 2, Side::Sell, 101, 1), &mut result)
        .expect("own later ask opens");

    assert_eq!(
        engine.execute_into(
            &Command::Take {
                account_id: AccountId(10),
                client_id: ClientOrderId(2),
                side: Side::Buy,
                limit_price: Price(101),
                size: Quantity(3),
            },
            &mut result,
        ),
        Err(MatchingError::SelfTrade)
    );
    assert_eq!(
        engine
            .get_order_by_client_id(AccountId(20), ClientOrderId(1))
            .unwrap()
            .size,
        Quantity(2)
    );
    assert_eq!(
        engine
            .get_order_by_client_id(AccountId(10), ClientOrderId(2))
            .unwrap()
            .size,
        Quantity(1)
    );

    assert_eq!(
        engine.execute_into(
            &Command::Take {
                account_id: AccountId(50),
                client_id: ClientOrderId(9),
                side: Side::Buy,
                limit_price: Price(99),
                size: Quantity(1),
            },
            &mut result,
        ),
        Err(MatchingError::NoLiquidity)
    );
    engine
        .execute_into(&add(30, 3, Side::Buy, 90, 1), &mut result)
        .expect("rejections did not consume ID or priority");
    assert_eq!(result.order_deltas[0].order_id.sequence, 3);
    assert_eq!(
        engine
            .get_order(result.order_deltas[0].order_id)
            .unwrap()
            .sequence,
        2
    );
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn response_capacity_failure_precedes_book_and_counter_mutation() {
    let mut engine = engine(2, 2);
    let mut too_small = MatchingResponse::default();
    assert_eq!(
        engine.execute_into(&add(10, 1, Side::Buy, 100, 1), &mut too_small),
        Err(MatchingError::OutputCapacity)
    );
    assert!(
        engine
            .get_order_by_client_id(AccountId(10), ClientOrderId(1))
            .is_none()
    );

    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Buy, 100, 1), &mut result)
        .expect("failed output did not consume the first ID");
    assert_eq!(result.order_deltas[0].order_id.sequence, 1);
}

#[test]
fn a_partial_remainder_can_be_below_the_instrument_minimum() {
    let mut minimum_two = spec();
    minimum_two.min_size = Quantity(2);
    let config = EngineConfig {
        max_orders_per_level: 1,
        max_levels: 1,
    };
    let mut engine = MatchingEngine::new(EngineId(1), minimum_two, config).expect("valid engine");
    let mut result = response(&config);
    engine
        .execute_into(&add(10, 1, Side::Sell, 100, 3), &mut result)
        .expect("maker meets original minimum size");
    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(20),
                client_id: ClientOrderId(2),
                side: Side::Buy,
                limit_price: Price(100),
                size: Quantity(2),
            },
            &mut result,
        )
        .expect("taker fills part of the order");
    assert_eq!(engine.depth(Side::Sell, 1)[0].size, 1);
    assert!(engine.validate_invariants().is_ok());
}

#[test]
fn full_capacity_sweep_returns_every_fill_order_delta_and_level_delta() {
    let config = EngineConfig::default();
    let mut engine = MatchingEngine::new(EngineId(7), spec(), config).expect("valid engine");
    let mut result = response(&config);
    let orders_per_level = config.max_orders_per_level;
    let mut client_id = 1u64;
    for level in 0..config.max_levels {
        let price = 1_000 + level as u64;
        for _ in 0..orders_per_level {
            engine
                .execute_into(
                    &add(10_000 + client_id, client_id, Side::Sell, price, 1),
                    &mut result,
                )
                .expect("all configured maker slots are admitted");
            client_id += 1;
        }
    }

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(1),
                client_id: ClientOrderId(u64::MAX),
                side: Side::Buy,
                limit_price: Price(1_000 + config.max_levels as u64 - 1),
                size: Quantity((config.max_orders_per_level * config.max_levels) as u64),
            },
            &mut result,
        )
        .expect("full eligible book sweep succeeds");
    assert_eq!(result.fills.len(), 131_072);
    assert_eq!(result.order_deltas.len(), 131_073);
    assert_eq!(result.level_deltas.len(), 1_024);
    assert!(
        result
            .fills
            .windows(2)
            .all(|pair| pair[0].price <= pair[1].price)
    );
    assert_eq!(engine.best_bid(), None);
    assert_eq!(engine.best_ask(), None);
    assert_eq!(engine.depth(Side::Sell, usize::MAX), Vec::new());
    assert!(engine.validate_invariants().is_ok());
}
