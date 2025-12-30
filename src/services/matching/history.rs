//! Trade and Order History
//!
//! In-memory storage for recent trades and orders with efficient lookup.

use super::types::*;
use dashmap::DashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::debug;

/// Trade and order history manager
pub struct HistoryManager {
    /// Trade history per symbol (most recent first)
    trade_history: DashMap<String, VecDeque<TradeRecord>>,

    /// Order history per user address (most recent first)
    order_history: DashMap<String, VecDeque<OrderHistoryRecord>>,

    /// Maximum trades to keep per symbol
    max_trades_per_symbol: usize,

    /// Maximum orders to keep per user
    max_orders_per_user: usize,

    /// Total trade count
    total_trades: AtomicUsize,

    /// Total order count
    total_orders: AtomicUsize,
}

impl HistoryManager {
    /// Create a new history manager
    pub fn new() -> Self {
        Self::with_limits(1000, 1000)
    }

    /// Create with custom limits
    pub fn with_limits(max_trades_per_symbol: usize, max_orders_per_user: usize) -> Self {
        Self {
            trade_history: DashMap::new(),
            order_history: DashMap::new(),
            max_trades_per_symbol,
            max_orders_per_user,
            total_trades: AtomicUsize::new(0),
            total_orders: AtomicUsize::new(0),
        }
    }

    // ========================================================================
    // Trade History
    // ========================================================================

    /// Store a trade record
    pub fn store_trade(&self, trade: TradeRecord) {
        let symbol = trade.symbol.clone();

        let mut entry = self.trade_history
            .entry(symbol.clone())
            .or_insert_with(VecDeque::new);

        // Add at front (most recent)
        entry.push_front(trade.clone());

        // Trim if exceeding limit
        if entry.len() > self.max_trades_per_symbol {
            entry.pop_back();
        } else {
            self.total_trades.fetch_add(1, Ordering::Relaxed);
        }

        debug!("Stored trade {} for {}", trade.trade_id, symbol);
    }

    /// Store multiple trades
    pub fn store_trades(&self, trades: Vec<TradeRecord>) {
        for trade in trades {
            self.store_trade(trade);
        }
    }

    /// Get trade history for a symbol
    pub fn get_trades(&self, symbol: &str, query: &TradeHistoryQuery) -> TradeHistoryResponse {
        let trades = self.trade_history.get(symbol).map(|entry| {
            let all_trades = entry.value();

            let mut filtered: Vec<TradeRecord> = all_trades
                .iter()
                .filter(|t| {
                    let matches_before = query.before.map_or(true, |ts| t.timestamp < ts);
                    let matches_after = query.after.map_or(true, |ts| t.timestamp > ts);
                    matches_before && matches_after
                })
                .cloned()
                .collect();

            let limit = query.get_limit();
            let has_more = filtered.len() > limit;
            filtered.truncate(limit);

            TradeHistoryResponse {
                trades: filtered.clone(),
                total_count: filtered.len(),
                has_more,
            }
        }).unwrap_or_else(|| TradeHistoryResponse {
            trades: vec![],
            total_count: 0,
            has_more: false,
        });

        trades
    }

    /// Get recent trades across all symbols
    pub fn get_recent_trades(&self, limit: usize) -> Vec<TradeRecord> {
        let mut all_trades: Vec<TradeRecord> = Vec::new();

        for entry in self.trade_history.iter() {
            for trade in entry.value().iter().take(limit) {
                all_trades.push(trade.clone());
            }
        }

        // Sort by timestamp descending
        all_trades.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all_trades.truncate(limit);
        all_trades
    }

    /// Get total trade count
    pub fn total_trade_count(&self) -> usize {
        self.total_trades.load(Ordering::Relaxed)
    }

    /// Clear trade history for a symbol
    pub fn clear_trades(&self, symbol: &str) {
        if let Some((_, trades)) = self.trade_history.remove(symbol) {
            self.total_trades.fetch_sub(trades.len(), Ordering::Relaxed);
        }
    }

    // ========================================================================
    // Order History
    // ========================================================================

    /// Store an order record
    pub fn store_order(&self, order: OrderHistoryRecord) {
        let user = order.user_address.clone();

        let mut entry = self.order_history
            .entry(user.clone())
            .or_insert_with(VecDeque::new);

        // Check if order already exists (update case)
        let existing_pos = entry.iter().position(|o| o.order_id == order.order_id);

        if let Some(pos) = existing_pos {
            // Update existing order
            entry.remove(pos);
            entry.push_front(order.clone());
        } else {
            // Add new order
            entry.push_front(order.clone());

            if entry.len() > self.max_orders_per_user {
                entry.pop_back();
            } else {
                self.total_orders.fetch_add(1, Ordering::Relaxed);
            }
        }

        debug!("Stored order {} for user {}", order.order_id, user);
    }

    /// Update an existing order
    pub fn update_order<F>(&self, user_address: &str, order_id: &str, updater: F)
    where
        F: FnOnce(&mut OrderHistoryRecord),
    {
        if let Some(mut entry) = self.order_history.get_mut(user_address) {
            if let Some(order) = entry.iter_mut().find(|o| o.order_id == order_id) {
                updater(order);
                order.updated_at = chrono::Utc::now().timestamp_millis();
                debug!("Updated order {} for user {}", order_id, user_address);
            }
        }
    }

    /// Get order history for a user
    pub fn get_orders(&self, user_address: &str, query: &OrderHistoryQuery) -> OrderHistoryResponse {
        let orders = self.order_history.get(user_address).map(|entry| {
            let all_orders = entry.value();

            let mut filtered: Vec<OrderHistoryRecord> = all_orders
                .iter()
                .filter(|o| {
                    query.matches_status(&o.status)
                        && query.matches_symbol(&o.symbol)
                        && query.matches_time(o.created_at)
                })
                .cloned()
                .collect();

            let limit = query.get_limit();
            let has_more = filtered.len() > limit;
            filtered.truncate(limit);

            OrderHistoryResponse {
                orders: filtered.clone(),
                total_count: filtered.len(),
                has_more,
            }
        }).unwrap_or_else(|| OrderHistoryResponse {
            orders: vec![],
            total_count: 0,
            has_more: false,
        });

        orders
    }

    /// Get a specific order
    pub fn get_order(&self, user_address: &str, order_id: &str) -> Option<OrderHistoryRecord> {
        self.order_history.get(user_address)?
            .iter()
            .find(|o| o.order_id == order_id)
            .cloned()
    }

    /// Get total order count
    pub fn total_order_count(&self) -> usize {
        self.total_orders.load(Ordering::Relaxed)
    }

    /// Clear order history for a user
    pub fn clear_user_orders(&self, user_address: &str) {
        if let Some((_, orders)) = self.order_history.remove(user_address) {
            self.total_orders.fetch_sub(orders.len(), Ordering::Relaxed);
        }
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Get statistics
    pub fn stats(&self) -> HistoryStats {
        HistoryStats {
            total_trades: self.total_trade_count(),
            total_orders: self.total_order_count(),
            symbols_with_trades: self.trade_history.len(),
            users_with_orders: self.order_history.len(),
        }
    }
}

impl Default for HistoryManager {
    fn default() -> Self {
        Self::new()
    }
}

/// History statistics
#[derive(Debug, Clone)]
pub struct HistoryStats {
    pub total_trades: usize,
    pub total_orders: usize,
    pub symbols_with_trades: usize,
    pub users_with_orders: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_trade(trade_id: &str, symbol: &str, price: &str) -> TradeRecord {
        TradeRecord {
            trade_id: trade_id.to_string(),
            symbol: symbol.to_string(),
            side: "buy".to_string(),
            price: price.to_string(),
            amount: "1.0".to_string(),
            maker_order_id: "maker1".to_string(),
            taker_order_id: "taker1".to_string(),
            maker_address: "0x1111".to_string(),
            taker_address: "0x2222".to_string(),
            maker_fee: "0.01".to_string(),
            taker_fee: "0.02".to_string(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    fn create_test_order(order_id: &str, user: &str, status: &str) -> OrderHistoryRecord {
        OrderHistoryRecord {
            order_id: order_id.to_string(),
            user_address: user.to_string(),
            symbol: "BTCUSDT".to_string(),
            side: "buy".to_string(),
            order_type: "limit".to_string(),
            price: "100.00".to_string(),
            original_amount: "1.0".to_string(),
            filled_amount: "0.0".to_string(),
            remaining_amount: "1.0".to_string(),
            status: status.to_string(),
            leverage: 1,
            created_at: chrono::Utc::now().timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
            avg_fill_price: None,
            trade_ids: vec![],
        }
    }

    #[test]
    fn test_store_and_get_trades() {
        let manager = HistoryManager::new();

        manager.store_trade(create_test_trade("t1", "BTCUSDT", "100.0"));
        manager.store_trade(create_test_trade("t2", "BTCUSDT", "101.0"));
        manager.store_trade(create_test_trade("t3", "ETHUSDT", "3000.0"));

        let btc_trades = manager.get_trades("BTCUSDT", &TradeHistoryQuery::default());
        assert_eq!(btc_trades.total_count, 2);

        let eth_trades = manager.get_trades("ETHUSDT", &TradeHistoryQuery::default());
        assert_eq!(eth_trades.total_count, 1);

        assert_eq!(manager.total_trade_count(), 3);
    }

    #[test]
    fn test_trade_limit() {
        let manager = HistoryManager::with_limits(2, 100);

        manager.store_trade(create_test_trade("t1", "BTCUSDT", "100.0"));
        manager.store_trade(create_test_trade("t2", "BTCUSDT", "101.0"));
        manager.store_trade(create_test_trade("t3", "BTCUSDT", "102.0"));

        let trades = manager.get_trades("BTCUSDT", &TradeHistoryQuery::default());
        assert_eq!(trades.total_count, 2);

        // Most recent should be first
        assert_eq!(trades.trades[0].trade_id, "t3");
        assert_eq!(trades.trades[1].trade_id, "t2");
    }

    #[test]
    fn test_store_and_get_orders() {
        let manager = HistoryManager::new();

        manager.store_order(create_test_order("o1", "0x1234", "open"));
        manager.store_order(create_test_order("o2", "0x1234", "filled"));
        manager.store_order(create_test_order("o3", "0x5678", "open"));

        let orders = manager.get_orders("0x1234", &OrderHistoryQuery::default());
        assert_eq!(orders.total_count, 2);

        let filtered = manager.get_orders("0x1234", &OrderHistoryQuery {
            status: Some("open".to_string()),
            ..Default::default()
        });
        assert_eq!(filtered.total_count, 1);
    }

    #[test]
    fn test_update_order() {
        let manager = HistoryManager::new();

        manager.store_order(create_test_order("o1", "0x1234", "open"));

        manager.update_order("0x1234", "o1", |order| {
            order.status = "filled".to_string();
            order.filled_amount = "1.0".to_string();
            order.remaining_amount = "0.0".to_string();
        });

        let order = manager.get_order("0x1234", "o1").unwrap();
        assert_eq!(order.status, "filled");
        assert_eq!(order.filled_amount, "1.0");
    }

    #[test]
    fn test_get_recent_trades() {
        let manager = HistoryManager::new();

        manager.store_trade(create_test_trade("t1", "BTCUSDT", "100.0"));
        manager.store_trade(create_test_trade("t2", "ETHUSDT", "3000.0"));

        let recent = manager.get_recent_trades(10);
        assert_eq!(recent.len(), 2);
    }
}
