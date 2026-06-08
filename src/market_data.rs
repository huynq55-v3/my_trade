use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use serde::{Deserialize, Serialize};
use rust_decimal::Decimal;
use tokio::sync::Notify;
use tokio::time::{sleep, Duration};
use futures_util::StreamExt;
use tracing::{info, error};
use crate::config::Triangle;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OrderBookLevel {
    pub price: Decimal,
    pub quantity: Decimal,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Default)]
pub struct OrderBook {
    pub bids: Vec<OrderBookLevel>,
    pub asks: Vec<OrderBookLevel>,
    pub last_update_id: u64,
}

#[derive(Clone, Debug, Default)]
pub struct OrderBookCache {
    pub books: HashMap<String, OrderBook>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepthPayload {
    pub last_update_id: u64,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
}

#[derive(Debug, Deserialize)]
pub struct CombinedStreamMessage {
    pub stream: String,
    pub data: DepthPayload,
}

impl DepthPayload {
    pub fn to_order_book(&self) -> Result<OrderBook, rust_decimal::Error> {
        let mut bids = Vec::with_capacity(self.bids.len());
        for raw in &self.bids {
            let price = raw[0].parse::<Decimal>()?;
            let quantity = raw[1].parse::<Decimal>()?;
            bids.push(OrderBookLevel { price, quantity });
        }

        let mut asks = Vec::with_capacity(self.asks.len());
        for raw in &self.asks {
            let price = raw[0].parse::<Decimal>()?;
            let quantity = raw[1].parse::<Decimal>()?;
            asks.push(OrderBookLevel { price, quantity });
        }

        Ok(OrderBook {
            bids,
            asks,
            last_update_id: self.last_update_id,
        })
    }
}

/// Initializes cache by fetching a full order book snapshot for all symbols
pub async fn init_cache_snapshots(
    rest_url: &str,
    symbols: &[String],
    cache: &Arc<RwLock<OrderBookCache>>,
) {
    let client = reqwest::Client::new();
    for symbol in symbols {
        let url = format!("{}/api/v3/depth?symbol={}&limit=5", rest_url, symbol);
        info!("Fetching initial snapshot for {}", symbol);

        let res = client.get(&url)
            .send()
            .await
            .unwrap_or_else(|e| panic!("Failed to fetch snapshot for {}: {:?}", symbol, e));

        let payload = res.json::<DepthPayload>()
            .await
            .unwrap_or_else(|e| panic!("Failed to parse snapshot for {}: {:?}", symbol, e));

        let book = payload.to_order_book().expect("Failed to convert raw book level to Decimal");

        let mut cache_lock = cache.write().expect("Failed to lock cache");
        cache_lock.books.insert(symbol.clone(), book);
    }
    info!("Cache successfully initialized with snapshots for all symbols.");
}

/// Starts the WebSocket stream task to fetch real-time updates and update the cache
pub async fn start_websocket_ingestion(
    ws_url: String,
    symbols: Vec<String>,
    cache: Arc<RwLock<OrderBookCache>>,
    notifier: Arc<Notify>,
) {
    let stream_params: Vec<String> = symbols
        .iter()
        .map(|s| format!("{}@depth5@100ms", s.to_lowercase()))
        .collect();
    let streams = stream_params.join("/");
    let connection_url = format!("{}?streams={}", ws_url, streams);

    loop {
        info!("Connecting to WebSocket stream: {}", connection_url);

        match tokio_tungstenite::connect_async(&connection_url).await {
            Ok((ws_stream, _)) => {
                info!("WebSocket connected successfully.");
                let (_, mut read) = ws_stream.split();

                while let Some(msg) = read.next().await {
                    match msg {
                        Ok(tokio_tungstenite::tungstenite::Message::Text(text)) => {
                            if let Ok(msg_parsed) = serde_json::from_str::<CombinedStreamMessage>(&text) {
                                // Extract the symbol name from the stream field, e.g. "ltcusdt@depth5@100ms" -> "LTCUSDT"
                                let symbol = msg_parsed.stream.split('@').next().unwrap_or("").to_uppercase();
                                if let Ok(book) = msg_parsed.data.to_order_book() {
                                    {
                                        let mut cache_lock = cache.write().expect("Failed to lock cache");
                                        cache_lock.books.insert(symbol, book);
                                    }
                                    // Notify the calculation loop of fresh data
                                    notifier.notify_one();
                                }
                            }
                        }
                        Ok(tokio_tungstenite::tungstenite::Message::Ping(_)) => {}
                        Err(e) => {
                            error!("WebSocket stream read error: {:?}. Reconnecting...", e);
                            break;
                        }
                        _ => {}
                    }
                }
            }
            Err(e) => {
                error!("WebSocket connection failed: {:?}. Retrying in 5 seconds...", e);
            }
        }
        sleep(Duration::from_secs(5)).await;
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolMetadata {
    symbol: String,
    status: String,
    base_asset: String,
    quote_asset: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeInfoSymbolsOnly {
    symbols: Vec<SymbolMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ticker24hr {
    symbol: String,
    quote_volume: String,
}

/// Dynamic whitelisting discovery: queries 24h ticker info and exchange symbols
/// to dynamically locate active Altcoins with both Alt/USDT and Alt/BTC trading pairs,
/// filtering by 24h quote volume limits.
pub async fn discover_triangles(
    rest_url: &str,
    min_vol: Decimal,
    max_vol: Decimal,
) -> Result<Vec<Triangle>, reqwest::Error> {
    use crate::config::{Triangle, Leg, TradeDirection};

    let client = reqwest::Client::new();
    
    // 1. Fetch exchange info to get active symbols & mappings
    let info_url = format!("{}/api/v3/exchangeInfo", rest_url);
    let info_res = client.get(&info_url)
        .send()
        .await?
        .json::<ExchangeInfoSymbolsOnly>()
        .await?;
    
    // 2. Fetch 24h tickers to check trading volumes
    let ticker_url = format!("{}/api/v3/ticker/24hr", rest_url);
    let ticker_res = client.get(&ticker_url)
        .send()
        .await?
        .json::<Vec<Ticker24hr>>()
        .await?;
    
    // Create map of symbol -> 24h quoteVolume
    let mut quote_volumes = HashMap::new();
    for ticker in ticker_res {
        if let Ok(vol) = ticker.quote_volume.parse::<Decimal>() {
            quote_volumes.insert(ticker.symbol, vol);
        }
    }
    
    // Create map of symbol -> metadata
    let mut symbol_map = HashMap::new();
    for sym in info_res.symbols {
        if sym.status == "TRADING" {
            symbol_map.insert(sym.symbol.clone(), sym);
        }
    }
    
    // First, let's identify all active A/USDT symbols matching volume filter
    let mut altcoins = Vec::new();
    for (sym_name, metadata) in &symbol_map {
        if metadata.quote_asset == "USDT" && metadata.base_asset != "BTC" {
            if let Some(&vol) = quote_volumes.get(sym_name) {
                if vol >= min_vol && vol <= max_vol {
                    altcoins.push(metadata.base_asset.clone());
                }
            }
        }
    }
    
    let mut triangles = Vec::new();
    
    // For each whitelisted altcoin, check if Alt/BTC exists
    for altcoin in altcoins {
        let alt_usdt_symbol = format!("{}USDT", altcoin);
        let alt_btc_symbol = format!("{}BTC", altcoin);
        
        if symbol_map.contains_key(&alt_usdt_symbol) && symbol_map.contains_key(&alt_btc_symbol) {
            triangles.push(Triangle {
                name: format!("USDT->{}->BTC->USDT", altcoin),
                leg1: Leg {
                    symbol: alt_usdt_symbol,
                    base_asset: altcoin.clone(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Buy,
                },
                leg2: Leg {
                    symbol: alt_btc_symbol,
                    base_asset: altcoin.clone(),
                    quote_asset: "BTC".to_string(),
                    direction: TradeDirection::Sell,
                },
                leg3: Leg {
                    symbol: "BTCUSDT".to_string(),
                    base_asset: "BTC".to_string(),
                    quote_asset: "USDT".to_string(),
                    direction: TradeDirection::Sell,
                },
            });
        }
    }
    
    Ok(triangles)
}

