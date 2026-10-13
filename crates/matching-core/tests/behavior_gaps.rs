//! Directed behavior gaps retained from the Python reference test review.

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MatchingEngine, MatchingError, MatchingResponse, Price, Quantity, Side,
};

fn engine() -> MatchingEngine {
    MatchingEngine::new(
        EngineId(29),
        InstrumentSpec {
            symbol: "REVIEW".into(),
            tick_size: DecimalScale::new(1, 0).unwrap(),
            lot_size: DecimalScale::new(1, 0).unwrap(),
            min_size: Quantity(2),
            max_size: Some(Quantity(10)),
        },
        EngineConfig {
            max_orders_per_level: 2,
            max_levels: 6,
        },
    )
    .unwrap()
}

fn response() -> MatchingResponse {
    MatchingResponse::with_capacity(&EngineConfig {
        max_orders_per_level: 2,
        max_levels: 6,
    })
    .unwrap()
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

fn top_two(engine: &MatchingEngine, side: Side) -> Vec<(Price, u128, usize)> {
    engine
        .depth(side, 2)
        .into_iter()
        .map(|level| (level.price, level.size, level.order_count))
        .collect()
}

#[test]
fn rejected_crossing_and_invalid_size_amends_preserve_original_and_identity() {
    let mut engine = engine();
    let mut out = response();
    engine
        .execute_into(&add(1, 7, Side::Buy, 100, 3), &mut out)
        .unwrap();
    let original = out.order_deltas[0].order_id;
    engine
        .execute_into(&add(2, 8, Side::Sell, 110, 3), &mut out)
        .unwrap();
    let original_ask = out.order_deltas[0].order_id;
    let before = engine.export_snapshot();

    for (account, order_id, price, size, expected) in [
        (1, original, 110, 3, MatchingError::WouldCross),
        (2, original_ask, 100, 3, MatchingError::WouldCross),
        (1, original, 105, 1, MatchingError::InvalidSize),
        (1, original, 105, 11, MatchingError::InvalidSize),
    ] {
        assert_eq!(
            engine.execute_into(
                &Command::Amend {
                    account_id: AccountId(account),
                    order_id,
                    price: Price(price),
                    size: Quantity(size),
                },
                &mut out,
            ),
            Err(expected)
        );
        assert_eq!(out, MatchingResponse::default());
        assert_eq!(engine.export_snapshot(), before);
        engine.validate_invariants().unwrap();
    }

    engine
        .execute_into(
            &Command::Amend {
                account_id: AccountId(1),
                order_id: original,
                price: Price(105),
                size: Quantity(10),
            },
            &mut out,
        )
        .unwrap();
    let replacement = out.order_deltas[1].order_id;
    assert_eq!(replacement.sequence, before.next_order_id);
    assert_eq!(engine.get_order(original), None);
    assert_eq!(
        engine
            .get_order_by_client_id(AccountId(1), ClientOrderId(7))
            .unwrap()
            .order_id,
        replacement
    );
    assert_eq!(engine.best_bid(), Some(Price(105)));
    assert_eq!(out.level_deltas[0].size, 0);
    assert_eq!(out.level_deltas[1].size, 10);

    engine
        .execute_into(
            &Command::Amend {
                account_id: AccountId(2),
                order_id: original_ask,
                price: Price(115),
                size: Quantity(2),
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(
        out.order_deltas[1].order_id.sequence,
        replacement.sequence + 1
    );
    assert_eq!(engine.best_ask(), Some(Price(115)));
    assert_eq!(out.level_deltas[0].size, 0);
    assert_eq!(out.level_deltas[1].size, 2);
    engine.validate_invariants().unwrap();
}

#[test]
fn filled_maker_releases_client_id_as_best_price_and_limited_depth_advance() {
    let mut engine = engine();
    let mut out = response();
    let mut bid_ids = Vec::new();
    for (account, price, size) in [(11, 90, 2), (12, 91, 3), (13, 92, 4)] {
        engine
            .execute_into(&add(account, account, Side::Buy, price, size), &mut out)
            .unwrap();
        bid_ids.push(out.order_deltas[0].order_id);
    }
    let mut maker_ids = Vec::new();
    for (account, client, price, size) in [(1, 7, 100, 2), (2, 8, 101, 3), (3, 9, 102, 4)] {
        engine
            .execute_into(&add(account, client, Side::Sell, price, size), &mut out)
            .unwrap();
        maker_ids.push(out.order_deltas[0].order_id);
    }
    assert_eq!(
        top_two(&engine, Side::Buy),
        vec![(Price(92), 4, 1), (Price(91), 3, 1)]
    );
    assert_eq!(
        top_two(&engine, Side::Sell),
        vec![(Price(100), 2, 1), (Price(101), 3, 1)]
    );

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(4),
                client_id: ClientOrderId(10),
                side: Side::Buy,
                limit_price: Price(100),
                size: Quantity(2),
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(out.fills[0].maker_order_id, maker_ids[0]);
    assert_eq!(engine.get_order(maker_ids[0]), None);
    assert!(!engine.has_order(AccountId(1), ClientOrderId(7)));
    assert_eq!(engine.get_level(Side::Sell, Price(100)), None);
    assert_eq!(engine.best_ask(), Some(Price(101)));

    engine
        .execute_into(&add(1, 7, Side::Sell, 103, 2), &mut out)
        .unwrap();
    assert_eq!(out.order_deltas[0].order_id.sequence, 8);
    assert_eq!(
        top_two(&engine, Side::Sell),
        vec![(Price(101), 3, 1), (Price(102), 4, 1)]
    );
    engine
        .execute_into(
            &Command::Cancel {
                account_id: AccountId(2),
                order_id: maker_ids[1],
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(
        top_two(&engine, Side::Sell),
        vec![(Price(102), 4, 1), (Price(103), 2, 1)]
    );

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(4),
                client_id: ClientOrderId(11),
                side: Side::Sell,
                limit_price: Price(92),
                size: Quantity(4),
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(out.fills[0].maker_order_id, bid_ids[2]);
    assert_eq!(engine.best_bid(), Some(Price(91)));
    assert_eq!(
        top_two(&engine, Side::Buy),
        vec![(Price(91), 3, 1), (Price(90), 2, 1)]
    );
    engine.validate_invariants().unwrap();
}

#[test]
fn identical_value_amend_still_replaces_identity_and_loses_fifo_priority() {
    let mut engine = engine();
    let mut out = response();
    engine
        .execute_into(&add(1, 1, Side::Buy, 100, 2), &mut out)
        .unwrap();
    let original = out.order_deltas[0].order_id;
    engine
        .execute_into(&add(2, 2, Side::Buy, 100, 3), &mut out)
        .unwrap();
    let peer = out.order_deltas[0].order_id;

    engine
        .execute_into(
            &Command::Amend {
                account_id: AccountId(1),
                order_id: original,
                price: Price(100),
                size: Quantity(2),
            },
            &mut out,
        )
        .unwrap();
    let replacement = out.order_deltas[1].order_id;
    assert_eq!(replacement.sequence, peer.sequence + 1);
    assert_eq!(engine.get_order(original), None);
    assert_eq!(out.level_deltas[0].size, 3);
    assert_eq!(out.level_deltas[1].size, 5);

    engine
        .execute_into(
            &Command::Take {
                account_id: AccountId(3),
                client_id: ClientOrderId(3),
                side: Side::Sell,
                limit_price: Price(100),
                size: Quantity(3),
            },
            &mut out,
        )
        .unwrap();
    assert_eq!(out.fills[0].maker_order_id, peer);
    assert_eq!(engine.get_order(replacement).unwrap().size, Quantity(2));
    engine.validate_invariants().unwrap();
}
