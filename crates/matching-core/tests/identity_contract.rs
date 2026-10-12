//! Client identity scope for transient takers and reusable terminal orders.

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, InstrumentSpec,
    MatchingEngine, MatchingResponse, Price, Quantity, Side,
};

#[test]
fn transient_taker_client_id_does_not_replace_a_live_maker_mapping() {
    let config = EngineConfig {
        max_orders_per_level: 2,
        max_levels: 2,
    };
    let mut engine = MatchingEngine::new(
        EngineId(1),
        InstrumentSpec {
            symbol: "IDENTITY".into(),
            tick_size: DecimalScale::new(1, 0).unwrap(),
            lot_size: DecimalScale::new(1, 0).unwrap(),
            min_size: Quantity(1),
            max_size: None,
        },
        config,
    )
    .unwrap();
    let mut response = MatchingResponse::with_capacity(&config).unwrap();
    for (account, side, price) in [(1, Side::Buy, 90), (2, Side::Sell, 100)] {
        engine
            .execute_into(
                &Command::Add {
                    account_id: AccountId(account),
                    client_id: ClientOrderId(7),
                    side,
                    price: Price(price),
                    size: Quantity(3),
                },
                &mut response,
            )
            .unwrap();
    }
    let original = *engine
        .get_order_by_client_id(AccountId(1), ClientOrderId(7))
        .unwrap();
    for expected_id in [3, 4] {
        engine
            .execute_into(
                &Command::Take {
                    account_id: AccountId(1),
                    client_id: ClientOrderId(7),
                    side: Side::Buy,
                    limit_price: Price(100),
                    size: Quantity(1),
                },
                &mut response,
            )
            .unwrap();
        assert_eq!(response.fills[0].taker_order_id.sequence, expected_id);
        assert_eq!(response.fills[0].taker_client_id, ClientOrderId(7));
        assert_eq!(response.fills[0].maker_account_id, AccountId(2));
        assert_eq!(
            engine.get_order_by_client_id(AccountId(1), ClientOrderId(7)),
            Some(&original)
        );
        assert_eq!(engine.get_order(response.fills[0].taker_order_id), None);
    }
    assert_eq!(engine.get_level(Side::Sell, Price(100)).unwrap().size, 1);
    engine.validate_invariants().unwrap();
}
