use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;
// use crate::services::price_feed::MarketInfo;

/// Normalize symbol format to backend format (BTCUSDT)
/// Supports multiple input formats:
/// - "BTCUSDT" -> "BTCUSDT" (already correct)
/// - "BTC-USD" -> "BTCUSDT" (frontend TradingView format)
/// - "BTC-USDT" -> "BTCUSDT"
/// - "btcusdt" -> "BTCUSDT" (lowercase)
fn normalize_symbol(symbol: &str) -> String {
    let upper = symbol.to_uppercase();
    
    // If already in BTCUSDT format (no separators), return as is
    if !upper.contains('-') && !upper.contains('/') && !upper.contains('_') {
        return upper;
    }
    
    // Handle BTC-USD format (convert to BTCUSDT)
    if upper.ends_with("-USD") {
        let base = upper.strip_suffix("-USD").unwrap_or(&upper);
        return format!("{}USDT", base);
    }
    
    // Handle BTC-USDT format (convert to BTCUSDT)
    if upper.contains("-USDT") {
        return upper.replace("-", "");
    }
    
    // Handle BTC/USD or BTC_USD formats
    if upper.contains("/") || upper.contains("_") {
        let cleaned = upper.replace("/", "").replace("_", "");
        if !cleaned.ends_with("USDT") && cleaned.ends_with("USD") {
            let base = cleaned.strip_suffix("USD").unwrap_or(&cleaned);
            return format!("{}USDT", base);
        }
        return cleaned;
    }
    
    // Default: return uppercase version
    upper
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct Market {
    pub symbol: String,
    pub base_asset: String,
    pub quote_asset: String,
    pub last_price: Decimal,
    pub price_change_24h: Decimal,
    pub price_change_percent_24h: Decimal,
    pub high_24h: Decimal,
    pub low_24h: Decimal,
    pub volume_24h: Decimal,
    pub volume_24h_usd: Decimal,
    pub rank: usize,
}

#[derive(Debug, Serialize)]
pub struct MarketsResponse {
    pub markets: Vec<Market>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct MarketsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct OrderbookResponse {
    pub symbol: String,
    pub bids: Vec<[String; 2]>, // [price, amount]
    pub asks: Vec<[String; 2]>,
    pub timestamp: i64,
}

#[derive(Debug, Serialize)]
pub struct Trade {
    pub id: String,
    pub price: String,
    pub amount: String,
    pub side: String,
    pub timestamp: i64,
}

#[derive(Debug, Serialize)]
pub struct TradesResponse {
    pub symbol: String,
    pub trades: Vec<Trade>,
}

#[derive(Debug, Serialize)]
pub struct TickerResponse {
    pub symbol: String,
    pub last_price: Decimal,
    pub price_change_24h: Decimal,
    pub price_change_percent_24h: Decimal,
    pub high_24h: Decimal,
    pub low_24h: Decimal,
    pub volume_24h: Decimal,
    pub open_interest: Decimal,
    pub funding_rate: Decimal,
    pub next_funding_time: i64,
}

#[derive(Debug, Serialize)]
pub struct PriceResponse {
    pub symbol: String,
    pub mark_price: Decimal,
    pub index_price: Decimal,
    pub last_price: Decimal,
    pub bid_price: Decimal,
    pub ask_price: Decimal,
    pub funding_rate: Decimal,
    pub next_funding_rate: Decimal,
    pub next_funding_time: i64,
    pub updated_at: i64,
}

/// List all available markets (top 50 by volume from OKX)
pub async fn list_markets(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MarketsQuery>,
) -> Result<Json<MarketsResponse>, StatusCode> {
    let limit = query.limit.unwrap_or(50).min(50);

    // Get markets from price feed service
    let market_infos = state.price_feed_service.get_markets().await;
    let prices = state.price_feed_service.get_all_prices().await;

    let markets: Vec<Market> = market_infos
        .into_iter()
        .take(limit)
        .map(|info| {
            let price_data = prices.get(&info.symbol);

            Market {
                symbol: info.symbol.clone(),
                base_asset: info.base_asset,
                quote_asset: info.quote_asset,
                last_price: price_data.map(|p| p.last_price).unwrap_or(Decimal::ZERO),
                price_change_24h: price_data.map(|p| p.price_change_24h).unwrap_or(Decimal::ZERO),
                price_change_percent_24h: price_data.map(|p| p.price_change_percent_24h).unwrap_or(Decimal::ZERO),
                high_24h: price_data.map(|p| p.high_24h).unwrap_or(Decimal::ZERO),
                low_24h: price_data.map(|p| p.low_24h).unwrap_or(Decimal::ZERO),
                volume_24h: price_data.map(|p| p.volume_24h).unwrap_or(Decimal::ZERO),
                volume_24h_usd: info.volume_24h_usd,
                rank: info.rank,
            }
        })
        .collect();

    let total = markets.len();

    Ok(Json(MarketsResponse { markets, total }))
}

/// Get orderbook for a symbol
pub async fn get_orderbook(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<OrderbookResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Normalize symbol format (supports BTC-USD, BTCUSDT, etc.)
    let normalized_symbol = normalize_symbol(&symbol);
    
    // Validate market using dynamic symbol list
    if !state.price_feed_service.is_valid_symbol(&normalized_symbol).await {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Unknown trading pair: {}", normalized_symbol),
                code: "INVALID_MARKET".to_string(),
            }),
        ));
    }

    // Try to get orderbook from Redis cache first
    if let Some(orderbook_cache) = state.cache.orderbook_opt() {
        let cached = orderbook_cache.get_orderbook(&normalized_symbol, Some(20)).await;
        if !cached.bids.is_empty() || !cached.asks.is_empty() {
            // Convert PriceLevel to [String; 2] format
            let bids: Vec<[String; 2]> = cached.bids
                .iter()
                .map(|level| [level.price.to_string(), level.amount.to_string()])
                .collect();
            let asks: Vec<[String; 2]> = cached.asks
                .iter()
                .map(|level| [level.price.to_string(), level.amount.to_string()])
                .collect();

            return Ok(Json(OrderbookResponse {
                symbol: cached.symbol,
                bids,
                asks,
                timestamp: cached.timestamp,
            }));
        }
    }

    // Fallback to matching engine if Redis cache is empty
    match state.matching_engine.get_orderbook(&normalized_symbol, 20) {
        Ok(snapshot) => Ok(Json(OrderbookResponse {
            symbol: snapshot.symbol,
            bids: snapshot.bids,
            asks: snapshot.asks,
            timestamp: snapshot.timestamp,
        })),
        Err(_) => Ok(Json(OrderbookResponse {
            symbol: normalized_symbol,
            bids: vec![],
            asks: vec![],
            timestamp: chrono::Utc::now().timestamp_millis(),
        })),
    }
}

/// Get recent trades for a symbol
pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<TradesResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Normalize symbol format (supports BTC-USD, BTCUSDT, etc.)
    let normalized_symbol = normalize_symbol(&symbol);
    
    // Validate market using dynamic symbol list
    if !state.price_feed_service.is_valid_symbol(&normalized_symbol).await {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Unknown trading pair: {}", normalized_symbol),
                code: "INVALID_MARKET".to_string(),
            }),
        ));
    }

    // Get trades from database
    let rows: Vec<(String, Decimal, Decimal, String, i64)> = sqlx::query_as(
        r#"
        SELECT id::text, price, amount, side::text,
               EXTRACT(EPOCH FROM created_at)::bigint * 1000 as timestamp
        FROM trades
        WHERE symbol = $1
        ORDER BY created_at DESC
        LIMIT 50
        "#
    )
    .bind(&normalized_symbol)
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default();

    let trades: Vec<Trade> = rows
        .into_iter()
        .map(|(id, price, amount, side, timestamp)| Trade {
            id,
            price: price.to_string(),
            amount: amount.to_string(),
            side,
            timestamp,
        })
        .collect();

    Ok(Json(TradesResponse { symbol: normalized_symbol, trades }))
}

/// Get ticker for a symbol
pub async fn get_ticker(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<TickerResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Normalize symbol format (supports BTC-USD, BTCUSDT, etc.)
    let normalized_symbol = normalize_symbol(&symbol);
    
    // Validate market using dynamic symbol list
    if !state.price_feed_service.is_valid_symbol(&normalized_symbol).await {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Unknown trading pair: {}", normalized_symbol),
                code: "INVALID_MARKET".to_string(),
            }),
        ));
    }

    // Get real-time price data from price feed service
    if let Some(price_data) = state.price_feed_service.get_price_data(&normalized_symbol).await {
        return Ok(Json(TickerResponse {
            symbol: normalized_symbol.clone(),
            last_price: price_data.last_price,
            price_change_24h: price_data.price_change_24h,
            price_change_percent_24h: price_data.price_change_percent_24h,
            high_24h: price_data.high_24h,
            low_24h: price_data.low_24h,
            volume_24h: price_data.volume_ccy_24h,
            open_interest: Decimal::ZERO,
            funding_rate: price_data.funding_rate,
            next_funding_time: price_data.next_funding_time / 1000,
        }));
    }

    // Fallback to database if price feed not available
    let last_price: Option<Decimal> = sqlx::query_scalar(
        "SELECT price FROM trades WHERE symbol = $1 ORDER BY created_at DESC LIMIT 1"
    )
    .bind(&normalized_symbol)
    .fetch_optional(&state.db.pool)
    .await
    .ok()
    .flatten();

    // Get 24h stats
    let stats: Option<(Decimal, Decimal, Decimal)> = sqlx::query_as(
        r#"
        SELECT
            COALESCE(MAX(price), 0) as high,
            COALESCE(MIN(price), 0) as low,
            COALESCE(SUM(amount * price), 0) as volume
        FROM trades
        WHERE symbol = $1
        AND created_at > NOW() - INTERVAL '24 hours'
        "#
    )
    .bind(&normalized_symbol)
    .fetch_optional(&state.db.pool)
    .await
    .ok()
    .flatten();

    let (high_24h, low_24h, volume_24h) = stats.unwrap_or((Decimal::ZERO, Decimal::ZERO, Decimal::ZERO));

    Ok(Json(TickerResponse {
        symbol: normalized_symbol,
        last_price: last_price.unwrap_or(Decimal::ZERO),
        price_change_24h: Decimal::ZERO,
        price_change_percent_24h: Decimal::ZERO,
        high_24h,
        low_24h,
        volume_24h,
        open_interest: Decimal::ZERO,
        funding_rate: Decimal::new(1, 4), // 0.01%
        next_funding_time: chrono::Utc::now().timestamp() + 3600,
    }))
}

/// Get real-time price data for a symbol (from OKX)
pub async fn get_price(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> Result<Json<PriceResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Normalize symbol format (supports BTC-USD, BTCUSDT, etc.)
    let normalized_symbol = normalize_symbol(&symbol);
    
    // Validate market using dynamic symbol list
    if !state.price_feed_service.is_valid_symbol(&normalized_symbol).await {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: format!("Unknown trading pair: {}", normalized_symbol),
                code: "INVALID_MARKET".to_string(),
            }),
        ));
    }

    // Get price data from price feed service
    match state.price_feed_service.get_price_data(&normalized_symbol).await {
        Some(data) => Ok(Json(PriceResponse {
            symbol: normalized_symbol,
            mark_price: data.mark_price,
            index_price: data.index_price,
            last_price: data.last_price,
            bid_price: data.bid_price,
            ask_price: data.ask_price,
            funding_rate: data.funding_rate,
            next_funding_rate: data.next_funding_rate,
            next_funding_time: data.next_funding_time,
            updated_at: data.updated_at,
        })),
        None => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Price data temporarily unavailable".to_string(),
                code: "PRICE_DATA_UNAVAILABLE".to_string(),
            }),
        )),
    }
}
