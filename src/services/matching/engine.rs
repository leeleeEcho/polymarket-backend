//! Matching Engine
//!
//! High-performance order matching engine with concurrent access support.
//! Manages multiple orderbooks for different trading pairs.
//!
//! # Prediction Market Matching
//!
//! Supports three types of matching:
//! - **Normal**: Same share type, opposite sides (Yes buy vs Yes sell)
//! - **Mint**: Two buys for complementary shares (Yes buy + No buy → new shares)
//! - **Merge**: Two sells for complementary shares (Yes sell + No sell → collateral)

use super::history::HistoryManager;
use super::orderbook::Orderbook;
use super::types::*;
use crate::metrics;
use crate::models::market::ShareType;
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
        // Start with empty orderbooks for prediction markets
        // Orderbooks are created dynamically when orders are submitted
        Self::with_symbols(vec![])
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
    // Complement Orderbook (for Mint/Merge matching)
    // ========================================================================

    /// Parse market key into components
    fn parse_market_key(market_key: &str) -> Option<(Uuid, Uuid, ShareType)> {
        OrderbookSnapshot::parse_market_key(market_key)
    }

    /// Get the complement market key (Yes ↔ No)
    fn get_complement_market_key(market_key: &str) -> Option<String> {
        let (market_id, outcome_id, share_type) = Self::parse_market_key(market_key)?;
        let complement_type = share_type.complement();
        Some(format!("{}:{}:{}", market_id, outcome_id, complement_type))
    }

    /// Get or create the complement orderbook
    fn get_or_create_complement_orderbook(&self, market_key: &str) -> Option<Arc<Orderbook>> {
        let complement_key = Self::get_complement_market_key(market_key)?;
        let orderbook = self.orderbooks
            .entry(complement_key.clone())
            .or_insert_with(|| Arc::new(Orderbook::new(complement_key)))
            .clone();
        Some(orderbook)
    }

    /// Try Mint matching: match taker buy order against complement orderbook's buy orders
    ///
    /// Mint matching occurs when:
    /// - Taker wants to buy Yes shares at price P_yes
    /// - Maker wants to buy No shares at price P_no
    /// - P_yes + P_no >= 1.0 (combined willingness to pay covers minting cost)
    ///
    /// Example: Taker buys 100 Yes @ 0.65, Maker buys 100 No @ 0.40
    /// Combined: 0.65 + 0.40 = 1.05 >= 1.0 ✓
    /// Result: Mint 100 (Yes, No) pairs, taker gets Yes, maker gets No
    fn try_mint_match(
        &self,
        taker_order_id: Uuid,
        _taker_address: &str,
        taker_share_type: ShareType,
        taker_market_key: &str,
        complement_orderbook: &Orderbook,
        taker_price: Decimal,
        mut remaining_amount: Decimal,
    ) -> (Vec<TradeExecution>, Decimal) {
        let mut trades = Vec::new();
        let complement_price = Decimal::ONE - taker_price;
        let now = chrono::Utc::now().timestamp_millis();

        // Parse market_key for trade records
        let (market_id, outcome_id, _) = Self::parse_market_key(taker_market_key)
            .unwrap_or((Uuid::nil(), Uuid::nil(), ShareType::Yes));

        // Look for buy orders in complement orderbook that can mint with us
        // We need: taker_price + maker_price >= 1.0
        // So maker_price >= 1.0 - taker_price = complement_price
        //
        // In the complement orderbook, buy orders are stored in bids
        // We look for bids at price >= complement_price
        let matching_orders = complement_orderbook.get_matching_buy_orders(complement_price);

        for maker_order in matching_orders {
            if remaining_amount <= Decimal::ZERO {
                break;
            }

            // Calculate trade amount
            let trade_amount = remaining_amount.min(maker_order.remaining_amount);

            // Calculate fees
            let taker_fee = self.fee_config.calculate_taker_fee(taker_price, trade_amount);
            let maker_fee = self.fee_config.calculate_maker_fee(maker_order.price, trade_amount);

            let trade = TradeExecution {
                trade_id: Uuid::new_v4(),
                market_id,
                outcome_id,
                share_type: taker_share_type,
                match_type: MatchType::Mint,
                maker_order_id: maker_order.id,
                taker_order_id,
                maker_address: maker_order.user_address.clone(),
                price: taker_price, // Taker pays their price
                amount: trade_amount,
                maker_fee,
                taker_fee,
                timestamp: now,
            };

            trades.push(trade);
            remaining_amount -= trade_amount;

            // Update maker order in complement orderbook
            complement_orderbook.fill_order(maker_order.id, trade_amount);

            debug!(
                "🔨 MINT match: {} {} @ {:.4} + {} No @ {:.4} = {} pairs",
                trade_amount, taker_share_type, taker_price,
                trade_amount, maker_order.price, trade_amount
            );
        }

        (trades, remaining_amount)
    }

    /// Try Merge matching: match taker sell order against complement orderbook's sell orders
    ///
    /// Merge matching occurs when:
    /// - Taker wants to sell Yes shares at price P_yes
    /// - Maker wants to sell No shares at price P_no
    /// - P_yes + P_no <= 1.0 (combined receive <= redemption value)
    ///
    /// Example: Taker sells 100 Yes @ 0.55, Maker sells 100 No @ 0.40
    /// Combined: 0.55 + 0.40 = 0.95 <= 1.0 ✓
    /// Result: Merge 100 (Yes, No) pairs → redeem 100 USDC
    fn try_merge_match(
        &self,
        taker_order_id: Uuid,
        _taker_address: &str,
        taker_share_type: ShareType,
        taker_market_key: &str,
        complement_orderbook: &Orderbook,
        taker_price: Decimal,
        mut remaining_amount: Decimal,
    ) -> (Vec<TradeExecution>, Decimal) {
        let mut trades = Vec::new();
        let complement_price = Decimal::ONE - taker_price;
        let now = chrono::Utc::now().timestamp_millis();

        // Parse market_key for trade records
        let (market_id, outcome_id, _) = Self::parse_market_key(taker_market_key)
            .unwrap_or((Uuid::nil(), Uuid::nil(), ShareType::Yes));

        // Look for sell orders in complement orderbook that can merge with us
        // We need: taker_price + maker_price <= 1.0
        // So maker_price <= 1.0 - taker_price = complement_price
        //
        // In the complement orderbook, sell orders are stored in asks
        // We look for asks at price <= complement_price
        let matching_orders = complement_orderbook.get_matching_sell_orders(complement_price);

        for maker_order in matching_orders {
            if remaining_amount <= Decimal::ZERO {
                break;
            }

            // Calculate trade amount
            let trade_amount = remaining_amount.min(maker_order.remaining_amount);

            // Calculate fees
            let taker_fee = self.fee_config.calculate_taker_fee(taker_price, trade_amount);
            let maker_fee = self.fee_config.calculate_maker_fee(maker_order.price, trade_amount);

            let trade = TradeExecution {
                trade_id: Uuid::new_v4(),
                market_id,
                outcome_id,
                share_type: taker_share_type,
                match_type: MatchType::Merge,
                maker_order_id: maker_order.id,
                taker_order_id,
                maker_address: maker_order.user_address.clone(),
                price: taker_price, // Taker receives their price
                amount: trade_amount,
                maker_fee,
                taker_fee,
                timestamp: now,
            };

            trades.push(trade);
            remaining_amount -= trade_amount;

            // Update maker order in complement orderbook
            complement_orderbook.fill_order(maker_order.id, trade_amount);

            debug!(
                "🔄 MERGE match: {} {} @ {:.4} + {} No @ {:.4} → {} USDC",
                trade_amount, taker_share_type, taker_price,
                trade_amount, maker_order.price, trade_amount
            );
        }

        (trades, remaining_amount)
    }

    // ========================================================================
    // Order Operations
    // ========================================================================

    /// Submit an order for matching
    ///
    /// This method implements the full prediction market matching logic:
    /// 1. **Normal matching**: Match against opposite side in same orderbook
    /// 2. **Mint matching** (for buy orders): Match against buy orders in complement orderbook
    /// 3. **Merge matching** (for sell orders): Match against sell orders in complement orderbook
    pub fn submit_order(
        &self,
        order_id: Uuid,
        symbol: &str,
        user_address: &str,
        side: Side,
        order_type: OrderType,
        amount: Decimal,
        price: Option<Decimal>,
        _leverage: u32,
    ) -> Result<MatchResult, MatchingError> {
        // Get or create orderbook for this symbol/market_key
        // For prediction markets, orderbooks are created dynamically
        let orderbook = self.orderbooks
            .entry(symbol.to_string())
            .or_insert_with(|| Arc::new(Orderbook::new(symbol.to_string())))
            .clone();

        // Validate inputs
        if amount <= Decimal::ZERO {
            return Err(MatchingError::InvalidAmount("Amount must be positive".to_string()));
        }

        if order_type == OrderType::Limit && price.is_none() {
            return Err(MatchingError::InvalidPrice("Limit order requires price".to_string()));
        }

        // Record order submission metric
        let timer = metrics::Timer::new();
        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };
        let order_type_str = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
        };
        metrics::record_order_submitted(side_str, order_type_str);

        let now = chrono::Utc::now().timestamp_millis();

        debug!(
            "Processing order: id={}, symbol={}, side={:?}, type={:?}, amount={}, price={:?}",
            order_id, symbol, side, order_type, amount, price
        );

        // Parse share type from symbol for Mint/Merge matching
        let share_type = Self::parse_market_key(symbol)
            .map(|(_, _, st)| st)
            .unwrap_or(ShareType::Yes);

        // ========================================================================
        // Step 1: Normal matching (same share type, opposite sides)
        // ========================================================================
        let (mut trades, mut remaining) = orderbook.match_order(
            order_id,
            user_address,
            side,
            amount,
            price,
            &self.fee_config,
        );

        // ========================================================================
        // Step 2: Mint/Merge matching (complement orderbook)
        // ========================================================================
        // Only try Mint/Merge if:
        // - There's remaining amount
        // - Order has a price (limit order)
        // - We can find/create the complement orderbook
        if remaining > Decimal::ZERO && price.is_some() {
            if let Some(complement_orderbook) = self.get_or_create_complement_orderbook(symbol) {
                let taker_price = price.unwrap();

                match side {
                    Side::Buy => {
                        // Try Mint matching: match our buy against complement's buy orders
                        let (mint_trades, new_remaining) = self.try_mint_match(
                            order_id,
                            user_address,
                            share_type,
                            symbol,
                            &complement_orderbook,
                            taker_price,
                            remaining,
                        );

                        if !mint_trades.is_empty() {
                            info!(
                                "🔨 MINT matched {} trades, filled {} shares",
                                mint_trades.len(),
                                remaining - new_remaining
                            );
                            trades.extend(mint_trades);
                            remaining = new_remaining;

                            // Broadcast complement orderbook update
                            if let Some(complement_key) = Self::get_complement_market_key(symbol) {
                                self.broadcast_orderbook_update(&complement_key);
                            }
                        }
                    }
                    Side::Sell => {
                        // Try Merge matching: match our sell against complement's sell orders
                        let (merge_trades, new_remaining) = self.try_merge_match(
                            order_id,
                            user_address,
                            share_type,
                            symbol,
                            &complement_orderbook,
                            taker_price,
                            remaining,
                        );

                        if !merge_trades.is_empty() {
                            info!(
                                "🔄 MERGE matched {} trades, redeemed {} shares",
                                merge_trades.len(),
                                remaining - new_remaining
                            );
                            trades.extend(merge_trades);
                            remaining = new_remaining;

                            // Broadcast complement orderbook update
                            if let Some(complement_key) = Self::get_complement_market_key(symbol) {
                                self.broadcast_orderbook_update(&complement_key);
                            }
                        }
                    }
                }
            }
        }

        let filled_amount = amount - remaining;

        // Broadcast trade events and record metrics
        for trade in &trades {
            // Use from_execution to preserve match_type (Normal/Mint/Merge)
            let event = TradeEvent::from_execution(
                trade,
                symbol.to_string(),
                user_address.to_string(),
                side,
            );

            // Record trade metrics
            let match_type_str = match trade.match_type {
                MatchType::Normal => "normal",
                MatchType::Mint => "mint",
                MatchType::Merge => "merge",
            };
            metrics::record_order_matched(match_type_str);

            // Record trade volume (convert Decimal to f64 for metrics)
            let volume_usdc = (trade.price * trade.amount).to_string().parse::<f64>().unwrap_or(0.0);
            metrics::record_trade_executed(match_type_str, volume_usdc);

            // Record mint/merge specific metrics
            match trade.match_type {
                MatchType::Mint => metrics::record_mint_operation(),
                MatchType::Merge => metrics::record_merge_operation(),
                MatchType::Normal => {}
            }

            // Broadcast with detailed logging
            let match_type_log = match trade.match_type {
                MatchType::Normal => "NORMAL",
                MatchType::Mint => "🔨 MINT",
                MatchType::Merge => "🔄 MERGE",
            };
            match self.trade_sender.send(event.clone()) {
                Ok(n) => {
                    info!(
                        "📊 {} Trade broadcast to {} subscribers: symbol={}, price={}, amount={}, side={}",
                        match_type_log, n, event.symbol, event.price, event.amount, event.side
                    );
                }
                Err(e) => {
                    warn!(
                        "⚠️  Failed to broadcast {} trade (no subscribers?): {} - symbol={}, price={}",
                        match_type_log, e, event.symbol, event.price
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
                        let _ = orderbook.add_order(entry);
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
                    let _ = orderbook.add_order(entry);
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
            leverage: 1, // No leverage in prediction markets
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

        // Record order matching duration
        metrics::record_order_match_duration(timer.elapsed_secs());

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
            // Record cancellation metric
            metrics::record_order_cancelled();

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

    fn create_market_key() -> String {
        let market_id = Uuid::new_v4();
        let outcome_id = Uuid::new_v4();
        format!("{}:{}:yes", market_id, outcome_id)
    }

    #[test]
    fn test_engine_creation() {
        let engine = MatchingEngine::new();
        // New engine starts empty (no default symbols for prediction markets)
        assert!(engine.symbols().is_empty());
    }

    #[test]
    fn test_submit_limit_order_no_match() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        let result = engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x1234",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.55)), // probability price 0-1
            1,
        ).unwrap();

        assert_eq!(result.status, OrderStatus::Open);
        assert_eq!(result.filled_amount, dec!(0));
        assert_eq!(result.remaining_amount, dec!(100.0));
        assert!(result.trades.is_empty());
    }

    #[test]
    fn test_submit_and_match_orders() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        // Submit sell order at 0.60
        let sell_result = engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x1111",
            Side::Sell,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.60)),
            1,
        ).unwrap();
        assert_eq!(sell_result.status, OrderStatus::Open);

        // Submit matching buy order at 0.60
        let buy_result = engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x2222",
            Side::Buy,
            OrderType::Limit,
            dec!(50.0),
            Some(dec!(0.60)),
            1,
        ).unwrap();

        assert_eq!(buy_result.status, OrderStatus::Filled);
        assert_eq!(buy_result.filled_amount, dec!(50.0));
        assert_eq!(buy_result.trades.len(), 1);
        assert_eq!(buy_result.trades[0].price, dec!(0.60));
        assert_eq!(buy_result.trades[0].amount, dec!(50.0));
    }

    #[test]
    fn test_market_order() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        // Add liquidity (sell order)
        engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x1111",
            Side::Sell,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.65)),
            1,
        ).unwrap();

        // Market buy
        let result = engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x2222",
            Side::Buy,
            OrderType::Market,
            dec!(50.0),
            None,
            1,
        ).unwrap();

        assert_eq!(result.status, OrderStatus::Filled);
        assert_eq!(result.filled_amount, dec!(50.0));
    }

    #[test]
    fn test_cancel_order() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        let result = engine.submit_order(
            Uuid::new_v4(),
            &market_key,
            "0x1234",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.55)),
            1,
        ).unwrap();

        let cancelled = engine.cancel_order(&market_key, result.order_id, "0x1234").unwrap();
        assert!(cancelled);

        // Try to cancel again
        let cancelled_again = engine.cancel_order(&market_key, result.order_id, "0x1234").unwrap();
        assert!(!cancelled_again);
    }

    #[test]
    fn test_orderbook_snapshot() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        // Add orders with probability prices
        engine.submit_order(Uuid::new_v4(), &market_key, "0x1", Side::Buy, OrderType::Limit, dec!(100.0), Some(dec!(0.55)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), &market_key, "0x2", Side::Buy, OrderType::Limit, dec!(200.0), Some(dec!(0.50)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), &market_key, "0x3", Side::Sell, OrderType::Limit, dec!(150.0), Some(dec!(0.65)), 1).unwrap();

        let snapshot = engine.get_orderbook(&market_key, 10).unwrap();

        assert!(snapshot.symbol.contains("yes")); // market_key contains share type
        assert_eq!(snapshot.bids.len(), 2);
        assert_eq!(snapshot.asks.len(), 1);
    }

    #[test]
    fn test_trade_history() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        // Create trades
        engine.submit_order(Uuid::new_v4(), &market_key, "0x1", Side::Sell, OrderType::Limit, dec!(100.0), Some(dec!(0.60)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), &market_key, "0x2", Side::Buy, OrderType::Limit, dec!(100.0), Some(dec!(0.60)), 1).unwrap();

        let trades = engine.get_trades(&market_key, &TradeHistoryQuery::default());
        assert_eq!(trades.total_count, 1);
    }

    #[test]
    fn test_order_history() {
        let engine = MatchingEngine::new();
        let market_key = create_market_key();

        engine.submit_order(Uuid::new_v4(), &market_key, "0x1234", Side::Buy, OrderType::Limit, dec!(100.0), Some(dec!(0.55)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), &market_key, "0x1234", Side::Sell, OrderType::Limit, dec!(50.0), Some(dec!(0.65)), 1).unwrap();

        let orders = engine.get_orders("0x1234", &OrderHistoryQuery::default());
        assert_eq!(orders.total_count, 2);
    }

    #[test]
    fn test_stats() {
        let engine = MatchingEngine::new();
        let market_key1 = create_market_key();
        let market_key2 = create_market_key();

        engine.submit_order(Uuid::new_v4(), &market_key1, "0x1", Side::Buy, OrderType::Limit, dec!(100.0), Some(dec!(0.55)), 1).unwrap();
        engine.submit_order(Uuid::new_v4(), &market_key2, "0x2", Side::Sell, OrderType::Limit, dec!(200.0), Some(dec!(0.65)), 1).unwrap();

        let stats = engine.stats();
        // With Mint/Merge support, complement orderbooks are also created
        // So we have 4 orderbooks: 2 original + 2 complements (Yes <-> No)
        assert_eq!(stats.symbols_count, 4);
        assert_eq!(stats.total_orders_in_book, 2);
    }

    #[test]
    fn test_mint_matching() {
        let engine = MatchingEngine::new();

        // Create market keys for Yes and No
        let market_id = Uuid::new_v4();
        let outcome_id = Uuid::new_v4();
        let yes_market_key = format!("{}:{}:yes", market_id, outcome_id);
        let no_market_key = format!("{}:{}:no", market_id, outcome_id);

        // User A submits buy order for No shares at 0.40
        let order_a = engine.submit_order(
            Uuid::new_v4(),
            &no_market_key,
            "0xUserA",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.40)),
            1,
        ).unwrap();

        // Order should be open (no match yet)
        assert_eq!(order_a.status, OrderStatus::Open);
        assert_eq!(order_a.filled_amount, dec!(0));

        // User B submits buy order for Yes shares at 0.65
        // Combined: 0.65 + 0.40 = 1.05 >= 1.0, should trigger MINT
        let order_b = engine.submit_order(
            Uuid::new_v4(),
            &yes_market_key,
            "0xUserB",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.65)),
            1,
        ).unwrap();

        // Should be filled via MINT matching
        assert_eq!(order_b.status, OrderStatus::Filled);
        assert_eq!(order_b.filled_amount, dec!(100.0));
        assert_eq!(order_b.trades.len(), 1);
        assert_eq!(order_b.trades[0].match_type, MatchType::Mint);
    }

    #[test]
    fn test_merge_matching() {
        let engine = MatchingEngine::new();

        // Create market keys for Yes and No
        let market_id = Uuid::new_v4();
        let outcome_id = Uuid::new_v4();
        let yes_market_key = format!("{}:{}:yes", market_id, outcome_id);
        let no_market_key = format!("{}:{}:no", market_id, outcome_id);

        // User A submits sell order for No shares at 0.35
        let order_a = engine.submit_order(
            Uuid::new_v4(),
            &no_market_key,
            "0xUserA",
            Side::Sell,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.35)),
            1,
        ).unwrap();

        // Order should be open (no match yet)
        assert_eq!(order_a.status, OrderStatus::Open);
        assert_eq!(order_a.filled_amount, dec!(0));

        // User B submits sell order for Yes shares at 0.60
        // Combined: 0.60 + 0.35 = 0.95 <= 1.0, should trigger MERGE
        let order_b = engine.submit_order(
            Uuid::new_v4(),
            &yes_market_key,
            "0xUserB",
            Side::Sell,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.60)),
            1,
        ).unwrap();

        // Should be filled via MERGE matching
        assert_eq!(order_b.status, OrderStatus::Filled);
        assert_eq!(order_b.filled_amount, dec!(100.0));
        assert_eq!(order_b.trades.len(), 1);
        assert_eq!(order_b.trades[0].match_type, MatchType::Merge);
    }

    #[test]
    fn test_mint_not_triggered_when_prices_too_low() {
        let engine = MatchingEngine::new();

        // Create market keys for Yes and No
        let market_id = Uuid::new_v4();
        let outcome_id = Uuid::new_v4();
        let yes_market_key = format!("{}:{}:yes", market_id, outcome_id);
        let no_market_key = format!("{}:{}:no", market_id, outcome_id);

        // User A submits buy order for No shares at 0.30
        engine.submit_order(
            Uuid::new_v4(),
            &no_market_key,
            "0xUserA",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.30)),
            1,
        ).unwrap();

        // User B submits buy order for Yes shares at 0.60
        // Combined: 0.60 + 0.30 = 0.90 < 1.0, should NOT trigger MINT
        let order_b = engine.submit_order(
            Uuid::new_v4(),
            &yes_market_key,
            "0xUserB",
            Side::Buy,
            OrderType::Limit,
            dec!(100.0),
            Some(dec!(0.60)),
            1,
        ).unwrap();

        // Should be open (no MINT match because prices sum to < 1.0)
        assert_eq!(order_b.status, OrderStatus::Open);
        assert_eq!(order_b.filled_amount, dec!(0));
        assert!(order_b.trades.is_empty());
    }
}
