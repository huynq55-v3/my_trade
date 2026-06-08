mod config;
mod market_data;
mod arbitrage;
mod execution;

use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::Notify;
use tracing::{info, error};
use rust_decimal_macros::dec;

use crate::config::Config;
use crate::market_data::OrderBookCache;
use crate::execution::ExecutionEngine;

#[tokio::main]
async fn main() {
    // 1. Initialize structured logging with millisecond timestamps
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    info!("Initializing Binance Spot Testnet Real-time Triangular Arbitrage Bot...");

    // 2. Load configuration from environment variables
    let config = Config::load_from_env();
    info!("Configuration loaded. Active triangles: {}", config.triangles.len());

    // 3. Initialize thread-safe cache and notification channels
    let cache = Arc::new(RwLock::new(OrderBookCache::default()));
    let notifier = Arc::new(Notify::new());
    let is_executing = Arc::new(AtomicBool::new(false));

    // 4. Fetch initial order book snapshots from REST to bootstrap cache
    let unique_symbols = config.get_unique_symbols();
    market_data::init_cache_snapshots(&config.rest_url, &unique_symbols, &cache).await;

    // 5. Initialize the execution engine (performs time sync and loads lot sizes / precision rules)
    let execution_engine = Arc::new(ExecutionEngine::new(&config).await);

    // 6. Spawn the WebSocket data ingestion task
    let ws_url = config.ws_url.clone();
    let symbols_clone = unique_symbols.clone();
    let cache_clone = cache.clone();
    let notifier_clone = notifier.clone();
    tokio::spawn(async move {
        market_data::start_websocket_ingestion(ws_url, symbols_clone, cache_clone, notifier_clone).await;
    });

    info!("WebSocket ingestion task spawned. Starting arbitrage loop...");

    let triangles = config.triangles.clone();
    let fee_rate = config.fee_rate;
    let min_profit_rate = config.min_profit_rate;
    
    // Binance Spot Testnet minimum notional limit is usually 10 USDT
    let min_usdt_order = dec!(10.0);

    // 7. Core calculation and decision loop
    loop {
        // Wait until notified of cache updates
        notifier.notified().await;

        // Skip calculations if an execution is currently in progress
        if is_executing.load(Ordering::Relaxed) {
            continue;
        }

        // Clone the cache state as quickly as possible to release the read lock
        let current_books = {
            let cache_lock = cache.read().expect("Failed to lock cache");
            cache_lock.books.clone()
        };

        // Scan all defined triangles
        for triangle in &triangles {
            if let Some(opportunity) = arbitrage::find_arbitrage_opportunity(
                triangle,
                &current_books,
                fee_rate,
                min_profit_rate,
                min_usdt_order,
            ) {
                let now_ms = chrono::Utc::now().timestamp_millis();
                info!(
                    "[OPPORTUNITY] Timestamp: {} | Path: {} | Optimal Volume: {} USDT | Expected Net Profit: {:.4}%",
                    now_ms,
                    opportunity.path_name,
                    opportunity.optimal_volume.normalize(),
                    opportunity.expected_profit_pct.normalize()
                );

                // Acquire the execution lock
                is_executing.store(true, Ordering::Relaxed);

                // Spawn the execution in a separate async thread so as not to block the calculation loop
                let engine = execution_engine.clone();
                let triangle_clone = triangle.clone();
                let is_executing_clone = is_executing.clone();

                tokio::spawn(async move {
                    if let Err(e) = engine.execute_triangular_trade(&triangle_clone, &opportunity).await {
                        error!("[TRADE FAILURE] Execution of triangle failed: {}", e);
                    } else {
                        info!("[TRADE SUCCESS] Successfully executed triangular trade!");
                    }
                    // Release the execution lock
                    is_executing_clone.store(false, Ordering::Relaxed);
                });
            }
        }
    }
}
