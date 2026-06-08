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
    pub min_24h_volume: Decimal,     // e.g., 50,000 USDT
    pub max_24h_volume: Decimal,     // e.g., 500,000 USDT
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

        let min_24h_volume = env::var("MIN_24H_VOLUME")
            .ok()
            .and_then(|val| val.parse::<Decimal>().ok())
            .unwrap_or(dec!(50000.0)); // Default $50,000

        let max_24h_volume = env::var("MAX_24H_VOLUME")
            .ok()
            .and_then(|val| val.parse::<Decimal>().ok())
            .unwrap_or(dec!(500000.0)); // Default $500,000

        Self {
            api_key,
            secret_key,
            rest_url,
            ws_url,
            fee_rate,
            min_profit_rate,
            min_24h_volume,
            max_24h_volume,
            triangles: Vec::new(),
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
