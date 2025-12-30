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
#[allow(unused_imports)]
use crate::services::matching::OrderbookUpdate;
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

#[allow(dead_code)]
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
    // =========================================
    // Prediction Market Specific Messages
    // =========================================
    /// Trade event for prediction markets
    MarketTrade {
        id: String,
        market_id: String,
        outcome_id: String,
        share_type: String,
        match_type: String, // "normal", "mint", "merge"
        price: String,
        amount: String,
        side: String,
        timestamp: i64,
    },
    /// Orderbook update for prediction markets
    MarketOrderbook {
        market_id: String,
        outcome_id: String,
        share_type: String,
        bids: Vec<OrderbookLevel>,
        asks: Vec<OrderbookLevel>,
        timestamp: i64,
    },
    /// Market status/probability update
    MarketUpdate {
        market_id: String,
        status: String,
        yes_price: String,
        no_price: String,
        volume_24h: String,
        timestamp: i64,
    },
    /// User share position update
    ShareUpdate {
        market_id: String,
        outcome_id: String,
        share_type: String,
        amount: String,
        avg_cost: String,
        unrealized_pnl: String,
        event: String, // "buy", "sell", "mint", "merge"
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

/// Validate timestamp (within 5 minutes)
#[allow(dead_code)]
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

            // Handle trade events from matching engine (prediction market trades)
            trade = trade_receiver.recv() => {
                match trade {
                    Ok(trade_event) => {
                        // Generate unique trade ID
                        let trade_id = format!("{}-{}", trade_event.timestamp, Uuid::new_v4().to_string().split('-').next().unwrap_or("0"));

                        // Check for prediction market channel subscriptions
                        // Channels: "trades:{market_id}", "market:{market_id}", "trades:*"
                        let market_id = trade_event.market_id.to_string();
                        let market_trade_channel = format!("trades:{}", market_id);
                        let market_channel = format!("market:{}", market_id);

                        let subscribed_to_market = subscriptions.contains(&market_trade_channel)
                            || subscriptions.contains(&market_channel)
                            || subscriptions.contains("trades:*");

                        if subscribed_to_market {
                            tracing::info!(
                                "✅ Sending market trade: market_id={}, match_type={}",
                                market_id, trade_event.match_type
                            );

                            let msg = ServerMessage::MarketTrade {
                                id: trade_id.clone(),
                                market_id: market_id.clone(),
                                outcome_id: trade_event.outcome_id.to_string(),
                                share_type: trade_event.share_type.to_string(),
                                match_type: trade_event.match_type.to_string().to_lowercase(),
                                price: trade_event.price.to_string(),
                                amount: trade_event.amount.to_string(),
                                side: trade_event.side.clone(),
                                timestamp: trade_event.timestamp,
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                        }

                        // Also check legacy symbol-based channel (backwards compatibility)
                        let symbol_channel = format!("trades:{}", trade_event.symbol);
                        if subscriptions.contains(&symbol_channel) {
                            let msg = ServerMessage::Trade {
                                id: trade_id,
                                symbol: trade_event.symbol.clone(),
                                price: trade_event.price.to_string(),
                                amount: trade_event.amount.to_string(),
                                side: trade_event.side.clone(),
                                timestamp: trade_event.timestamp,
                            };
                            let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
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
                        // Convert to frontend-compatible format
                        let bids: Vec<OrderbookLevel> = orderbook_update.bids
                            .iter()
                            .map(|[price, size]| OrderbookLevel { price: price.clone(), size: size.clone() })
                            .collect();
                        let asks: Vec<OrderbookLevel> = orderbook_update.asks
                            .iter()
                            .map(|[price, size]| OrderbookLevel { price: price.clone(), size: size.clone() })
                            .collect();

                        // Symbol format for prediction markets: {market_id}:{outcome_id}:{share_type}
                        let symbol = &orderbook_update.symbol;

                        // Check for prediction market orderbook channel
                        // Channels: "orderbook:{market_id}:{outcome_id}:{share_type}", "orderbook:{market_id}", "market:{market_id}"
                        let parts: Vec<&str> = symbol.split(':').collect();
                        if parts.len() == 3 {
                            // Prediction market orderbook
                            let market_id = parts[0];
                            let outcome_id = parts[1];
                            let share_type = parts[2];

                            let specific_channel = format!("orderbook:{}", symbol);
                            let market_ob_channel = format!("orderbook:{}", market_id);
                            let market_channel = format!("market:{}", market_id);

                            let subscribed = subscriptions.contains(&specific_channel)
                                || subscriptions.contains(&market_ob_channel)
                                || subscriptions.contains(&market_channel)
                                || subscriptions.contains("orderbook:*");

                            if subscribed {
                                let msg = ServerMessage::MarketOrderbook {
                                    market_id: market_id.to_string(),
                                    outcome_id: outcome_id.to_string(),
                                    share_type: share_type.to_string(),
                                    bids: bids.clone(),
                                    asks: asks.clone(),
                                    timestamp: orderbook_update.timestamp,
                                };
                                let _ = sender.send(Message::Text(serde_json::to_string(&msg).unwrap())).await;
                            }
                        }

                        // Also check legacy symbol-based channel (backwards compatibility)
                        let orderbook_channel = format!("orderbook:{}", symbol);
                        if subscriptions.contains(&orderbook_channel) {
                            let msg = ServerMessage::Orderbook {
                                symbol: symbol.clone(),
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

            // Ticker updates - simplified for prediction markets
            _ = ticker_interval.tick() => {
                // TODO: Implement prediction market ticker updates if needed
                // For now, ticker updates are not supported in the prediction market version
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
                // TODO: Implement prediction market ticker subscription
                // For now, just acknowledge the subscription without sending data
                tracing::debug!("Ticker subscription for prediction markets not yet implemented");
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
            }
            // TODO: Add kline support for prediction markets if needed
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
/// Note: In prediction markets, "positions" are actually share holdings
async fn fetch_user_positions(state: &Arc<AppState>, address: &str) -> Result<Vec<ServerMessage>, sqlx::Error> {
    // For prediction markets, we don't have traditional positions with leverage
    // Instead we have share holdings. For now, return empty until we implement share holdings
    let rows: Vec<(String, String, String, Decimal, Decimal, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        r#"
        SELECT id::text, market_id::text, share_type, shares, avg_price, updated_at
        FROM share_holdings
        WHERE user_address = $1 AND shares > 0
        "#
    )
    .bind(address)
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default(); // Return empty if table doesn't exist yet

    let mut messages = Vec::new();
    for (id, market_id, share_type, shares, avg_price, updated_at) in rows {
        // For prediction markets, we report holdings as "positions"
        messages.push(ServerMessage::Position {
            id,
            symbol: format!("{}:{}", market_id, share_type),
            side: share_type, // Yes or No
            size: shares.to_string(),
            entry_price: avg_price.to_string(),
            mark_price: avg_price.to_string(), // TODO: Get current probability from orderbook
            liquidation_price: "0".to_string(), // No liquidation in prediction markets
            unrealized_pnl: "0".to_string(), // TODO: Calculate based on current probability
            leverage: 1, // No leverage in prediction markets
            margin: (shares * avg_price).to_string(),
            updated_at: updated_at.timestamp_millis(),
            event: None,
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
