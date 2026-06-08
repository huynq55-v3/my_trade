use std::collections::HashMap;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use crate::config::{Triangle, TradeDirection};
use crate::market_data::OrderBook;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ArbitrageOpportunity {
    pub path_name: String,
    pub optimal_volume: Decimal,       // USDT input
    pub expected_profit_pct: Decimal,  // e.g. 0.25%
    pub expected_profit_usdt: Decimal, // profit in USDT
    pub leg1_raw_qty: Decimal,         // quantity to send to exchange for order 1
    pub leg2_raw_qty: Decimal,         // quantity to send to exchange for order 2
    pub leg3_raw_qty: Decimal,         // quantity to send to exchange for order 3
}

/// Simulates a single leg trade.
/// Returns (raw_received, net_received) where net_received is after fee deduction.
pub fn simulate_leg(
    volume_in: Decimal,
    direction: TradeDirection,
    book: &OrderBook,
    fee_rate: Decimal,
) -> Option<(Decimal, Decimal)> {
    if volume_in.is_zero() {
        return Some((Decimal::ZERO, Decimal::ZERO));
    }

    let mut remaining = volume_in;
    let mut accumulated = Decimal::ZERO;

    match direction {
        TradeDirection::Buy => {
            // We buy the base asset with the quote asset (e.g. pay USDT, get LTC).
            // We sweep the asks (sellers).
            for level in &book.asks {
                let level_price = level.price;
                let level_qty = level.quantity;
                let level_cost = level_price * level_qty;

                if remaining <= level_cost {
                    accumulated += remaining / level_price;
                    remaining = Decimal::ZERO;
                    break;
                } else {
                    accumulated += level_qty;
                    remaining -= level_cost;
                }
            }
            if remaining > Decimal::ZERO {
                return None; // Exceeded book depth
            }
            let net = accumulated * (Decimal::ONE - fee_rate);
            Some((accumulated, net))
        }
        TradeDirection::Sell => {
            // We sell the base asset for the quote asset (e.g. pay LTC, get BTC).
            // We sweep the bids (buyers).
            for level in &book.bids {
                let level_price = level.price;
                let level_qty = level.quantity;

                if remaining <= level_qty {
                    accumulated += remaining * level_price;
                    remaining = Decimal::ZERO;
                    break;
                } else {
                    accumulated += level_qty * level_price;
                    remaining -= level_qty;
                }
            }
            if remaining > Decimal::ZERO {
                return None; // Exceeded book depth
            }
            let net = accumulated * (Decimal::ONE - fee_rate);
            Some((accumulated, net))
        }
    }
}

/// Simulates the reverse execution of a leg.
/// Returns the raw input required to get a certain net output.
pub fn reverse_simulate_leg(
    desired_output: Decimal,
    direction: TradeDirection,
    book: &OrderBook,
    fee_rate: Decimal,
) -> Option<Decimal> {
    if desired_output.is_zero() {
        return Some(Decimal::ZERO);
    }

    // Output received is after fee: output_received = output_raw * (1 - fee_rate)
    // So output_raw = desired_output / (1 - fee_rate)
    let raw_needed = desired_output / (Decimal::ONE - fee_rate);
    let mut remaining = raw_needed;
    let mut accumulated_input = Decimal::ZERO;

    match direction {
        TradeDirection::Buy => {
            // We buy base asset. We sweep asks.
            // Remaining is the quantity of base asset we want to buy.
            for level in &book.asks {
                let level_price = level.price;
                let level_qty = level.quantity;

                if remaining <= level_qty {
                    accumulated_input += remaining * level_price;
                    remaining = Decimal::ZERO;
                    break;
                } else {
                    accumulated_input += level_qty * level_price;
                    remaining -= level_qty;
                }
            }
            if remaining > Decimal::ZERO {
                return None;
            }
            Some(accumulated_input)
        }
        TradeDirection::Sell => {
            // We sell base asset to get quote asset. We sweep bids.
            // Remaining is the quantity of quote asset we want to receive.
            for level in &book.bids {
                let level_price = level.price;
                let level_qty = level.quantity;
                let level_capacity = level_qty * level_price;

                if remaining <= level_capacity {
                    accumulated_input += remaining / level_price;
                    remaining = Decimal::ZERO;
                    break;
                } else {
                    accumulated_input += level_qty;
                    remaining -= level_capacity;
                }
            }
            if remaining > Decimal::ZERO {
                return None;
            }
            Some(accumulated_input)
        }
    }
}

/// Runs a forward simulation of the entire triangle.
/// Returns the final USDT output.
pub fn simulate_triangle(
    volume_in_usdt: Decimal,
    triangle: &Triangle,
    books: &HashMap<String, OrderBook>,
    fee_rate: Decimal,
) -> Option<Decimal> {
    let book1 = books.get(&triangle.leg1.symbol)?;
    let book2 = books.get(&triangle.leg2.symbol)?;
    let book3 = books.get(&triangle.leg3.symbol)?;

    // Leg 1: USDT -> A
    let (_, net1) = simulate_leg(volume_in_usdt, triangle.leg1.direction, book1, fee_rate)?;

    // Leg 2: A -> B
    let (_, net2) = simulate_leg(net1, triangle.leg2.direction, book2, fee_rate)?;

    // Leg 3: B -> USDT
    let (_, net3) = simulate_leg(net2, triangle.leg3.direction, book3, fee_rate)?;

    Some(net3)
}

/// Computes the maximum USDT capacity of the triangle based on the top 5 levels.
pub fn calculate_max_usdt_capacity(
    triangle: &Triangle,
    books: &HashMap<String, OrderBook>,
    fee_rate: Decimal,
) -> Option<Decimal> {
    let book1 = books.get(&triangle.leg1.symbol)?;
    let book2 = books.get(&triangle.leg2.symbol)?;
    let book3 = books.get(&triangle.leg3.symbol)?;

    // Leg 1 max capacity: sum of cost of all asks
    let v1_max: Decimal = book1.asks.iter().map(|l| l.price * l.quantity).sum();

    // Leg 2 max capacity: translated to input USDT
    let v2_max = match triangle.leg2.direction {
        TradeDirection::Sell => {
            // We sell A. Max A we can sell is sum of bid quantities on Leg 2.
            let a_max: Decimal = book2.bids.iter().map(|l| l.quantity).sum();
            reverse_simulate_leg(a_max, triangle.leg1.direction, book1, fee_rate).unwrap_or(Decimal::MAX)
        }
        TradeDirection::Buy => {
            // We buy B. Max B we can buy is sum of ask quantities on Leg 2.
            let b_max: Decimal = book2.asks.iter().map(|l| l.quantity).sum();
            if let Some(a_needed) = reverse_simulate_leg(b_max, triangle.leg2.direction, book2, fee_rate) {
                reverse_simulate_leg(a_needed, triangle.leg1.direction, book1, fee_rate).unwrap_or(Decimal::MAX)
            } else {
                Decimal::MAX
            }
        }
    };

    // Leg 3 max capacity: translated to input USDT
    let v3_max = {
        // We sell B. Max B we can sell is sum of bid quantities on Leg 3.
        let b_max: Decimal = book3.bids.iter().map(|l| l.quantity).sum();
        if let Some(a_needed) = reverse_simulate_leg(b_max, triangle.leg2.direction, book2, fee_rate) {
            reverse_simulate_leg(a_needed, triangle.leg1.direction, book1, fee_rate).unwrap_or(Decimal::MAX)
        } else {
            Decimal::MAX
        }
    };

    let cap = v1_max.min(v2_max).min(v3_max);
    Some(cap)
}

/// Finds the optimal input volume to maximize profit, returning details.
pub fn find_arbitrage_opportunity(
    triangle: &Triangle,
    books: &HashMap<String, OrderBook>,
    fee_rate: Decimal,
    min_profit_rate: Decimal,
    min_usdt_order: Decimal,
) -> Option<ArbitrageOpportunity> {
    let max_cap = calculate_max_usdt_capacity(triangle, books, fee_rate)?;
    if max_cap < min_usdt_order {
        return None;
    }

    let mut best_profit_usdt = Decimal::ZERO;
    let mut best_opportunity: Option<ArbitrageOpportunity> = None;

    // Grid search to locate the optimal execution size
    let steps = 30;
    let step_size = (max_cap - min_usdt_order) / Decimal::from(steps);

    for i in 0..=steps {
        let volume_in = min_usdt_order + step_size * Decimal::from(i);
        if let Some(volume_out) = simulate_triangle(volume_in, triangle, books, fee_rate) {
            let profit_usdt = volume_out - volume_in;
            let profit_pct = profit_usdt / volume_in;

            if profit_pct > min_profit_rate && profit_usdt > best_profit_usdt {
                let book1 = books.get(&triangle.leg1.symbol)?;
                let book2 = books.get(&triangle.leg2.symbol)?;

                // For execution, we need the exact raw quantities to place:
                // Order 1: BUY A/USDT. Quantity: raw quantity to buy.
                let (leg1_raw_qty, leg1_net_qty) = simulate_leg(volume_in, triangle.leg1.direction, book1, fee_rate)?;

                // Order 2: A -> B
                let (leg2_raw_qty, leg2_net_qty) = match triangle.leg2.direction {
                    TradeDirection::Sell => {
                        // SELL A/B. We sell leg1_net_qty of A.
                        // Order quantity is leg1_net_qty.
                        // Output is B.
                        let (_raw, net) = simulate_leg(leg1_net_qty, TradeDirection::Sell, book2, fee_rate)?;
                        (leg1_net_qty, net)
                    }
                    TradeDirection::Buy => {
                        // BUY B/A. We spend leg1_net_qty of A to buy B.
                        // Order quantity is the raw amount of B we buy (raw).
                        // Output is B.
                        let (raw, net) = simulate_leg(leg1_net_qty, TradeDirection::Buy, book2, fee_rate)?;
                        (raw, net)
                    }
                };

                // Order 3: B -> USDT (SELL B/USDT).
                // We sell leg2_net_qty of B.
                // Order quantity is leg2_net_qty.
                let leg3_raw_qty = leg2_net_qty;

                best_profit_usdt = profit_usdt;
                best_opportunity = Some(ArbitrageOpportunity {
                    path_name: triangle.name.clone(),
                    optimal_volume: volume_in,
                    expected_profit_pct: profit_pct * dec!(100.0), // in %
                    expected_profit_usdt: profit_usdt,
                    leg1_raw_qty,
                    leg2_raw_qty,
                    leg3_raw_qty,
                });
            }
        }
    }

    best_opportunity
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market_data::OrderBookLevel;

    fn mock_buy_book() -> OrderBook {
        OrderBook {
            bids: vec![],
            asks: vec![
                OrderBookLevel {
                    price: dec!(100.0),
                    quantity: dec!(1.0),
                },
                OrderBookLevel {
                    price: dec!(101.0),
                    quantity: dec!(2.0),
                },
            ],
            last_update_id: 1,
        }
    }

    fn mock_sell_book() -> OrderBook {
        OrderBook {
            bids: vec![
                OrderBookLevel {
                    price: dec!(99.0),
                    quantity: dec!(1.0),
                },
                OrderBookLevel {
                    price: dec!(98.0),
                    quantity: dec!(2.0),
                },
            ],
            asks: vec![],
            last_update_id: 1,
        }
    }

    #[test]
    fn test_simulate_leg_buy() {
        let book = mock_buy_book();
        let fee = dec!(0.001); // 0.1%

        // Spend 100 USDT (exactly matches level 0)
        let res = simulate_leg(dec!(100.0), TradeDirection::Buy, &book, fee).unwrap();
        assert_eq!(res.0, dec!(1.0)); // raw A
        assert_eq!(res.1, dec!(0.999)); // net A

        // Spend 201 USDT (fills level 0 completely and partially level 1: 100 + 101 = 201)
        let res2 = simulate_leg(dec!(201.0), TradeDirection::Buy, &book, fee).unwrap();
        assert_eq!(res2.0, dec!(2.0)); // raw A
        assert_eq!(res2.1, dec!(1.998)); // net A

        // Spend 500 USDT (exceeds total depth of 1*100 + 2*101 = 302)
        let res3 = simulate_leg(dec!(500.0), TradeDirection::Buy, &book, fee);
        assert!(res3.is_none());
    }

    #[test]
    fn test_simulate_leg_sell() {
        let book = mock_sell_book();
        let fee = dec!(0.001); // 0.1%

        // Sell 1.0 base asset (fills level 0)
        let res = simulate_leg(dec!(1.0), TradeDirection::Sell, &book, fee).unwrap();
        assert_eq!(res.0, dec!(99.0)); // raw USDT
        assert_eq!(res.1, dec!(98.901)); // net USDT (99 * 0.999)

        // Sell 2.0 base asset (fills level 0 completely, and 1.0 of level 1: 1*99 + 1*98 = 197)
        let res2 = simulate_leg(dec!(2.0), TradeDirection::Sell, &book, fee).unwrap();
        assert_eq!(res2.0, dec!(197.0)); // raw USDT
        assert_eq!(res2.1, dec!(196.803)); // net USDT (197 * 0.999)

        // Sell 5.0 base asset (exceeds total depth of 1 + 2 = 3)
        let res3 = simulate_leg(dec!(5.0), TradeDirection::Sell, &book, fee);
        assert!(res3.is_none());
    }

    #[test]
    fn test_reverse_simulate_leg() {
        let buy_book = mock_buy_book();
        let sell_book = mock_sell_book();
        let fee = dec!(0.001);

        // Desired output from BUY: 0.999 net base asset
        let input1 = reverse_simulate_leg(dec!(0.999), TradeDirection::Buy, &buy_book, fee).unwrap();
        assert_eq!(input1, dec!(100.0)); // Should cost 100 USDT

        // Desired output from SELL: 98.901 net quote asset
        let input2 = reverse_simulate_leg(dec!(98.901), TradeDirection::Sell, &sell_book, fee).unwrap();
        assert_eq!(input2, dec!(1.0)); // Should require selling 1.0 base asset
    }

    #[test]
    fn test_calculate_max_usdt_capacity() {
        use crate::config::Leg;

        let triangle = Triangle {
            name: "USDT->A->B->USDT".to_string(),
            leg1: Leg {
                symbol: "AUSDT".to_string(),
                base_asset: "A".to_string(),
                quote_asset: "USDT".to_string(),
                direction: TradeDirection::Buy,
            },
            leg2: Leg {
                symbol: "AB".to_string(),
                base_asset: "A".to_string(),
                quote_asset: "B".to_string(),
                direction: TradeDirection::Sell,
            },
            leg3: Leg {
                symbol: "BUSDT".to_string(),
                base_asset: "B".to_string(),
                quote_asset: "USDT".to_string(),
                direction: TradeDirection::Sell,
            },
        };

        let mut books = HashMap::new();
        // Leg 1: Buy A/USDT.
        // Asks: price 100, qty 1; price 101, qty 2. Max cost = 1*100 + 2*101 = 302.
        books.insert("AUSDT".to_string(), mock_buy_book());

        // Leg 2: Sell A/B.
        // Bids: price 0.1, qty 0.5. (We can sell up to 0.5 A)
        books.insert("AB".to_string(), OrderBook {
            bids: vec![
                OrderBookLevel {
                    price: dec!(0.1),
                    quantity: dec!(0.5),
                }
            ],
            asks: vec![],
            last_update_id: 1,
        });

        // Leg 3: Sell B/USDT.
        // Bids: price 1000, qty 10.0 (very deep book, would cause None in old code)
        books.insert("BUSDT".to_string(), OrderBook {
            bids: vec![
                OrderBookLevel {
                    price: dec!(1000.0),
                    quantity: dec!(10.0),
                }
            ],
            asks: vec![],
            last_update_id: 1,
        });

        let fee_rate = dec!(0.001);
        let cap = calculate_max_usdt_capacity(&triangle, &books, fee_rate);
        assert!(cap.is_some(), "Capacity should not be None");
        let cap_val = cap.unwrap();
        assert!(cap_val < dec!(51.0));
        assert!(cap_val > dec!(50.0));
    }
}

