//! WebSocket Handler
//!
//! Phase 11: Complete WebSocket with proper authentication and real-time updates

use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::auth::eip712::{verify_ws_auth_signature, WebSocketAuthMessage};
use crate::auth::jwt::validate_token;
use crate::services::matching::OrderbookUpdate;
use crate::services::kline::KlinePeriod;
use crate::AppState;

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClientMessage {
    /// Authenticate with wallet signature or JWT token
    Auth {
        #[serde(default)]
        address: Option<String>,
        #[serde(default)]
        signature: Option<String>,
        #[serde(default)]
        timestamp: Option<u64>,
        #[serde(default)]
        token: Option<String>,
    },
    /// Authenticate with JWT token (alternative to signature auth)
    AuthToken {
        token: String,
    },
    Subscribe {
        channel: String,
        #[serde(default)]
        token: Option<String>,
    },
    Unsubscribe {
        channel: String,
    },
    Ping,
}

#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ServerMessage {
    AuthResult {
        success: bool,
        message: Option<String>,
    },
    Subscribed {
        channel: String,
    },
    Unsubscribed {
        channel: String,
    },
    Trade {
        id: String,
        symbol: String,
        price: String,
        amount: String,
        side: String,
        timestamp: i64,
    },
    Orderbook {
        symbol: String,
        bids: Vec<OrderbookLevel>,
        asks: Vec<OrderbookLevel>,
        timestamp: i64,
    },
    Ticker {
        symbol: String,
        last_price: String,
        mark_price: String,
        index_price: String,
        price_change_24h: String,
        price_change_percent_24h: String,
        high_24h: String,
        low_24h: String,
        volume_24h: String,
        volume_24h_usd: String,
        /// Open Interest - Long position value in USD
        open_interest_long: String,
        /// Open Interest - Short position value in USD
        open_interest_short: String,
        /// Open Interest - Long percentage (e.g., "58")
        open_interest_long_percent: String,
        /// Open Interest - Short percentage (e.g., "42")
        open_interest_short_percent: String,
        /// Available liquidity for long positions
        available_liquidity_long: String,
        /// Available liquidity for short positions
        available_liquidity_short: String,
        /// Funding rate for long positions per hour (negative = pay)
        funding_rate_long_1h: String,
        /// Funding rate for short positions per hour (negative = pay)
        funding_rate_short_1h: String,
    },
    Position {
        id: String,
        symbol: String,
        side: String,
        size: String,
        entry_price: String,
        mark_price: String,
        liquidation_price: String,
        unrealized_pnl: String,
        leverage: i32,
        margin: String,
        updated_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        event: Option<String>,
    },
    Order {
        id: String,
        symbol: String,
        side: String,
        order_type: String,
        price: Option<String>,
        amount: String,
        filled_amount: String,
        status: String,
        updated_at: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        event: Option<String>,
    },
    Balance {
        token: String,
        symbol: String,
        available: String,
        frozen: String,
        total: String,
    },
    Error {
        code: String,
        message: String,
    },
    Pong,
    /// K-line update
    Kline {
        channel: String,
        data: KlineData,
    },
    /// K-line snapshot (initial data on subscribe)
    KlineSnapshot {
        channel: String,
        data: KlineData,
    },
}

/// Orderbook level for WebSocket (frontend compatible format)
#[derive(Debug, Serialize, Clone)]
pub struct OrderbookLevel {
    pub price: String,
    pub size: String,
}

/// K-line data for WebSocket
#[derive(Debug, Serialize, Clone)]
pub struct KlineData {
    pub time: i64,
    pub open: String,
    pub high: String,
    pub low: String,
    pub close: String,
    pub volume: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote_volume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trade_count: Option<u32>,
    pub is_final: bool,
}

/// Format funding rate as percentage string with sign
fn format_funding_rate(rate: Decimal) -> String {
    let pct = rate * Decimal::from(100);
    if pct >= Decimal::ZERO {
        format!("+{}%", pct.round_dp(4))
    } else {
        format!("{}%", pct.round_dp(4))
    }
}


/// Validate timestamp (within 5 minutes)
fn validate_timestamp(timestamp: u64) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now.abs_diff(timestamp) <= 300
}

pub async fn handle_socket(socket: WebSocket, state: Arc<AppState>) {
    let (mut sender, mut receiver) = socket.split();

    let mut authenticated = false;
    let mut user_address: Option<String> = None;
    let mut subscriptions: HashSet<String> = HashSet::new();

    // Subscribe to trade events from matching engine
    let mut trade_receiver = state.matching_engine.subscribe_trades();
    tracing::info!("📡 WebSocket subscribed to trade events from matching engine");

    // Subscribe to orderbook updates from matching engine
    let mut orderbook_receiver = state.matching_engine.subscribe_orderbook();
    tracing::info!("📡 WebSocket subscribed to orderbook events from matching engine");

    // Subscribe to K-line updates
    let mut kline_receiver = state.kline_service.subscribe();

    // Subscribe to order updates for real-time push
    let mut order_update_receiver = state.order_update_sender.subscribe();
    tracing::info!("📡 WebSocket subscribed to order update events");

    // Ticker update interval (every 2 seconds)
    let mut ticker_interval = tokio::time::interval(tokio::time::Duration::from_secs(2));

    // Orderbook update interval (every 500ms for real-time feel)
    let mut orderbook_interval = tokio::time::interval(tokio::time::Duration::from_millis(500));

    // Position/balance update interval for authenticated users (every 5 seconds)
    let mut private_interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

    loop {
        tokio::select! {
            // Handle incoming client messages
            msg = receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Err(response) = handle_client_message(
                            &text,
                            &mut authenticated,
                            &mut user_address,
                            &mut subscriptions,
                            &state,
                            &mut sender,
                        ).await {
                            let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                        }
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = sender.send(Message::Pong(data)).await;
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        break;
                    }
                    Some(Err(e)) => {
                        // Connection reset without closing handshake is normal
                        // (user closes browser, network switch, etc.)
                        tracing::warn!("WebSocket disconnected: {}", e);
                        break;
                    }
                    _ => {}
                }
            }

            // Handle trade events from matching engine
            trade = trade_receiver.recv() => {
                match trade {
                    Ok(trade_event) => {
                        tracing::debug!(
                            "📊 WebSocket received trade event: symbol={}, price={}, amount={}, side={}",
                            trade_event.symbol, trade_event.price, trade_event.amount, trade_event.side
                        );
                        
                        let trade_channel = format!("trades:{}", trade_event.symbol);
                        tracing::debug!(
                            "📡 Checking subscriptions for channel '{}': {:?}",
                            trade_channel, subscriptions
                        );
                        
                        if subscriptions.contains(&trade_channel) || subscriptions.contains("trades:*") {
                            tracing::info!("✅ Sending trade to WebSocket client: {}", trade_channel);
                            // Generate unique trade ID from timestamp and random suffix
                            let trade_id = format!("{}-{}", trade_event.timestamp, Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));
                            let msg = ServerMessage::Trade {
                                id: trade_id,
                                symbol: trade_event.symbol.clone(),
                                price: trade_event.price.to_string(),
                                amount: trade_event.amount.to_string(),
                                side: trade_event.side.clone(),
                                timestamp: trade_event.timestamp,
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        } else {
                            tracing::warn!(
                                "⚠️  Trade NOT sent - no matching subscription. Channel: '{}', Have: {:?}",
                                trade_channel, subscriptions
                            );
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("⚠️  Trade receiver lagged by {} messages - some trades may have been missed!", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        tracing::error!("❌ Trade receiver closed - no more trade events will be received");
                        break;
                    }
                }
            }

            // Handle orderbook updates from matching engine
            orderbook = orderbook_receiver.recv() => {
                match orderbook {
                    Ok(orderbook_update) => {
                        let orderbook_channel = format!("orderbook:{}", orderbook_update.symbol);
                        if subscriptions.contains(&orderbook_channel) || subscriptions.contains("orderbook:*") {
                            // Convert to frontend-compatible format
                            let bids: Vec<OrderbookLevel> = orderbook_update.bids
                                .into_iter()
                                .map(|[price, size]| OrderbookLevel { price, size })
                                .collect();
                            let asks: Vec<OrderbookLevel> = orderbook_update.asks
                                .into_iter()
                                .map(|[price, size]| OrderbookLevel { price, size })
                                .collect();
                            let msg = ServerMessage::Orderbook {
                                symbol: orderbook_update.symbol.clone(),
                                bids,
                                asks,
                                timestamp: orderbook_update.timestamp,
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Orderbook receiver lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Continue without orderbook updates
                    }
                }
            }

            // Handle K-line updates
            kline = kline_receiver.recv() => {
                match kline {
                    Ok(kline_update) => {
                        // Check if client is subscribed to this kline channel
                        let channel = format!("kline:{}:{}", kline_update.symbol, kline_update.period);
                        if subscriptions.contains(&channel) {
                            let msg = ServerMessage::Kline {
                                channel: channel.clone(),
                                data: KlineData {
                                    time: kline_update.candle.time,
                                    open: kline_update.candle.open.to_string(),
                                    high: kline_update.candle.high.to_string(),
                                    low: kline_update.candle.low.to_string(),
                                    close: kline_update.candle.close.to_string(),
                                    volume: kline_update.candle.volume.to_string(),
                                    quote_volume: kline_update.candle.quote_volume.map(|v| v.to_string()),
                                    trade_count: kline_update.candle.trade_count,
                                    is_final: kline_update.is_final,
                                },
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Kline receiver lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Continue without kline updates
                    }
                }
            }

            // Handle order updates (real-time push when orders are created/updated)
            order_update = order_update_receiver.recv() => {
                match order_update {
                    Ok(event) => {
                        // Only send to the user who owns this order
                        if authenticated && user_address.is_some() {
                            let addr = user_address.as_ref().unwrap().to_lowercase();
                            if addr == event.user_address && subscriptions.contains("orders") {
                                tracing::info!(
                                    "📤 Sending real-time order update to {}: order_id={}, status={:?}",
                                    addr, event.order.order_id, event.order.status
                                );
                                let msg = serde_json::json!({
                                    "channel": "orders",
                                    "type": "order_update",
                                    "data": event.order
                                });
                                let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Order update receiver lagged by {} messages", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // Continue without order updates
                    }
                }
            }

            // Ticker updates
            _ = ticker_interval.tick() => {
                for channel in &subscriptions {
                    if channel.starts_with("ticker:") {
                        let raw_symbol = channel.strip_prefix("ticker:").unwrap_or("");
                        let symbol = normalize_symbol(raw_symbol);
                        
                        tracing::trace!("Ticker update check for {} (from channel: {})", symbol, channel);
                        
                        if let Some(price_data) = state.price_feed_service.get_price_data(&symbol).await {
                            tracing::trace!("Sending ticker data for {}: price={}", symbol, price_data.last_price);
                            // Get funding rate info for open interest and rates
                            let funding_info = state.funding_rate_service.get_funding_rate(&symbol).await;

                            // Get open interest values
                            let (oi_long, oi_short) = if let Some(ref info) = funding_info {
                                (info.long_open_interest, info.short_open_interest)
                            } else {
                                (Decimal::ZERO, Decimal::ZERO)
                            };

                            // Calculate OI percentages
                            let total_oi = oi_long + oi_short;
                            let (oi_long_pct, oi_short_pct) = if total_oi > Decimal::ZERO {
                                let long_pct = (oi_long / total_oi * Decimal::from(100)).round_dp(0);
                                let short_pct = (oi_short / total_oi * Decimal::from(100)).round_dp(0);
                                (long_pct, short_pct)
                            } else {
                                (Decimal::from(50), Decimal::from(50))
                            };

                            // Get funding rate per hour
                            let funding_rate_1h = if let Some(ref info) = funding_info {
                                info.funding_rate_per_hour
                            } else {
                                Decimal::ZERO
                            };

                            // Funding: Long pays when rate is positive, Short pays when rate is negative
                            let funding_long = -funding_rate_1h;  // Long pays positive rate
                            let funding_short = funding_rate_1h;   // Short receives positive rate

                            // Calculate available liquidity from orderbook
                            let (liq_long, liq_short) = if let Some(orderbook_cache) = state.cache.orderbook_opt() {
                                let ob = orderbook_cache.get_orderbook(&symbol, Some(50)).await;
                                let ask_liquidity: Decimal = ob.asks.iter()
                                    .map(|level| level.price * level.amount)
                                    .sum();
                                let bid_liquidity: Decimal = ob.bids.iter()
                                    .map(|level| level.price * level.amount)
                                    .sum();
                                (ask_liquidity, bid_liquidity)  // Long buys from asks, Short buys from bids
                            } else {
                                (Decimal::ZERO, Decimal::ZERO)
                            };

                            let msg = ServerMessage::Ticker {
                                symbol: symbol.to_string(),
                                last_price: price_data.last_price.to_string(),
                                mark_price: price_data.mark_price.to_string(),
                                index_price: price_data.index_price.to_string(),
                                price_change_24h: price_data.price_change_24h.to_string(),
                                price_change_percent_24h: price_data.price_change_percent_24h.to_string(),
                                high_24h: price_data.high_24h.to_string(),
                                low_24h: price_data.low_24h.to_string(),
                                volume_24h: price_data.volume_24h.to_string(),
                                volume_24h_usd: price_data.volume_ccy_24h.to_string(),
                                open_interest_long: oi_long.to_string(),
                                open_interest_short: oi_short.to_string(),
                                open_interest_long_percent: oi_long_pct.to_string(),
                                open_interest_short_percent: oi_short_pct.to_string(),
                                available_liquidity_long: liq_long.to_string(),
                                available_liquidity_short: liq_short.to_string(),
                                funding_rate_long_1h: format_funding_rate(funding_long),
                                funding_rate_short_1h: format_funding_rate(funding_short),
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        }
                    }
                }
            }

            // Orderbook updates from Redis cache
            _ = orderbook_interval.tick() => {
                if let Some(orderbook_cache) = state.cache.orderbook_opt() {
                    for channel in &subscriptions {
                        if channel.starts_with("orderbook:") {
                            let raw_symbol = channel.strip_prefix("orderbook:").unwrap_or("");
                            let symbol = normalize_symbol(raw_symbol);
                            let cached = orderbook_cache.get_orderbook(&symbol, Some(20)).await;
                            if !cached.bids.is_empty() || !cached.asks.is_empty() {
                                let bids: Vec<OrderbookLevel> = cached.bids
                                    .iter()
                                    .map(|level| OrderbookLevel {
                                        price: level.price.to_string(),
                                        size: level.amount.to_string(),
                                    })
                                    .collect();
                                let asks: Vec<OrderbookLevel> = cached.asks
                                    .iter()
                                    .map(|level| OrderbookLevel {
                                        price: level.price.to_string(),
                                        size: level.amount.to_string(),
                                    })
                                    .collect();
                                let msg = ServerMessage::Orderbook {
                                    symbol: cached.symbol,
                                    bids,
                                    asks,
                                    timestamp: cached.timestamp,
                                };
                                let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                            }
                        }
                    }
                }
            }

            // Private data updates (positions, orders, balances)
            _ = private_interval.tick() => {
                if authenticated && user_address.is_some() {
                    let address = user_address.as_ref().unwrap().to_lowercase();

                    // Send position updates
                    if subscriptions.contains("positions") {
                        if let Ok(positions) = fetch_user_positions(&state, &address).await {
                            for position in positions {
                                let _ = sender.send(Message::Text(serde_json::to_string(&position).unwrap())).await;
                            }
                        }
                    }

                    // Send balance updates
                    if subscriptions.contains("balance") {
                        if let Ok(balances) = fetch_user_balances(&state, &address).await {
                            for balance in balances {
                                let _ = sender.send(Message::Text(serde_json::to_string(&balance).unwrap())).await;
                            }
                        }
                    }

                    // Send open order updates
                    if subscriptions.contains("orders") {
                        if let Ok(orders) = fetch_user_orders(&state, &address).await {
                            for order in orders {
                                let _ = sender.send(Message::Text(serde_json::to_string(&order).unwrap())).await;
                            }
                        }
                    }
                }
            }
        }
    }

    tracing::info!("WebSocket connection closed for {:?}", user_address);
}

async fn handle_client_message(
    text: &str,
    authenticated: &mut bool,
    user_address: &mut Option<String>,
    subscriptions: &mut HashSet<String>,
    state: &Arc<AppState>,
    sender: &mut futures::stream::SplitSink<WebSocket, Message>,
) -> Result<(), ServerMessage> {
    let client_msg: ClientMessage = serde_json::from_str(text).map_err(|e| ServerMessage::Error {
        code: "INVALID_MESSAGE".to_string(),
        message: format!("Failed to parse message: {}", e),
    })?;

    match client_msg {
        ClientMessage::Auth {
            address,
            signature,
            timestamp,
            token,
        } => {
            // Check if token-based auth (JWT)
            if let Some(jwt_token) = token {
                match validate_token(&jwt_token, &state.config.jwt_secret) {
                    Ok(claims) => {
                        *authenticated = true;
                        *user_address = Some(claims.sub.to_lowercase());

                        tracing::info!("WebSocket authenticated via JWT: {}", claims.sub);

                        let response = ServerMessage::AuthResult {
                            success: true,
                            message: None,
                        };
                        let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                    }
                    Err(e) => {
                        tracing::warn!("WebSocket JWT validation failed: {}", e);
                        let response = ServerMessage::AuthResult {
                            success: false,
                            message: Some("Invalid or expired token".to_string()),
                        };
                        let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                    }
                }
                return Ok(());
            }

            // Signature-based auth requires all fields
            let (address, signature, timestamp) = match (address, signature, timestamp) {
                (Some(a), Some(s), Some(t)) => (a, s, t),
                _ => {
                    let response = ServerMessage::AuthResult {
                        success: false,
                        message: Some("Missing required fields for signature auth".to_string()),
                    };
                    let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                    return Ok(());
                }
            };

            // 验证时间戳（5分钟内有效）
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            if now.abs_diff(timestamp) > 300 {
                tracing::warn!("WebSocket auth timestamp expired for address: {}", address);
                let response = ServerMessage::AuthResult {
                    success: false,
                    message: Some("Timestamp expired".to_string()),
                };
                let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                return Ok(());
            }

            // EIP-712 签名验证
            let ws_auth_msg = WebSocketAuthMessage {
                wallet: address.to_lowercase(),
                timestamp,
            };

            let valid = match verify_ws_auth_signature(&ws_auth_msg, &signature, &address) {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!("WebSocket auth signature verification error for {}: {}", address, e);
                    let response = ServerMessage::AuthResult {
                        success: false,
                        message: Some("Invalid signature format".to_string()),
                    };
                    let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                    return Ok(());
                }
            };

            if !valid {
                tracing::warn!("WebSocket auth signature verification failed for address: {}", address);
                let response = ServerMessage::AuthResult {
                    success: false,
                    message: Some("Signature verification failed".to_string()),
                };
                let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                return Ok(());
            }

            tracing::info!("EIP-712 WebSocket auth signature verified for address: {}", address);

            *authenticated = true;
            *user_address = Some(address.to_lowercase());

            tracing::info!("WebSocket authenticated: {}", address);

            let response = ServerMessage::AuthResult {
                success: true,
                message: None,
            };
            let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
        }

        ClientMessage::AuthToken { token } => {
            // Validate JWT token
            match validate_token(&token, &state.config.jwt_secret) {
                Ok(claims) => {
                    *authenticated = true;
                    *user_address = Some(claims.sub.to_lowercase());

                    tracing::info!("WebSocket authenticated via JWT: {}", claims.sub);

                    let response = ServerMessage::AuthResult {
                        success: true,
                        message: None,
                    };
                    let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                }
                Err(e) => {
                    tracing::warn!("WebSocket JWT validation failed: {}", e);
                    let response = ServerMessage::AuthResult {
                        success: false,
                        message: Some("Invalid or expired token".to_string()),
                    };
                    let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
                }
            }
        }

        ClientMessage::Subscribe { channel, token } => {
            // If token is provided with subscribe, try to authenticate first
            if let Some(jwt_token) = token {
                if !*authenticated {
                    if let Ok(claims) = validate_token(&jwt_token, &state.config.jwt_secret) {
                        *authenticated = true;
                        *user_address = Some(claims.sub.to_lowercase());
                        tracing::info!("WebSocket auto-authenticated via subscribe token: {}", claims.sub);
                    }
                }
            }

            // Check if private channel requires auth
            let is_private = channel.starts_with("positions")
                || channel.starts_with("orders")
                || channel.starts_with("balance");

            if is_private && !*authenticated {
                return Err(ServerMessage::Error {
                    code: "AUTH_REQUIRED".to_string(),
                    message: "Authentication required for private channels".to_string(),
                });
            }

            subscriptions.insert(channel.clone());
            
            tracing::info!(
                "✅ Client subscribed to '{}' (total subscriptions: {})",
                channel, subscriptions.len()
            );
            tracing::debug!("Current subscriptions: {:?}", subscriptions);

            let response = ServerMessage::Subscribed { channel: channel.clone() };
            let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;

            // Send initial data for certain channels
            if channel.starts_with("orderbook:") {
                let raw_symbol = channel.strip_prefix("orderbook:").unwrap_or("");
                let symbol = normalize_symbol(raw_symbol);
                // Try Redis cache first, then fallback to matching engine
                let orderbook_msg = if let Some(orderbook_cache) = state.cache.orderbook_opt() {
                    let cached = orderbook_cache.get_orderbook(&symbol, Some(20)).await;
                    if !cached.bids.is_empty() || !cached.asks.is_empty() {
                        let bids: Vec<OrderbookLevel> = cached.bids
                            .iter()
                            .map(|level| OrderbookLevel {
                                price: level.price.to_string(),
                                size: level.amount.to_string(),
                            })
                            .collect();
                        let asks: Vec<OrderbookLevel> = cached.asks
                            .iter()
                            .map(|level| OrderbookLevel {
                                price: level.price.to_string(),
                                size: level.amount.to_string(),
                            })
                            .collect();
                        Some(ServerMessage::Orderbook {
                            symbol: cached.symbol,
                            bids,
                            asks,
                            timestamp: cached.timestamp,
                        })
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Fallback to matching engine if Redis cache is empty
                let msg = orderbook_msg.unwrap_or_else(|| {
                    if let Ok(snapshot) = state.matching_engine.get_orderbook(&symbol, 20) {
                        let bids: Vec<OrderbookLevel> = snapshot.bids
                            .into_iter()
                            .map(|[price, size]| OrderbookLevel { price, size })
                            .collect();
                        let asks: Vec<OrderbookLevel> = snapshot.asks
                            .into_iter()
                            .map(|[price, size]| OrderbookLevel { price, size })
                            .collect();
                        ServerMessage::Orderbook {
                            symbol: snapshot.symbol,
                            bids,
                            asks,
                            timestamp: snapshot.timestamp,
                        }
                    } else {
                        ServerMessage::Orderbook {
                            symbol: symbol.to_string(),
                            bids: vec![],
                            asks: vec![],
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        }
                    }
                });
                let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
            } else if channel.starts_with("ticker:") {
                let raw_symbol = channel.strip_prefix("ticker:").unwrap_or("");
                let symbol = normalize_symbol(raw_symbol);
                if let Some(price_data) = state.price_feed_service.get_price_data(&symbol).await {
                    // Get funding rate info for open interest and rates
                    let funding_info = state.funding_rate_service.get_funding_rate(&symbol).await;

                    // Get open interest values
                    let (oi_long, oi_short) = if let Some(ref info) = funding_info {
                        (info.long_open_interest, info.short_open_interest)
                    } else {
                        (Decimal::ZERO, Decimal::ZERO)
                    };

                    // Calculate OI percentages
                    let total_oi = oi_long + oi_short;
                    let (oi_long_pct, oi_short_pct) = if total_oi > Decimal::ZERO {
                        let long_pct = (oi_long / total_oi * Decimal::from(100)).round_dp(0);
                        let short_pct = (oi_short / total_oi * Decimal::from(100)).round_dp(0);
                        (long_pct, short_pct)
                    } else {
                        (Decimal::from(50), Decimal::from(50))
                    };

                    // Get funding rate per hour
                    let funding_rate_1h = if let Some(ref info) = funding_info {
                        info.funding_rate_per_hour
                    } else {
                        Decimal::ZERO
                    };

                    // Funding: Long pays when rate is positive, Short pays when rate is negative
                    let funding_long = -funding_rate_1h;
                    let funding_short = funding_rate_1h;

                    // Calculate available liquidity from orderbook
                    let (liq_long, liq_short) = if let Some(orderbook_cache) = state.cache.orderbook_opt() {
                        let ob = orderbook_cache.get_orderbook(&symbol, Some(50)).await;
                        let ask_liquidity: Decimal = ob.asks.iter()
                            .map(|level| level.price * level.amount)
                            .sum();
                        let bid_liquidity: Decimal = ob.bids.iter()
                            .map(|level| level.price * level.amount)
                            .sum();
                        (ask_liquidity, bid_liquidity)
                    } else {
                        (Decimal::ZERO, Decimal::ZERO)
                    };

                    let msg = ServerMessage::Ticker {
                        symbol: symbol.to_string(),
                        last_price: price_data.last_price.to_string(),
                        mark_price: price_data.mark_price.to_string(),
                        index_price: price_data.index_price.to_string(),
                        price_change_24h: price_data.price_change_24h.to_string(),
                        price_change_percent_24h: price_data.price_change_percent_24h.to_string(),
                        high_24h: price_data.high_24h.to_string(),
                        low_24h: price_data.low_24h.to_string(),
                        volume_24h: price_data.volume_24h.to_string(),
                        volume_24h_usd: price_data.volume_ccy_24h.to_string(),
                        open_interest_long: oi_long.to_string(),
                        open_interest_short: oi_short.to_string(),
                        open_interest_long_percent: oi_long_pct.to_string(),
                        open_interest_short_percent: oi_short_pct.to_string(),
                        available_liquidity_long: liq_long.to_string(),
                        available_liquidity_short: liq_short.to_string(),
                        funding_rate_long_1h: format_funding_rate(funding_long),
                        funding_rate_short_1h: format_funding_rate(funding_short),
                    };
                    let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                }
            } else if channel == "positions" && *authenticated && user_address.is_some() {
                let address = user_address.as_ref().unwrap().to_lowercase();
                if let Ok(positions) = fetch_user_positions(state, &address).await {
                    for position in positions {
                        let _ = sender.send(Message::Text(serde_json::to_string(&position).unwrap())).await;
                    }
                }
            } else if channel == "balance" && *authenticated && user_address.is_some() {
                let address = user_address.as_ref().unwrap().to_lowercase();
                if let Ok(balances) = fetch_user_balances(state, &address).await {
                    for balance in balances {
                        let _ = sender.send(Message::Text(serde_json::to_string(&balance).unwrap())).await;
                    }
                }
            } else if channel == "orders" && *authenticated && user_address.is_some() {
                let address = user_address.as_ref().unwrap().to_lowercase();
                if let Ok(orders) = fetch_user_orders(state, &address).await {
                    for order in orders {
                        let _ = sender.send(Message::Text(serde_json::to_string(&order).unwrap())).await;
                    }
                }
            } else if channel.starts_with("kline:") {
                // Parse kline channel: kline:{symbol}:{period}
                let parts: Vec<&str> = channel.strip_prefix("kline:").unwrap_or("").split(':').collect();
                if parts.len() == 2 {
                    let raw_symbol = parts[0];
                    let symbol = normalize_symbol(raw_symbol);
                    let period_str = parts[1];
                    if let Some(period) = KlinePeriod::from_str(period_str) {
                        // Send initial snapshot (latest candle)
                        let candles = state.kline_service.get_candles(&symbol, period, 1, None, None).await;
                        if let Some(latest_candle) = candles.into_iter().last() {
                            let snapshot_data = KlineData {
                                time: latest_candle.time,
                                open: latest_candle.open.to_string(),
                                high: latest_candle.high.to_string(),
                                low: latest_candle.low.to_string(),
                                close: latest_candle.close.to_string(),
                                volume: latest_candle.volume.to_string(),
                                quote_volume: latest_candle.quote_volume.map(|v| v.to_string()),
                                trade_count: latest_candle.trade_count,
                                is_final: true, // Historical candles are final
                            };
                            let msg = ServerMessage::KlineSnapshot {
                                channel: channel.clone(),
                                data: snapshot_data,
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        }
                    }
                }
            }
        }

        ClientMessage::Unsubscribe { channel } => {
            subscriptions.remove(&channel);

            let response = ServerMessage::Unsubscribed { channel };
            let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
        }

        ClientMessage::Ping => {
            let response = ServerMessage::Pong;
            let _ = sender.send(Message::Text(serde_json::to_string(&response).unwrap())).await;
        }
    }

    Ok(())
}

/// Fetch user positions from database
async fn fetch_user_positions(state: &Arc<AppState>, address: &str) -> Result<Vec<ServerMessage>, sqlx::Error> {
    let rows: Vec<(String, String, String, Decimal, Decimal, Decimal, i32, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"
        SELECT id::text, symbol, side, size, entry_price, collateral, leverage, updated_at
        FROM positions
        WHERE user_address = $1 AND status = 'open'
        "#
    )
    .bind(address)
    .fetch_all(&state.db.pool)
    .await?;

    let mut messages = Vec::new();
    for (id, symbol, side, size, entry_price, collateral, leverage, updated_at) in rows {
        // Get mark price
        let mark_price = state.price_feed_service
            .get_mark_price(&symbol)
            .await
            .unwrap_or(entry_price);

        // Calculate unrealized PnL
        let is_long = side.to_lowercase() == "long";
        let unrealized_pnl = if is_long {
            (mark_price - entry_price) * size
        } else {
            (entry_price - mark_price) * size
        };

        // Calculate liquidation price
        let position_value = size * entry_price;
        let maintenance_margin = position_value * Decimal::new(5, 3);
        let liq_distance = (collateral - maintenance_margin) / size;
        let liquidation_price = if is_long {
            entry_price - liq_distance
        } else {
            entry_price + liq_distance
        };

        messages.push(ServerMessage::Position {
            id,
            symbol,
            side,
            size: size.to_string(),
            entry_price: entry_price.to_string(),
            mark_price: mark_price.to_string(),
            liquidation_price: liquidation_price.max(Decimal::ZERO).to_string(),
            unrealized_pnl: unrealized_pnl.to_string(),
            leverage,
            margin: collateral.to_string(),
            updated_at: updated_at.timestamp_millis(),
            event: None, // Event is set when position state changes
        });
    }

    Ok(messages)
}

/// Fetch user balances from database
async fn fetch_user_balances(state: &Arc<AppState>, address: &str) -> Result<Vec<ServerMessage>, sqlx::Error> {
    let rows: Vec<(String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT token, available, frozen FROM balances WHERE user_address = $1"
    )
    .bind(address)
    .fetch_all(&state.db.pool)
    .await?;

    let messages: Vec<ServerMessage> = rows
        .into_iter()
        .map(|(token, available, frozen)| {
            // Get symbol from config if possible, otherwise use token address
            let symbol = state.config.get_token_symbol(&token)
                .map(|s| s.to_string())
                .unwrap_or_else(|| token.clone());

            ServerMessage::Balance {
                token,
                symbol,
                available: available.to_string(),
                frozen: frozen.to_string(),
                total: (available + frozen).to_string(),
            }
        })
        .collect();

    Ok(messages)
}

/// Fetch user open orders from database
async fn fetch_user_orders(state: &Arc<AppState>, address: &str) -> Result<Vec<ServerMessage>, sqlx::Error> {
    let rows: Vec<(String, String, String, String, Option<Decimal>, Decimal, Decimal, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"
        SELECT id::text, symbol, side, order_type, price, amount, filled_amount, status, updated_at
        FROM orders
        WHERE user_address = $1 AND status IN ('open', 'pending', 'partially_filled')
        ORDER BY created_at DESC
        LIMIT 50
        "#
    )
    .bind(address)
    .fetch_all(&state.db.pool)
    .await?;

    let messages: Vec<ServerMessage> = rows
        .into_iter()
        .map(|(id, symbol, side, order_type, price, amount, filled_amount, status, updated_at)| {
            ServerMessage::Order {
                id,
                symbol,
                side,
                order_type,
                price: price.map(|p| p.to_string()),
                amount: amount.to_string(),
                filled_amount: filled_amount.to_string(),
                status,
                updated_at: updated_at.timestamp_millis(),
                event: None, // Event is set when order state changes
            }
        })
        .collect();

    Ok(messages)
}
