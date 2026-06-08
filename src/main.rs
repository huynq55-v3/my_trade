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

    info!("Initializing Binance Spot Real-time Triangular Arbitrage Bot...");

    // 2. Load configuration from environment variables
    let mut config = Config::load_from_env();
    
    // 2.5 Discover target niche paths dynamically based on 24h quote trading volume
    info!(
        "Discovering target niche triangles (24h Volume limit: {} to {} USDT)...",
        config.min_24h_volume.normalize(), config.max_24h_volume.normalize()
    );
    let discovered_triangles = market_data::discover_triangles(
        &config.rest_url,
        config.min_24h_volume,
        config.max_24h_volume,
    )
    .await
    .unwrap_or_else(|e| {
        panic!("Failed to dynamically discover triangular trading paths: {:?}", e);
    });

    if discovered_triangles.is_empty() {
        error!("No trading paths discovered matching the volume criteria. Exiting bot.");
        return;
    }

    info!(
        "Successfully whitelisted {} triangular paths:",
        discovered_triangles.len()
    );
    for t in &discovered_triangles {
        info!("  - {}", t.name);
    }
    
    config.triangles = discovered_triangles;

    // 3. Initialize thread-safe cache and notification channels
    let cache = Arc::new(RwLock::new(OrderBookCache::default()));
    let notifier = Arc::new(Notify::new());
    let is_executing = Arc::new(AtomicBool::new(false));

    // 4. Fetch initial order book snapshots from REST to bootstrap cache
    let unique_symbols = config.get_unique_symbols();
    market_data::init_cache_snapshots(&config.rest_url, &unique_symbols, &cache).await;

    // 5. Initialize the execution engine (performs time sync and loads lot sizes / precision rules)
    let execution_engine = Arc::new(ExecutionEngine::new(&config).await);

    // 6. Spawn the WebSocket data ingestion task (TRẢ LẠI LUỒNG WEBSOCKET CHUẨN)
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
    
    // Trong src/main.rs - Sửa lại mức test nhỏ hơn
    let min_usdt_order = dec!(1.0);

    let mut last_debug_print = 0;

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

        let now_sec = chrono::Utc::now().timestamp();
        let should_debug = now_sec - last_debug_print >= 10;
        if should_debug {
            last_debug_print = now_sec;
        }

        // Scan all defined triangles
        for (idx, triangle) in triangles.iter().enumerate() {
            let has_leg1 = current_books.contains_key(&triangle.leg1.symbol);
            let has_leg2 = current_books.contains_key(&triangle.leg2.symbol);
            let has_leg3 = current_books.contains_key(&triangle.leg3.symbol);

            if should_debug && (idx == 0 || triangle.name == "USDT->KNC->BTC->USDT") {
                info!(
                    "DEBUG CACHE - Path: {} | Leg1={}({}), Leg2={}({}), Leg3={}({})", 
                    triangle.name,
                    triangle.leg1.symbol, has_leg1,
                    triangle.leg2.symbol, has_leg2,
                    triangle.leg3.symbol, has_leg3
                );
            }

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

                // ĐƯA ĐOẠN MONITOR ONLY VÀO ĐÂY ĐỂ CHỈ THEO DÕI, KHÔNG ĐẶT LỆNH THẬT
                let triangle_clone = triangle.clone();
                let opportunity_clone = opportunity.clone();
                let is_executing_clone = is_executing.clone();

                tokio::spawn(async move {
                    info!(
                        "[MONITOR ONLY] Tìm thấy cặp ngách tiềm năng cực ngon: {} | Lãi dự kiến sau phí: {:.4}%", 
                        triangle_clone.name, 
                        opportunity_clone.expected_profit_pct.normalize()
                    );
                    // Giải phóng lock ngay lập tức để tiếp tục vòng lặp lắng nghe
                    is_executing_clone.store(false, Ordering::Relaxed);
                });
            }
        }
    }
}