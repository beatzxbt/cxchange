//! Maximum-capacity matching through admission, queued execution, and delivery.
//!
//! Checks every maker fill/order delta and all level removals for a 131,072-order
//! sweep, then executes later work while the consumer still owns that sweep's
//! response. This catches truncation and accidental reuse of leased output.

use matching_core::execution::{AdmissionError, ExecutionAdapter, ExecutionConfig};
use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MAX_LIVE_ORDERS, MatchingEngine, MatchingResponse, OrderId, OrderStatus, Price, Quantity, Side,
};

#[test]
fn default_full_sweep_preserves_every_event_while_the_consumer_holds_its_credit() {
    let config = EngineConfig::default();
    let domain = EngineId(91);
    let mut engine = MatchingEngine::new(
        domain,
        InstrumentSpec {
            symbol: "MAXIMUM".into(),
            tick_size: DecimalScale::new(1, 0).unwrap(),
            lot_size: DecimalScale::new(1, 0).unwrap(),
            min_size: Quantity(1),
            max_size: None,
        },
        config,
    )
    .unwrap();
    let mut seed_output = MatchingResponse::with_capacity(&config).unwrap();
    for index in 0..MAX_LIVE_ORDERS {
        engine
            .execute_into(
                &Command::Add {
                    account_id: AccountId(1),
                    client_id: ClientOrderId(index as u64),
                    side: Side::Sell,
                    price: Price(100 + (index / config.max_orders_per_level) as u64),
                    size: Quantity(1),
                },
                &mut seed_output,
            )
            .unwrap();
    }
    drop(seed_output);
    let mut adapter = ExecutionAdapter::new(
        engine,
        ExecutionConfig {
            command_capacity: 2,
            response_capacity: 2,
        },
    )
    .unwrap();
    let sweep = adapter
        .try_submit(Command::Take {
            account_id: AccountId(2),
            client_id: ClientOrderId(1),
            side: Side::Buy,
            limit_price: Price(100 + config.max_levels as u64 - 1),
            size: Quantity(MAX_LIVE_ORDERS as u64),
        })
        .unwrap();
    let following_command = Command::Add {
        account_id: AccountId(3),
        client_id: ClientOrderId(1),
        side: Side::Buy,
        price: Price(1),
        size: Quantity(7),
    };
    let following = adapter.try_submit(following_command).unwrap();
    assert_eq!(adapter.process_next(), Some(sweep));
    let result = adapter.take_completed().unwrap();
    assert_eq!(result.outcome(), Ok(()));
    assert_eq!(result.response().fills.len(), MAX_LIVE_ORDERS);
    assert_eq!(result.response().order_deltas.len(), MAX_LIVE_ORDERS + 1);
    assert_eq!(result.response().level_deltas.len(), config.max_levels);
    assert_eq!(adapter.process_next(), Some(following));
    let next_result = adapter.take_completed().unwrap();
    assert_eq!(next_result.outcome(), Ok(()));
    assert_eq!(
        next_result.response().order_deltas[0].order_id.sequence,
        MAX_LIVE_ORDERS as u64 + 2
    );
    assert_eq!(
        adapter.try_submit(following_command),
        Err(AdmissionError::ResponsePoolExhausted)
    );

    let taker_id = OrderId::new(domain, MAX_LIVE_ORDERS as u64 + 1);
    let taker = result.response().order_deltas[0];
    assert_eq!(taker.order_id, taker_id);
    assert_eq!(taker.status, OrderStatus::Filled);
    assert_eq!(taker.filled_size, Quantity(MAX_LIVE_ORDERS as u64));
    assert_eq!(taker.remaining_size, Quantity(0));
    for (index, fill) in result.response().fills.iter().enumerate() {
        let maker_id = OrderId::new(domain, index as u64 + 1);
        let price = Price(100 + (index / config.max_orders_per_level) as u64);
        assert_eq!(fill.maker_order_id, maker_id);
        assert_eq!(fill.maker_client_id, ClientOrderId(index as u64));
        assert_eq!(fill.maker_account_id, AccountId(1));
        assert_eq!(fill.taker_order_id, taker_id);
        assert_eq!(fill.taker_client_id, ClientOrderId(1));
        assert_eq!(fill.taker_account_id, AccountId(2));
        assert_eq!(fill.taker_side, Side::Buy);
        assert_eq!(fill.price, price);
        assert_eq!(fill.size, Quantity(1));
        let delta = result.response().order_deltas[index + 1];
        assert_eq!(delta.order_id, maker_id);
        assert_eq!(delta.client_id, fill.maker_client_id);
        assert_eq!(delta.status, OrderStatus::Filled);
        assert_eq!(delta.filled_size, Quantity(1));
        assert_eq!(delta.remaining_size, Quantity(0));
        assert_eq!(delta.fill_price, Some(price));
        assert_eq!(adapter.engine().get_order(maker_id), None);
    }
    for (index, level) in result.response().level_deltas.iter().enumerate() {
        assert_eq!(level.side, Side::Sell);
        assert_eq!(level.price, Price(100 + index as u64));
        assert_eq!(level.order_count, 0);
        assert_eq!(level.size, 0);
    }
    assert_eq!(adapter.engine().best_ask(), None);
    assert_eq!(adapter.engine().best_bid(), Some(Price(1)));
    assert_eq!(adapter.available_response_credits(), 0);
    adapter.recycle(next_result).unwrap();
    adapter.recycle(result).unwrap();
    assert_eq!(adapter.available_response_credits(), 2);
    adapter.engine().validate_invariants().unwrap();
}
