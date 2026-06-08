use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use reqwest::Client;

use serde::Deserialize;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use tracing::{info, error, warn};
use tokio::time::{sleep, Duration};

use crate::config::{Config, Triangle, TradeDirection};
use crate::arbitrage::ArbitrageOpportunity;

type HmacSha256 = Hmac<Sha256>;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct SymbolRules {
    pub step_size: Decimal,
    pub min_qty: Decimal,
    pub min_notional: Decimal,
}

pub struct ExecutionEngine {
    client: Client,
    api_key: String,
    secret_key: String,
    rest_url: String,
    pub server_time_offset: i64, // Server time - Local time (ms)
    pub rules: HashMap<String, SymbolRules>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimeResponse {
    server_time: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeInfoResponse {
    symbols: Vec<SymbolInfo>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolInfo {
    symbol: String,
    filters: Vec<serde_json::Value>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderResponse {
    pub symbol: String,
    pub order_id: u64,
    pub client_order_id: String,
    pub transact_time: u64,
    pub price: String,
    pub orig_qty: String,
    pub executed_qty: String,
    pub cummulative_quote_qty: String,
    pub status: String,
}

impl ExecutionEngine {
    pub async fn new(config: &Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to build HTTP Client");

        let mut engine = Self {
            client,
            api_key: config.api_key.clone(),
            secret_key: config.secret_key.clone(),
            rest_url: config.rest_url.clone(),
            server_time_offset: 0,
            rules: HashMap::new(),
        };

        // 1. Sync time
        engine.sync_time().await;

        // 2. Fetch exchange rules
        engine.load_rules(config.get_unique_symbols()).await;

        engine
    }

    /// Syncs time offset with Binance server
    async fn sync_time(&mut self) {
        let url = format!("{}/api/v3/time", self.rest_url);
        let local_before = Self::current_time_ms();
        
        let response = self.client.get(&url)
            .send()
            .await
            .expect("Failed to fetch Binance server time")
            .json::<TimeResponse>()
            .await
            .expect("Failed to parse server time response");

        let local_after = Self::current_time_ms();
        let local_midpoint = (local_before + local_after) / 2;
        self.server_time_offset = response.server_time - local_midpoint;

        info!(
            "Server time synced. Offset: {}ms (Local midpoint to server)",
            self.server_time_offset
        );
    }

    /// Load constraints (LOT_SIZE, NOTIONAL) for symbols
    async fn load_rules(&mut self, target_symbols: Vec<String>) {
        let url = format!("{}/api/v3/exchangeInfo", self.rest_url);
        
        let response = self.client.get(&url)
            .send()
            .await
            .expect("Failed to fetch exchange info")
            .json::<ExchangeInfoResponse>()
            .await
            .expect("Failed to parse exchange info JSON");

        for sym_info in response.symbols {
            if !target_symbols.contains(&sym_info.symbol) {
                continue;
            }

            let mut step_size = dec!(0.00000001);
            let mut min_qty = dec!(0.00000001);
            let mut min_notional = dec!(10.0); // Safe fallback

            for filter in sym_info.filters {
                let filter_type = filter.get("filterType").and_then(|v| v.as_str()).unwrap_or("");
                match filter_type {
                    "LOT_SIZE" => {
                        if let Some(s) = filter.get("stepSize").and_then(|v| v.as_str()) {
                            step_size = s.parse::<Decimal>().unwrap_or(step_size);
                        }
                        if let Some(m) = filter.get("minQty").and_then(|v| v.as_str()) {
                            min_qty = m.parse::<Decimal>().unwrap_or(min_qty);
                        }
                    }
                    "NOTIONAL" | "MIN_NOTIONAL" => {
                        if let Some(n) = filter.get("minNotional").and_then(|v| v.as_str()) {
                            min_notional = n.parse::<Decimal>().unwrap_or(min_notional);
                        }
                    }
                    _ => {}
                }
            }

            self.rules.insert(
                sym_info.symbol.clone(),
                SymbolRules {
                    step_size,
                    min_qty,
                    min_notional,
                },
            );

            info!(
                "Loaded rules for {}: step_size={}, min_qty={}, min_notional={}",
                sym_info.symbol, step_size, min_qty, min_notional
            );
        }
    }

    /// Helper to get current epoch time in milliseconds
    fn current_time_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("System time is backward")
            .as_millis() as i64
    }

    fn sign_query(&self, query: &str) -> String {
        let mut mac = HmacSha256::new_from_slice(self.secret_key.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(query.as_bytes());
        let result = mac.finalize();
        let code_bytes = result.into_bytes();
        hex::encode(code_bytes)
    }

    /// Rounds quantity down according to step_size
    pub fn round_quantity(&self, symbol: &str, qty: Decimal) -> Decimal {
        if let Some(rule) = self.rules.get(symbol) {
            let step = rule.step_size;
            if step.is_zero() {
                return qty;
            }
            // Rounded down to multiple of step size
            let steps = (qty / step).floor();
            steps * step
        } else {
            qty
        }
    }

    /// Places a single market order on Binance Testnet
    pub async fn place_market_order(
        &self,
        symbol: &str,
        direction: TradeDirection,
        quantity: Decimal,
    ) -> Result<OrderResponse, String> {
        let side_str = match direction {
            TradeDirection::Buy => "BUY",
            TradeDirection::Sell => "SELL",
        };

        // Format quantity with enough decimals, stripping trailing zeros
        let rounded_qty = self.round_quantity(symbol, quantity);
        let qty_str = rounded_qty.normalize().to_string();

        let timestamp = Self::current_time_ms() + self.server_time_offset;
        let query = format!(
            "symbol={}&side={}&type=MARKET&quantity={}&timestamp={}&recvWindow=5000",
            symbol, side_str, qty_str, timestamp
        );
        let signature = self.sign_query(&query);
        let url = format!("{}/api/v3/order?{}&signature={}", self.rest_url, query, signature);

        info!("Sending order: POST {}", url);

        let res = self.client.post(&url)
            .header("X-MBX-APIKEY", &self.api_key)
            .send()
            .await
            .map_err(|e| format!("Network request failed: {:?}", e))?;

        let status = res.status();
        let body = res.text().await.map_err(|e| format!("Failed to read response body: {:?}", e))?;

        if !status.is_success() {
            error!("Order failed status={}: {}", status, body);
            return Err(format!("Binance API Error: status={}, response={}", status, body));
        }

        let order_res: OrderResponse = serde_json::from_str(&body).map_err(|e| {
            format!("Failed to parse order response: {} (raw body: {})", e, body)
        })?;

        Ok(order_res)
    }

    /// Executes the full triangular arbitrage trade sequentially
    pub async fn execute_triangular_trade(
        &self,
        triangle: &Triangle,
        opportunity: &ArbitrageOpportunity,
    ) -> Result<(), String> {
        info!(
            "[EXECUTION START] Executing triangle {} starting with input USDT={}",
            triangle.name, opportunity.optimal_volume
        );

        // --- Leg 1 ---
        info!(
            "Leg 1: {} {:?} Qty={}",
            triangle.leg1.symbol, triangle.leg1.direction, opportunity.leg1_raw_qty
        );
        let res_leg1 = self.place_market_order(
            &triangle.leg1.symbol,
            triangle.leg1.direction,
            opportunity.leg1_raw_qty,
        ).await;

        let order1 = match res_leg1 {
            Ok(ord) => {
                info!("Leg 1 filled successfully. OrderId: {}", ord.order_id);
                ord
            }
            Err(e) => {
                let err_msg = format!("[CRITICAL] Leg 1 failed to execute! Halting: {:?}", e);
                error!("{}", err_msg);
                return Err(err_msg);
            }
        };

        // --- Leg 2 ---
        info!(
            "Leg 2: {} {:?} Qty={}",
            triangle.leg2.symbol, triangle.leg2.direction, opportunity.leg2_raw_qty
        );
        let res_leg2 = self.place_market_order(
            &triangle.leg2.symbol,
            triangle.leg2.direction,
            opportunity.leg2_raw_qty,
        ).await;

        let order2 = match res_leg2 {
            Ok(ord) => {
                info!("Leg 2 filled successfully. OrderId: {}", ord.order_id);
                ord
            }
            Err(e) => {
                let err_msg = format!(
                    "[CRITICAL EMERGENCY] Leg 2 failed! Leg 1 filled successfully (OrderId={}). capital is now in intermediate asset! {:?}",
                    order1.order_id, e
                );
                error!("{}", err_msg);
                return Err(err_msg);
            }
        };

        // --- Leg 3 (with Emergency Rescue Loop) ---
        info!(
            "Leg 3: {} {:?} Qty={}",
            triangle.leg3.symbol, triangle.leg3.direction, opportunity.leg3_raw_qty
        );

        let mut leg3_success = false;
        let mut retry_count = 0;
        let max_retries = 3;

        while retry_count <= max_retries {
            let res_leg3 = self.place_market_order(
                &triangle.leg3.symbol,
                triangle.leg3.direction,
                opportunity.leg3_raw_qty,
            ).await;

            match res_leg3 {
                Ok(ord) => {
                    info!("Leg 3 filled successfully. OrderId: {}", ord.order_id);
                    leg3_success = true;
                    break;
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count <= max_retries {
                        warn!(
                            "[RESCUE] Leg 3 failed ({:?}). Retrying count={}/{} in 500ms...",
                            e, retry_count, max_retries
                        );
                        sleep(Duration::from_millis(500)).await;
                    } else {
                        error!(
                            "[CRITICAL EMERGENCY ALERT] Leg 3 failed after {} retries! Leg 1 (OrderId={}) and Leg 2 (OrderId={}) executed successfully. Capital is trapped in intermediate asset {}!",
                            max_retries, order1.order_id, order2.order_id, triangle.leg3.base_asset
                        );
                    }
                }
            }
        }

        if !leg3_success {
            return Err("[EMERGENCY] Leg 3 failed completely. Capital trapped.".to_string());
        }

        info!("[EXECUTION COMPLETE] Triangle {} completed successfully!", triangle.name);
        Ok(())
    }
}
