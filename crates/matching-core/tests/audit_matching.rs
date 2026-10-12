//! Independent matching model and directed price-time correctness checks.

use std::collections::BTreeSet;

use matching_core::{
    AccountId, ClientOrderId, Command, DecimalScale, EngineConfig, EngineId, Fill, InstrumentSpec,
    LevelDelta, MatchingEngine, MatchingError, MatchingResponse, Order, OrderDelta, OrderId,
    OrderStatus, Price, Quantity, Side,
};

const ENGINE_ID: EngineId = EngineId(77);

fn config() -> EngineConfig {
    EngineConfig {
        max_orders_per_level: 8,
        max_levels: 8,
    }
}

fn engine() -> MatchingEngine {
    MatchingEngine::new(
        ENGINE_ID,
        InstrumentSpec {
            symbol: "AUDIT".into(),
            tick_size: DecimalScale::new(1, 2).unwrap(),
            lot_size: DecimalScale::new(1, 3).unwrap(),
            min_size: Quantity(1),
            max_size: Some(Quantity(50)),
        },
        config(),
    )
    .unwrap()
}

fn buffer() -> MatchingResponse {
    MatchingResponse::with_capacity(&config()).unwrap()
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

fn place(
    engine: &mut MatchingEngine,
    response: &mut MatchingResponse,
    command: Command,
) -> OrderId {
    engine.execute_into(&command, response).unwrap();
    response.order_deltas[0].order_id
}

#[derive(Clone)]
struct Model {
    orders: Vec<Order>,
    next_id: u64,
    next_priority: u64,
}

impl Model {
    fn new() -> Self {
        Self {
            orders: Vec::new(),
            next_id: 1,
            next_priority: 0,
        }
    }

    fn identity(&mut self) -> (OrderId, u64) {
        let result = (OrderId::new(ENGINE_ID, self.next_id), self.next_priority);
        self.next_id += 1;
        self.next_priority += 1;
        result
    }

    fn level(&self, side: Side, price: Price) -> LevelDelta {
        let matching = self
            .orders
            .iter()
            .filter(|order| order.side == side && order.price == price);
        LevelDelta {
            side,
            price,
            size: matching.clone().map(|order| u128::from(order.size.0)).sum(),
            order_count: matching.count(),
        }
    }

    fn levels(&self, side: Side) -> Vec<LevelDelta> {
        let mut prices: Vec<_> = self
            .orders
            .iter()
            .filter(|order| order.side == side)
            .map(|order| order.price)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if side == Side::Buy {
            prices.reverse();
        }
        prices
            .into_iter()
            .map(|price| self.level(side, price))
            .collect()
    }

    fn active_levels(&self) -> usize {
        self.orders
            .iter()
            .map(|order| (order.side, order.price))
            .collect::<BTreeSet<_>>()
            .len()
    }

    fn would_cross(&self, side: Side, price: Price) -> bool {
        let opposite = self.levels(side.opposite());
        opposite.first().is_some_and(|best| match side {
            Side::Buy => price >= best.price,
            Side::Sell => price <= best.price,
        })
    }

    fn validate(price: Price, size: Quantity) -> Result<(), MatchingError> {
        if price.0 == 0 {
            return Err(MatchingError::InvalidPrice);
        }
        if !(1..=50).contains(&size.0) {
            return Err(MatchingError::InvalidSize);
        }
        Ok(())
    }

    fn owned(&self, account: AccountId, id: OrderId) -> Result<usize, MatchingError> {
        if id.engine_id != ENGINE_ID {
            return Err(MatchingError::WrongEngine);
        }
        let index = self
            .orders
            .iter()
            .position(|order| order.order_id == id)
            .ok_or(MatchingError::OrderNotFound)?;
        if self.orders[index].account_id != account {
            return Err(MatchingError::WrongOwner);
        }
        Ok(index)
    }

    fn execute(&mut self, command: Command) -> Result<MatchingResponse, MatchingError> {
        let mut response = MatchingResponse::default();
        match command {
            Command::Add {
                account_id,
                client_id,
                side,
                price,
                size,
            } => {
                Self::validate(price, size)?;
                if self
                    .orders
                    .iter()
                    .any(|order| order.account_id == account_id && order.client_id == client_id)
                {
                    return Err(MatchingError::DuplicateClientId);
                }
                if self.would_cross(side, price) {
                    return Err(MatchingError::WouldCross);
                }
                let level = self.level(side, price);
                if level.order_count == config().max_orders_per_level
                    || (level.order_count == 0 && self.active_levels() == config().max_levels)
                    || self.orders.len() == config().max_orders_per_level * config().max_levels
                {
                    return Err(MatchingError::CapacityExceeded);
                }
                let (order_id, sequence) = self.identity();
                self.orders.push(Order {
                    order_id,
                    account_id,
                    client_id,
                    side,
                    price,
                    size,
                    sequence,
                });
                response.order_deltas.push(OrderDelta {
                    order_id,
                    client_id,
                    status: OrderStatus::Open,
                    remaining_size: size,
                    filled_size: Quantity(0),
                    fill_price: None,
                });
                response.level_deltas.push(self.level(side, price));
            }
            Command::Cancel {
                account_id,
                order_id,
            } => {
                let index = self.owned(account_id, order_id)?;
                let old = self.orders.remove(index);
                response.order_deltas.push(OrderDelta {
                    order_id,
                    client_id: old.client_id,
                    status: OrderStatus::Cancelled,
                    remaining_size: Quantity(0),
                    filled_size: Quantity(0),
                    fill_price: None,
                });
                response.level_deltas.push(self.level(old.side, old.price));
            }
            Command::Amend {
                account_id,
                order_id,
                price,
                size,
            } => {
                let index = self.owned(account_id, order_id)?;
                Self::validate(price, size)?;
                let old = self.orders[index];
                if self.would_cross(old.side, price) {
                    return Err(MatchingError::WouldCross);
                }
                let mut resulting = self.clone();
                resulting.orders.remove(index);
                if resulting.level(old.side, price).order_count == config().max_orders_per_level
                    || (resulting.level(old.side, price).order_count == 0
                        && resulting.active_levels() == config().max_levels)
                {
                    return Err(MatchingError::CapacityExceeded);
                }
                self.orders.remove(index);
                let old_level = self.level(old.side, old.price);
                let (new_id, sequence) = self.identity();
                self.orders.push(Order {
                    order_id: new_id,
                    account_id,
                    client_id: old.client_id,
                    side: old.side,
                    price,
                    size,
                    sequence,
                });
                response.order_deltas.push(OrderDelta {
                    order_id,
                    client_id: old.client_id,
                    status: OrderStatus::Replaced,
                    remaining_size: Quantity(0),
                    filled_size: Quantity(0),
                    fill_price: None,
                });
                response.order_deltas.push(OrderDelta {
                    order_id: new_id,
                    client_id: old.client_id,
                    status: OrderStatus::Open,
                    remaining_size: size,
                    filled_size: Quantity(0),
                    fill_price: None,
                });
                response.level_deltas.push(old_level);
                response.level_deltas.push(self.level(old.side, price));
            }
            Command::Take {
                account_id,
                client_id,
                side,
                limit_price,
                size,
            } => {
                Self::validate(limit_price, size)?;
                let mut eligible: Vec<_> = self
                    .orders
                    .iter()
                    .filter(|order| {
                        order.side == side.opposite()
                            && match side {
                                Side::Buy => order.price <= limit_price,
                                Side::Sell => order.price >= limit_price,
                            }
                    })
                    .copied()
                    .collect();
                eligible.sort_by_key(|order| {
                    (
                        if side == Side::Buy {
                            order.price.0
                        } else {
                            u64::MAX - order.price.0
                        },
                        order.sequence,
                    )
                });
                let mut remaining = size.0;
                let mut plan = Vec::new();
                for maker in eligible {
                    if remaining == 0 {
                        break;
                    }
                    if maker.account_id == account_id {
                        return Err(MatchingError::SelfTrade);
                    }
                    let filled = maker.size.0.min(remaining);
                    remaining -= filled;
                    plan.push((maker, Quantity(filled)));
                }
                if plan.is_empty() {
                    return Err(MatchingError::NoLiquidity);
                }
                let (taker_id, _) = self.identity();
                response.order_deltas.push(OrderDelta {
                    order_id: taker_id,
                    client_id,
                    status: if remaining == 0 {
                        OrderStatus::Filled
                    } else {
                        OrderStatus::PartiallyFilled
                    },
                    remaining_size: Quantity(0),
                    filled_size: Quantity(size.0 - remaining),
                    fill_price: None,
                });
                let mut changed = Vec::new();
                for (maker, filled) in plan {
                    response.fills.push(Fill {
                        taker_order_id: taker_id,
                        taker_client_id: client_id,
                        taker_account_id: account_id,
                        maker_order_id: maker.order_id,
                        maker_client_id: maker.client_id,
                        maker_account_id: maker.account_id,
                        price: maker.price,
                        size: filled,
                        taker_side: side,
                    });
                    let left = maker.size.0 - filled.0;
                    response.order_deltas.push(OrderDelta {
                        order_id: maker.order_id,
                        client_id: maker.client_id,
                        status: if left == 0 {
                            OrderStatus::Filled
                        } else {
                            OrderStatus::PartiallyFilled
                        },
                        remaining_size: Quantity(left),
                        filled_size: filled,
                        fill_price: Some(maker.price),
                    });
                    let index = self
                        .orders
                        .iter()
                        .position(|order| order.order_id == maker.order_id)
                        .unwrap();
                    if left == 0 {
                        self.orders.remove(index);
                    } else {
                        self.orders[index].size = Quantity(left);
                    }
                    if !changed.contains(&(maker.side, maker.price)) {
                        changed.push((maker.side, maker.price));
                    }
                }
                response.level_deltas.extend(
                    changed
                        .into_iter()
                        .map(|(maker_side, price)| self.level(maker_side, price)),
                );
            }
        }
        Ok(response)
    }
}

fn assert_book(engine: &MatchingEngine, model: &Model) {
    for side in [Side::Buy, Side::Sell] {
        let expected = model.levels(side);
        assert_eq!(engine.depth(side, usize::MAX), expected);
        assert_eq!(
            engine.depth(side, 1),
            expected.iter().take(1).copied().collect::<Vec<_>>()
        );
        for level in expected {
            assert_eq!(engine.get_level(side, level.price), Some(level));
        }
    }
    assert_eq!(
        engine.best_bid(),
        model.levels(Side::Buy).first().map(|level| level.price)
    );
    assert_eq!(
        engine.best_ask(),
        model.levels(Side::Sell).first().map(|level| level.price)
    );
    for id in 1..model.next_id {
        let public_id = OrderId::new(ENGINE_ID, id);
        let expected = model
            .orders
            .iter()
            .find(|order| order.order_id == public_id);
        assert_eq!(engine.get_order(public_id), expected);
        if let Some(order) = expected {
            assert_eq!(
                engine.get_order_by_client_id(order.account_id, order.client_id),
                Some(order)
            );
            assert!(engine.has_order(order.account_id, order.client_id));
            assert!(order.sequence < model.next_priority);
        }
    }
    engine.validate_invariants().unwrap();
}

fn next_random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn generated_command(model: &Model, state: &mut u64, step: usize) -> Command {
    let draw = next_random(state);
    let side = if draw & 1 == 0 { Side::Buy } else { Side::Sell };
    let account = AccountId(1 + next_random(state) % 8);
    let client = ClientOrderId(1 + next_random(state) % 20);
    let size = Quantity(1 + next_random(state) % 12);
    let price = Price(if side == Side::Buy {
        96 + next_random(state) % 8
    } else {
        102 + next_random(state) % 8
    });
    match draw % 10 {
        0..=4 => add(account.0, client.0, side, price.0, size.0),
        5..=7 => {
            let limit = if side == Side::Buy {
                102 + next_random(state) % 8
            } else {
                96 + next_random(state) % 8
            };
            take(account.0, client.0, side, limit, size.0)
        }
        _ => {
            let chosen = if model.orders.is_empty() || step % 13 == 0 {
                None
            } else {
                let index = next_random(state) as usize % model.orders.len();
                Some(model.orders[index])
            };
            let owner = chosen.map_or(account, |order| {
                if step % 11 == 0 {
                    AccountId(order.account_id.0 + 100)
                } else {
                    order.account_id
                }
            });
            let id = chosen.map_or(OrderId::new(ENGINE_ID, model.next_id + 100), |order| {
                order.order_id
            });
            if draw % 10 == 8 {
                Command::Cancel {
                    account_id: owner,
                    order_id: id,
                }
            } else {
                Command::Amend {
                    account_id: owner,
                    order_id: id,
                    price: Price(96 + next_random(state) % 14),
                    size,
                }
            }
        }
    }
}

#[test]
fn seeded_command_stream_matches_independent_book_model() {
    for seed in [1_u64, 0x1234_5678_9abc_def0, 0xdead_beef_cafe_babe] {
        let mut state = seed;
        let mut engine = engine();
        let mut response = buffer();
        let mut model = Model::new();
        let mut accepted_takes = 0;
        let mut accepted_amends = 0;
        for step in 0..300 {
            let command = generated_command(&model, &mut state, step);
            let expected = model.execute(command);
            let actual = engine.execute_into(&command, &mut response);
            assert_eq!(
                actual,
                expected.as_ref().map(|_| ()).map_err(|error| *error),
                "seed={seed} step={step} command={command:?}"
            );
            match expected {
                Ok(expected_response) => {
                    assert_eq!(
                        response, expected_response,
                        "seed={seed} step={step} command={command:?}"
                    );
                    accepted_takes += usize::from(matches!(command, Command::Take { .. }));
                    accepted_amends += usize::from(matches!(command, Command::Amend { .. }));
                }
                Err(_) => assert_eq!(response, MatchingResponse::default()),
            }
            assert_book(&engine, &model);
        }
        assert!(
            accepted_takes > 10,
            "seed={seed} did not exercise enough takes"
        );
        assert!(
            accepted_amends > 5,
            "seed={seed} did not exercise enough amends"
        );
    }
}

#[test]
fn head_tail_cancels_and_partial_fill_preserve_fifo_on_both_sides() {
    for maker_side in [Side::Buy, Side::Sell] {
        let mut engine = engine();
        let mut response = buffer();
        let price = if maker_side == Side::Buy { 100 } else { 105 };
        let ids: Vec<_> = (1..=4)
            .map(|account| {
                place(
                    &mut engine,
                    &mut response,
                    add(account, account, maker_side, price, account + 2),
                )
            })
            .collect();
        for index in [0, 3] {
            engine
                .execute_into(
                    &Command::Cancel {
                        account_id: AccountId(index as u64 + 1),
                        order_id: ids[index],
                    },
                    &mut response,
                )
                .unwrap();
        }
        let last = place(&mut engine, &mut response, add(5, 5, maker_side, price, 7));
        engine
            .execute_into(&take(9, 9, maker_side.opposite(), price, 2), &mut response)
            .unwrap();
        assert_eq!(
            response
                .fills
                .iter()
                .map(|fill| fill.maker_order_id)
                .collect::<Vec<_>>(),
            vec![ids[1]]
        );
        assert_eq!(engine.get_order(ids[1]).unwrap().size, Quantity(2));
        engine
            .execute_into(&take(9, 10, maker_side.opposite(), price, 7), &mut response)
            .unwrap();
        assert_eq!(
            response
                .fills
                .iter()
                .map(|fill| fill.maker_order_id)
                .collect::<Vec<_>>(),
            vec![ids[1], ids[2]]
        );
        assert_eq!(
            response
                .fills
                .iter()
                .map(|fill| fill.size.0)
                .collect::<Vec<_>>(),
            vec![2, 5]
        );
        assert_eq!(engine.get_level(maker_side, Price(price)).unwrap().size, 7);
        assert_eq!(engine.get_order(last).unwrap().size, Quantity(7));
        engine.validate_invariants().unwrap();
    }
}

#[test]
fn self_trade_is_checked_only_for_reached_makers_and_late_rejection_is_atomic() {
    for maker_side in [Side::Buy, Side::Sell] {
        let mut engine = engine();
        let mut response = buffer();
        let (best, worse) = if maker_side == Side::Buy {
            (105, 100)
        } else {
            (100, 105)
        };
        let first = place(&mut engine, &mut response, add(2, 1, maker_side, best, 3));
        let own = place(&mut engine, &mut response, add(1, 2, maker_side, worse, 4));
        engine
            .execute_into(&take(1, 3, maker_side.opposite(), worse, 3), &mut response)
            .unwrap();
        assert_eq!(response.fills.len(), 1);
        assert_eq!(response.fills[0].maker_order_id, first);
        assert_eq!(engine.get_order(own).unwrap().size, Quantity(4));

        let next = place(&mut engine, &mut response, add(3, 4, maker_side, best, 2));
        let before = engine.depth(maker_side, usize::MAX);
        assert_eq!(
            engine.execute_into(&take(1, 5, maker_side.opposite(), worse, 3), &mut response),
            Err(MatchingError::SelfTrade)
        );
        assert_eq!(response, MatchingResponse::default());
        assert_eq!(engine.depth(maker_side, usize::MAX), before);
        assert_eq!(engine.get_order(next).unwrap().size, Quantity(2));
        assert_eq!(engine.get_order(own).unwrap().size, Quantity(4));
        let later = place(&mut engine, &mut response, add(4, 6, maker_side, best, 1));
        assert_eq!(later.sequence, next.sequence + 1);
        assert_eq!(
            engine.get_order(later).unwrap().sequence,
            engine.get_order(next).unwrap().sequence + 1
        );
        engine.validate_invariants().unwrap();
    }
}

#[test]
fn amending_a_partially_filled_maker_moves_it_behind_its_peer() {
    for maker_side in [Side::Buy, Side::Sell] {
        let mut engine = engine();
        let mut response = buffer();
        let price = 100;
        let first = place(&mut engine, &mut response, add(1, 1, maker_side, price, 5));
        let peer = place(&mut engine, &mut response, add(2, 2, maker_side, price, 5));
        engine
            .execute_into(&take(3, 3, maker_side.opposite(), price, 2), &mut response)
            .unwrap();
        assert_eq!(engine.get_order(first).unwrap().size, Quantity(3));
        engine
            .execute_into(
                &Command::Amend {
                    account_id: AccountId(1),
                    order_id: first,
                    price: Price(price),
                    size: Quantity(3),
                },
                &mut response,
            )
            .unwrap();
        let replacement = response.order_deltas[1].order_id;
        assert_eq!(engine.get_order(first), None);
        assert_eq!(
            engine
                .get_order_by_client_id(AccountId(1), ClientOrderId(1))
                .unwrap()
                .order_id,
            replacement
        );
        engine
            .execute_into(&take(3, 4, maker_side.opposite(), price, 6), &mut response)
            .unwrap();
        assert_eq!(
            response
                .fills
                .iter()
                .map(|fill| fill.maker_order_id)
                .collect::<Vec<_>>(),
            vec![peer, replacement]
        );
        assert_eq!(
            response
                .fills
                .iter()
                .map(|fill| fill.size.0)
                .collect::<Vec<_>>(),
            vec![5, 1]
        );
        assert_eq!(engine.get_order(replacement).unwrap().size, Quantity(2));
        engine.validate_invariants().unwrap();
    }
}
