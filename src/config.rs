use std::env;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeDirection {
    Buy,
    Sell,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Leg {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub direction: TradeDirection,
}

#[derive(Debug, Clone)]
pub struct Triangle {
    pub name: String,
    pub leg1: Leg,
    pub leg2: Leg,
    pub leg3: Leg,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub api_key: String,
    pub secret_key: String,
    pub rest_url: String,
    pub ws_url: String,
    pub fee_rate: Decimal,           // e.g., 0.001 for 0.1% fee
    pub min_profit_rate: Decimal,    // e.g., 0.002 for 0.2% net profit
    pub triangles: Vec<Triangle>,
}

impl Config {
    pub fn load_from_env() -> Self {
        dotenvy::dotenv().ok();

        let api_key = env::var("BINANCE_API_KEY")
            .expect("BINANCE_API_KEY environment variable is required");
        let secret_key = env::var("BINANCE_SECRET_KEY")
            .expect("BINANCE_SECRET_KEY environment variable is required");

        let rest_url = env::var("BINANCE_REST_URL")
            .unwrap_or_else(|_| "https://testnet.binance.vision".to_string());
        
        let ws_url = env::var("BINANCE_WS_URL")
            .unwrap_or_else(|_| "wss://stream.testnet.binance.vision/stream".to_string());

        let fee_rate = env::var("TRADING_FEE_RATE")
            .ok()
            .and_then(|val| val.parse::<Decimal>().ok())
            .unwrap_or(dec!(0.001)); // Default 0.1%

        let min_profit_rate = env::var("MIN_PROFIT_RATE")
            .ok()
            .and_then(|val| val.parse::<Decimal>().ok())
            .unwrap_or(dec!(0.002)); // Default 0.2%

        // Define our target triangles.
        // Format: USDT -> Target_Coin_A -> Target_Coin_B -> USDT
        // Standard symbols on Binance Testnet:
        // A/USDT, B/USDT, and either A/B or B/A.
        // Let's create the following list of triangles:
        // 1. USDT -> LTC -> BTC -> USDT
        //    Leg 1: Buy LTCUSDT (USDT -> LTC)
        //    Leg 2: Sell LTCBTC (LTC -> BTC)
        //    Leg 3: Sell BTCUSDT (BTC -> USDT)
        // 2. USDT -> ETH -> BTC -> USDT
        //    Leg 1: Buy ETHUSDT (USDT -> ETH)
        //    Leg 2: Sell ETHBTC (ETH -> BTC)
        //    Leg 3: Sell BTCUSDT (BTC -> USDT)
        // 3. USDT -> ADA -> BTC -> USDT
        //    Leg 1: Buy ADAUSDT (USDT -> ADA)
        //    Leg 2: Sell ADABTC (ADA -> BTC)
        //    Leg 3: Sell BTCUSDT (BTC -> USDT)
        // 4. USDT -> XRP -> BTC -> USDT
        //    Leg 1: Buy XRPUSDT (USDT -> XRP)
        //    Leg 2: Sell XRPBTC (XRP -> BTC)
        //    Leg 3: Sell BTCUSDT (BTC -> USDT)
        
        let triangles = vec![
            Triangle {
                name: "USDT->LTC->BTC->USDT".to_string(),
                leg1: Leg {
                    symbol: "LTCUSDT".to_string(),
                    base_asset: "LTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Buy,
                },
                leg2: Leg {
                    symbol: "LTCBTC".to_string(),
                    base_asset: "LTC".to_string(),
                    quote_asset: "BTC".to_string(),
                    direction: TradeDirection::Sell,
                },
                leg3: Leg {
                    symbol: "BTCUSDT".to_string(),
                    base_asset: "BTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Sell,
                },
            },
            Triangle {
                name: "USDT->ETH->BTC->USDT".to_string(),
                leg1: Leg {
                    symbol: "ETHUSDT".to_string(),
                    base_asset: "ETH".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Buy,
                },
                leg2: Leg {
                    symbol: "ETHBTC".to_string(),
                    base_asset: "ETH".to_string(),
                    quote_asset: "BTC".to_string(),
                    direction: TradeDirection::Sell,
                },
                leg3: Leg {
                    symbol: "BTCUSDT".to_string(),
                    base_asset: "BTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Sell,
                },
            },
            Triangle {
                name: "USDT->ADA->BTC->USDT".to_string(),
                leg1: Leg {
                    symbol: "ADAUSDT".to_string(),
                    base_asset: "ADA".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Buy,
                },
                leg2: Leg {
                    symbol: "ADABTC".to_string(),
                    base_asset: "ADA".to_string(),
                    quote_asset: "BTC".to_string(),
                    direction: TradeDirection::Sell,
                },
                leg3: Leg {
                    symbol: "BTCUSDT".to_string(),
                    base_asset: "BTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Sell,
                },
            },
            Triangle {
                name: "USDT->XRP->BTC->USDT".to_string(),
                leg1: Leg {
                    symbol: "XRPUSDT".to_string(),
                    base_asset: "XRP".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Buy,
                },
                leg2: Leg {
                    symbol: "XRPBTC".to_string(),
                    base_asset: "XRP".to_string(),
                    quote_asset: "BTC".to_string(),
                    direction: TradeDirection::Sell,
                },
                leg3: Leg {
                    symbol: "BTCUSDT".to_string(),
                    base_asset: "BTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Sell,
                },
            },
        ];

        Self {
            api_key,
            secret_key,
            rest_url,
            ws_url,
            fee_rate,
            min_profit_rate,
            triangles,
        }
    }

    /// Helper to get all unique symbols that need subscription
    pub fn get_unique_symbols(&self) -> Vec<String> {
        let mut symbols = Vec::new();
        for t in &self.triangles {
            if !symbols.contains(&t.leg1.symbol) {
                symbols.push(t.leg1.symbol.clone());
            }
            if !symbols.contains(&t.leg2.symbol) {
                symbols.push(t.leg2.symbol.clone());
            }
            if !symbols.contains(&t.leg3.symbol) {
                symbols.push(t.leg3.symbol.clone());
            }
        }
        symbols
    }
}
