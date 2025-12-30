//! Matching Engine
//!
//! High-performance order matching engine with concurrent access support.
//! Manages multiple orderbooks for different trading pairs.

use super::history::HistoryManager;
use super::orderbook::Orderbook;
use super::types::*;
use dashmap::DashMap;
use rust_decimal::Decimal;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// The main matching engine
pub struct MatchingEngine {
    /// Map of symbol to orderbook (concurrent access)
    orderbooks: DashMap<String, Arc<Orderbook>>,

    /// Trade event broadcaster
    trade_sender: broadcast::Sender<TradeEvent>,

    /// Orderbook update broadcaster
    orderbook_sender: broadcast::Sender<OrderbookUpdate>,

    /// History manager for trade/order records
    history: Arc<HistoryManager>,

    /// Fee configuration
    fee_config: FeeConfig,

    /// Supported symbols
    symbols: Vec<String>,
}

impl MatchingEngine {
    /// Create a new matching engine
    pub fn new() -> Self {
        Self::with_symbols(vec![
            "BTCUSDT".to_string(),
            "ETHUSDT".to_string(),
        ])
    }

    /// Create with specific symbols
    pub fn with_symbols(symbols: Vec<String>) -> Self {
        let (trade_sender, _) = broadcast::channel(10000);
        let (orderbook_sender, _) = broadcast::channel(10000);
        let orderbooks = DashMap::new();

        // Initialize orderbooks for all symbols
        for symbol in &symbols {
            orderbooks.insert(symbol.clone(), Arc::new(Orderbook::new(symbol.clone())));
        }

        info!("MatchingEngine initialized with {} symbols", symbols.len());

        Self {
            orderbooks,
            trade_sender,
            orderbook_sender,
            history: Arc::new(HistoryManager::new()),
            fee_config: FeeConfig::default(),
            symbols,
        }
    }

    /// Create with custom fee config
    pub fn with_fee_config(mut self, fee_config: FeeConfig) -> Self {
        self.fee_config = fee_config;
        self
    }

    /// Get supported symbols
    pub fn symbols(&self) -> &[String] {
        &self.symbols
    }

    /// Check if a symbol is supported
    pub fn is_valid_symbol(&self, symbol: &str) -> bool {
        self.orderbooks.contains_key(symbol)
    }

    /// Add a new symbol/market
    pub fn add_symbol(&mut self, symbol: String) {
        if !self.orderbooks.contains_key(&symbol) {
            self.orderbooks.insert(symbol.clone(), Arc::new(Orderbook::new(symbol.clone())));
            self.symbols.push(symbol.clone());
            info!("Added new symbol: {}", symbol);
        }
    }

    /// Get trade event receiver
    pub fn subscribe_trades(&self) -> broadcast::Receiver<TradeEvent> {
        self.trade_sender.subscribe()
    }

    /// Get orderbook update receiver
    pub fn subscribe_orderbook(&self) -> broadcast::Receiver<OrderbookUpdate> {
        self.orderbook_sender.subscribe()
    }

    /// Broadcast orderbook update for a symbol
    fn broadcast_orderbook_update(&self, symbol: &str) {
        if let Some(orderbook) = self.orderbooks.get(symbol) {
            let snapshot = orderbook.snapshot(20); // Top 20 levels
            let update = OrderbookUpdate {
                symbol: symbol.to_string(),
                bids: snapshot.bids,
                asks: snapshot.asks,
                timestamp: chrono::Utc::now().timestamp_millis(),
            };
            let _ = self.orderbook_sender.send(update);
        }
    }

    /// Get history manager
    pub fn history(&self) -> Arc<HistoryManager> {
        Arc::clone(&self.history)
    }

    /// Get orderbook for a symbol
    pub fn get_orderbook_ref(&self, symbol: &str) -> Option<Arc<Orderbook>> {
        self.orderbooks.get(symbol).map(|ob| Arc::clone(ob.value()))
    }

    // ========================================================================
    // Order Operations
    // ========================================================================

    /// Submit an order for matching
    pub fn submit_order(
        &self,
        order_id: Uuid,
        symbol: &str,
        user_address: &str,
        side: Side,
        order_type: OrderType,
        amount: Decimal,
        price: Option<Decimal>,
        leverage: u32,
    ) -> Result<MatchResult, MatchingError> {
        // Validate symbol
        let orderbook = self.orderbooks.get(symbol)
            .ok_or_else(|| MatchingError::SymbolNotFound(symbol.to_string()))?;

        // Validate inputs
        if amount <= Decimal::ZERO {
            return Err(MatchingError::InvalidAmount("Amount must be positive".to_string()));
        }

        if order_type == OrderType::Limit && price.is_none() {
            return Err(MatchingError::InvalidPrice("Limit order requires price".to_string()));
        }

        let now = chrono::Utc::now().timestamp_millis();

        debug!(
            "Processing order: id={}, symbol={}, side={:?}, type={:?}, amount={}, price={:?}",
            order_id, symbol, side, order_type, amount, price
        );

        // Match the order
        let (trades, remaining) = orderbook.match_order(
            order_id,
            user_address,
            side,
            amount,
            price,
            &self.fee_config,
        );

        let filled_amount = amount - remaining;

        // Broadcast trade events
        for trade in &trades {
            let event = TradeEvent {
                symbol: symbol.to_string(),
                trade_id: trade.trade_id,
                maker_order_id: trade.maker_order_id,
                taker_order_id: trade.taker_order_id,
                maker_address: trade.maker_address.clone(),
                taker_address: user_address.to_string(),
                side: side.to_string(),
                price: trade.price,
                amount: trade.amount,
                maker_fee: trade.maker_fee,
                taker_fee: trade.taker_fee,
                timestamp: trade.timestamp,
            };

            // Broadcast with detailed logging
            match self.trade_sender.send(event.clone()) {
                Ok(n) => {
                    info!(
                        "📊 Trade broadcast to {} subscribers: symbol={}, price={}, amount={}, side={}",
                        n, event.symbol, event.price, event.amount, event.side
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️  Failed to broadcast trade (no subscribers?): {} - symbol={}, price={}",
                        e, event.symbol, event.price
                    );
                }
            }

            // Store in history
            self.history.store_trade(TradeRecord::from(&event));
        }

        // Determine order status
        let status = match order_type {
            OrderType::Market => {
                // Market orders are IOC - any remaining is cancelled
                if filled_amount == amount {
                    OrderStatus::Filled
                } else if filled_amount > Decimal::ZERO {
                    OrderStatus::PartiallyFilled
                } else {
                    OrderStatus::Cancelled
                }
            }
            OrderType::Limit => {
                if filled_amount == amount {
                    OrderStatus::Filled
                } else if filled_amount > Decimal::ZERO {
                    // Add remaining to orderbook
                    if remaining > Decimal::ZERO {
                        let entry = OrderEntry {
                            id: order_id,
                            user_address: user_address.to_string(),
                            price: price.unwrap(),
                            original_amount: amount,
                            remaining_amount: remaining,
                            side,
                            time_in_force: TimeInForce::GTC,
                            timestamp: now,
                        };
                        orderbook.add_order(entry);
                    }
                    OrderStatus::PartiallyFilled
                } else {
                    // No fill, add entire order to book
                    let entry = OrderEntry {
                        id: order_id,
                        user_address: user_address.to_string(),
                        price: price.unwrap(),
                        original_amount: amount,
                        remaining_amount: amount,
                        side,
                        time_in_force: TimeInForce::GTC,
                        timestamp: now,
                    };
                    orderbook.add_order(entry);
                    OrderStatus::Open
                }
            }
        };

        // Calculate average price
        let average_price = if filled_amount > Decimal::ZERO {
            let total_value: Decimal = trades.iter().map(|t| t.price * t.amount).sum();
            Some(total_value / filled_amount)
        } else {
            None
        };

        // Store order in history
        let order_record = OrderHistoryRecord {
            order_id: order_id.to_string(),
            user_address: user_address.to_string(),
            symbol: symbol.to_string(),
            side: side.to_string(),
            order_type: format!("{:?}", order_type).to_lowercase(),
            price: price.map(|p| p.to_string()).unwrap_or_default(),
            original_amount: amount.to_string(),
            filled_amount: filled_amount.to_string(),
            remaining_amount: remaining.to_string(),
            status: status.to_string(),
            leverage,
            created_at: now,
            updated_at: now,
            avg_fill_price: average_price.map(|p| p.to_string()),
            trade_ids: trades.iter().map(|t| t.trade_id.to_string()).collect(),
        };
        self.history.store_order(order_record);

        info!(
            "Order completed: id={}, status={:?}, filled={}, remaining={}",
            order_id, status, filled_amount, remaining
        );

        // Broadcast orderbook update after order processing
        self.broadcast_orderbook_update(symbol);

        Ok(MatchResult {
            order_id,
            status,
            filled_amount,
            remaining_amount: remaining,
            average_price,
            trades,
        })
    }

    /// Cancel an order
    pub fn cancel_order(&self, symbol: &str, order_id: Uuid, user_address: &str) -> Result<bool, MatchingError> {
        let orderbook = self.orderbooks.get(symbol)
            .ok_or_else(|| MatchingError::SymbolNotFound(symbol.to_string()))?;

        // Try to cancel
        let cancelled = orderbook.cancel_order(order_id);

        if cancelled.is_some() {
            // Update order history
            self.history.update_order(user_address, &order_id.to_string(), |order| {
                order.status = "cancelled".to_string();
            });

            info!("Order cancelled: id={}, symbol={}", order_id, symbol);

            // Broadcast orderbook update after cancellation
            self.broadcast_orderbook_update(symbol);

            Ok(true)
        } else {
            warn!("Order not found for cancellation: id={}", order_id);
            Ok(false)
        }
    }

    // ========================================================================
    // Query Operations
    // ========================================================================

    /// Get orderbook snapshot
    pub fn get_orderbook(&self, symbol: &str, depth: usize) -> Result<OrderbookSnapshot, MatchingError> {
        let orderbook = self.orderbooks.get(symbol)
            .ok_or_else(|| MatchingError::SymbolNotFound(symbol.to_string()))?;

        Ok(orderbook.snapshot(depth))
    }

    /// Get best bid/ask
    pub fn get_best_prices(&self, symbol: &str) -> Result<(Option<Decimal>, Option<Decimal>), MatchingError> {
        let orderbook = self.orderbooks.get(symbol)
            .ok_or_else(|| MatchingError::SymbolNotFound(symbol.to_string()))?;

        Ok((orderbook.best_bid(), orderbook.best_ask()))
    }

    /// Get trade history for a symbol
    pub fn get_trades(&self, symbol: &str, query: &TradeHistoryQuery) -> TradeHistoryResponse {
        self.history.get_trades(symbol, query)
    }

    /// Get order history for a user
    pub fn get_orders(&self, user_address: &str, query: &OrderHistoryQuery) -> OrderHistoryResponse {
        self.history.get_orders(user_address, query)
    }

    /// Broadcast a trade event (for internal/market maker use)
    pub fn broadcast_trade(&self, event: TradeEvent) -> Result<usize, broadcast::error::SendError<TradeEvent>> {
        // Also store in history
        self.history.store_trade(TradeRecord::from(&event));
        self.trade_sender.send(event)
    }

    /// Recover open limit orders from database on startup
    /// This ensures orderbook state is preserved after restart
    pub async fn recover_orders_from_db(&self, pool: &sqlx::PgPool) -> anyhow::Result<usize> {
        use sqlx::Row;

        info!("🔄 Starting order recovery from database...");

        // Query all open limit orders from database
        let rows = sqlx::query(
            r#"
            SELECT id, symbol, user_address, side, price, amount, filled_amount, leverage, created_at
            FROM orders
            WHERE status = 'open' AND order_type = 'limit'
            ORDER BY created_at ASC
            "#
        )
        .fetch_all(pool)
        .await?;

        let mut recovered_count = 0;

        for row in rows {
            let order_id: uuid::Uuid = row.get("id");
            let symbol: String = row.get("symbol");
            let user_address: String = row.get("user_address");
            let side_db: crate::models::OrderSide = row.get("side");
            let price: rust_decimal::Decimal = row.get("price");
            let amount: rust_decimal::Decimal = row.get("amount");
            let filled_amount: rust_decimal::Decimal = row.get("filled_amount");
            let leverage: i32 = row.get("leverage");

            // Convert database OrderSide to matching engine Side
            let side = match side_db {
                crate::models::OrderSide::Buy => Side::Buy,
                crate::models::OrderSide::Sell => Side::Sell,
            };
            let side_str = match side_db {
                crate::models::OrderSide::Buy => "Buy",
                crate::models::OrderSide::Sell => "Sell",
            };

            // Calculate remaining amount
            let remaining_amount = amount - filled_amount;

            if remaining_amount <= rust_decimal::Decimal::ZERO {
                warn!("Order {} has no remaining amount, skipping", order_id);
                continue;
            }

            // Submit the order to matching engine (this will add it to orderbook)
            match self.submit_order(
                order_id,
                &symbol,
                &user_address,
                side,
                OrderType::Limit,
                remaining_amount,
                Some(price),
                leverage as u32,
            ) {
                Ok(_) => {
                    recovered_count += 1;
                    debug!("✅ Recovered order {}: {} {} @ {} (remaining: {})",
                        order_id, side_str, symbol, price, remaining_amount);
                }
                Err(e) => {
                    warn!("Failed to recover order {}: {}", order_id, e);
                }
            }
        }

        info!("✅ Order recovery complete: {} orders restored to orderbook", recovered_count);
        Ok(recovered_count)
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Get engine statistics
    pub fn stats(&self) -> EngineStats {
        let mut total_orders = 0i64;
        let mut total_bid_depth = Decimal::ZERO;
        let mut total_ask_depth = Decimal::ZERO;

        for entry in self.orderbooks.iter() {
            let ob = entry.value();
            total_orders += ob.order_count();
            total_bid_depth += ob.bid_depth();
            total_ask_depth += ob.ask_depth();
        }

        let history_stats = self.history.stats();

        EngineStats {
            symbols_count: self.orderbooks.len(),
            total_orders_in_book: total_orders,
            total_bid_depth,
            total_ask_depth,
            total_trades_recorded: history_stats.total_trades,
            total_orders_recorded: history_stats.total_orders,
        }
    }
}

impl Default for MatchingEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Engine statistics
#[derive(Debug, Clone)]
pub struct EngineStats {
    pub symbols_count: usize,
    pub total_orders_in_book: i64,
    pub total_bid_depth: Decimal,
    pub total_ask_depth: Decimal,
    pub total_trades_recorded: usize,
    pub total_orders_recorded: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_engine_creation() {
        let engine = MatchingEngine::new();
        assert_eq!(engine.symbols().len(), 2);
        assert!(engine.is_valid_symbol("BTCUSDT"));
        assert!(engine.is_valid_symbol("ETHUSDT"));
        assert!(!engine.is_valid_symbol("INVALID"));
    }

    #[test]
    fn test_submit_limit_order_no_match() {
        let engine = MatchingEngine::new();

        let result = engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x1234",
            Side::Buy,
            OrderType::Limit,
            dec!(1.0),
            Some(dec!(100.0)),
            1,
        ).unwrap();

        assert_eq!(result.status, OrderStatus::Open);
        assert_eq!(result.filled_amount, dec!(0));
        assert_eq!(result.remaining_amount, dec!(1.0));
        assert!(result.trades.is_empty());
    }

    #[test]
    fn test_submit_and_match_orders() {
        let engine = MatchingEngine::new();

        // Submit sell order
        let sell_result = engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x1111",
            Side::Sell,
            OrderType::Limit,
            dec!(1.0),
            Some(dec!(100.0)),
            1,
        ).unwrap();
        assert_eq!(sell_result.status, OrderStatus::Open);

        // Submit matching buy order
        let buy_result = engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x2222",
            Side::Buy,
            OrderType::Limit,
            dec!(0.5),
            Some(dec!(100.0)),
            1,
        ).unwrap();

        assert_eq!(buy_result.status, OrderStatus::Filled);
        assert_eq!(buy_result.filled_amount, dec!(0.5));
        assert_eq!(buy_result.trades.len(), 1);
        assert_eq!(buy_result.trades[0].price, dec!(100.0));
        assert_eq!(buy_result.trades[0].amount, dec!(0.5));
    }

    #[test]
    fn test_market_order() {
        let engine = MatchingEngine::new();

        // Add liquidity
        engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x1111",
            Side::Sell,
            OrderType::Limit,
            dec!(1.0),
            Some(dec!(100.0)),
            1,
        ).unwrap();

        // Market buy
        let result = engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x2222",
            Side::Buy,
            OrderType::Market,
            dec!(0.5),
            None,
            1,
        ).unwrap();

        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_amount, dec!(0.5));
    }

    #[test]
    fn test_cancel_order() {
        let engine = MatchingEngine::new();

        let result = engine.submit_order(
            Uuid::new_v4(),
            "BTCUSDT",
            "0x1234",
            Side::Buy,
            OrderType::Limit,
            dec!(1.0),
            Some(dec!(100.0)),
            1,
        ).unwrap();

        let cancelled = engine.cancel_order("BTCUSDT", result.order_id, "0x1234").unwrap();
        assert!(cancelled);

        // Try to cancel again
        let cancelled_again = engine.cancel_order("BTCUSDT", result.order_id, "0x1234").unwrap();
        assert!(!cancelled_again);
    }

    #[test]
    fn test_orderbook_snapshot() {
        let engine = MatchingEngine::new();

        // Add orders
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x1", Side::Buy, OrderType::Limit, dec!(1.0), Some(dec!(99.0)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x2", Side::Buy, OrderType::Limit, dec!(2.0), Some(dec!(98.0)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x3", Side::Sell, OrderType::Limit, dec!(1.5), Some(dec!(101.0)), 1).unwrap();

        let snapshot = engine.get_orderbook("BTCUSDT", 10).unwrap();

        assert_eq!(snapshot.symbol, "BTCUSDT");
        assert_eq!(snapshot.bids.len(), 2);
        assert_eq!(snapshot.asks.len(), 1);
    }

    #[test]
    fn test_trade_history() {
        let engine = MatchingEngine::new();

        // Create trades
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x1", Side::Sell, OrderType::Limit, dec!(1.0), Some(dec!(100.0)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x2", Side::Buy, OrderType::Limit, dec!(1.0), Some(dec!(100.0)), 1).unwrap();

        let trades = engine.get_trades("BTCUSDT", &TradeHistoryQuery::default());
        assert_eq!(trades.total_count, 1);
    }

    #[test]
    fn test_order_history() {
        let engine = MatchingEngine::new();

        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x1234", Side::Buy, OrderType::Limit, dec!(1.0), Some(dec!(100.0)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x1234", Side::Sell, OrderType::Limit, dec!(0.5), Some(dec!(105.0)), 1).unwrap();

        let orders = engine.get_orders("0x1234", &OrderHistoryQuery::default());
        assert_eq!(orders.total_count, 2);
    }

    #[test]
    fn test_stats() {
        let engine = MatchingEngine::new();

        engine.submit_order(Uuid::new_v4(), "BTCUSDT", "0x1", Side::Buy, OrderType::Limit, dec!(1.0), Some(dec!(100.0)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), "ETHUSDT", "0x2", Side::Sell, OrderType::Limit, dec!(2.0), Some(dec!(3000.0)), 1).unwrap();

        let stats = engine.stats();
        assert_eq!(stats.symbols_count, 2);
        assert_eq!(stats.total_orders_in_book, 2);
    }
}
