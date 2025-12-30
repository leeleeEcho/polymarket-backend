//! Matching Engine Types
//!
//! Shared types and DTOs for the matching engine.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use uuid::Uuid;

// ============================================================================
// Price Level
// ============================================================================

/// Price level with 8 decimal precision for exact comparison
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PriceLevel(i64);

impl PriceLevel {
    /// Create a PriceLevel from a Decimal price
    pub fn from_decimal(price: Decimal) -> Self {
        let scaled = price * Decimal::from(100_000_000);
        let truncated = scaled.trunc();
        let value = truncated.mantissa() / 10i128.pow(truncated.scale() as u32);
        PriceLevel(value as i64)
    }

    /// Convert back to Decimal
    pub fn to_decimal(&self) -> Decimal {
        Decimal::from(self.0) / Decimal::from(100_000_000)
    }

    /// Get raw value
    pub fn raw(&self) -> i64 {
        self.0
    }
}

impl Ord for PriceLevel {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for PriceLevel {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// ============================================================================
// Order Types
// ============================================================================

/// Order side
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Buy,
    Sell,
}

impl std::fmt::Display for Side {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Side::Buy => write!(f, "buy"),
            Side::Sell => write!(f, "sell"),
        }
    }
}

/// Order type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Limit,
    Market,
}

/// Time in force
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TimeInForce {
    /// Good Till Cancel
    GTC,
    /// Immediate or Cancel
    IOC,
    /// Fill or Kill
    FOK,
}

impl Default for TimeInForce {
    fn default() -> Self {
        TimeInForce::GTC
    }
}

/// Order status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderStatus {
    /// Order is active in the orderbook
    Open,
    /// Order is partially filled
    PartiallyFilled,
    /// Order is completely filled
    Filled,
    /// Order was cancelled
    Cancelled,
    /// Order was rejected
    Rejected,
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::Open => write!(f, "open"),
            OrderStatus::PartiallyFilled => write!(f, "partially_filled"),
            OrderStatus::Filled => write!(f, "filled"),
            OrderStatus::Cancelled => write!(f, "cancelled"),
            OrderStatus::Rejected => write!(f, "rejected"),
        }
    }
}

// ============================================================================
// Order Entry (in orderbook)
// ============================================================================

/// An order entry in the orderbook
#[derive(Debug, Clone)]
pub struct OrderEntry {
    pub id: Uuid,
    pub user_address: String,
    pub price: Decimal,
    pub original_amount: Decimal,
    pub remaining_amount: Decimal,
    pub side: Side,
    pub time_in_force: TimeInForce,
    pub timestamp: i64,
}

// ============================================================================
// Trade Execution
// ============================================================================

/// A trade execution result
#[derive(Debug, Clone, Serialize)]
pub struct TradeExecution {
    pub trade_id: Uuid,
    pub maker_order_id: Uuid,
    pub taker_order_id: Uuid,
    pub maker_address: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub maker_fee: Decimal,
    pub taker_fee: Decimal,
    pub timestamp: i64,
}

/// Trade event for broadcasting
#[derive(Debug, Clone, Serialize)]
pub struct TradeEvent {
    pub symbol: String,
    pub trade_id: Uuid,
    pub maker_order_id: Uuid,
    pub taker_order_id: Uuid,
    pub maker_address: String,
    pub taker_address: String,
    pub side: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub maker_fee: Decimal,
    pub taker_fee: Decimal,
    pub timestamp: i64,
}

// ============================================================================
// Match Result
// ============================================================================

/// Result of order matching
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub order_id: Uuid,
    pub status: OrderStatus,
    pub filled_amount: Decimal,
    pub remaining_amount: Decimal,
    pub average_price: Option<Decimal>,
    pub trades: Vec<TradeExecution>,
}

// ============================================================================
// Orderbook Snapshot
// ============================================================================

/// Orderbook snapshot for API response
#[derive(Debug, Clone, Serialize)]
pub struct OrderbookSnapshot {
    pub symbol: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    pub last_price: Option<Decimal>,
    pub timestamp: i64,
}

/// Orderbook update event for broadcasting
#[derive(Debug, Clone, Serialize)]
pub struct OrderbookUpdate {
    pub symbol: String,
    pub bids: Vec<[String; 2]>,
    pub asks: Vec<[String; 2]>,
    pub timestamp: i64,
}

// ============================================================================
// Trade Record (for history)
// ============================================================================

/// Trade record for history storage
#[derive(Debug, Clone, Serialize)]
pub struct TradeRecord {
    pub trade_id: String,
    pub symbol: String,
    pub side: String,
    pub price: String,
    pub amount: String,
    pub maker_order_id: String,
    pub taker_order_id: String,
    pub maker_address: String,
    pub taker_address: String,
    pub maker_fee: String,
    pub taker_fee: String,
    pub timestamp: i64,
}

impl From<&TradeEvent> for TradeRecord {
    fn from(event: &TradeEvent) -> Self {
        TradeRecord {
            trade_id: event.trade_id.to_string(),
            symbol: event.symbol.clone(),
            side: event.side.clone(),
            price: event.price.to_string(),
            amount: event.amount.to_string(),
            maker_order_id: event.maker_order_id.to_string(),
            taker_order_id: event.taker_order_id.to_string(),
            maker_address: event.maker_address.clone(),
            taker_address: event.taker_address.clone(),
            maker_fee: event.maker_fee.to_string(),
            taker_fee: event.taker_fee.to_string(),
            timestamp: event.timestamp,
        }
    }
}

// ============================================================================
// Order History Record
// ============================================================================

/// Order history record for storage
#[derive(Debug, Clone, Serialize)]
pub struct OrderHistoryRecord {
    pub order_id: String,
    pub user_address: String,
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    pub price: String,
    pub original_amount: String,
    pub filled_amount: String,
    pub remaining_amount: String,
    pub status: String,
    pub leverage: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub avg_fill_price: Option<String>,
    pub trade_ids: Vec<String>,
}

// ============================================================================
// Query Types
// ============================================================================

/// Trade history query parameters
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TradeHistoryQuery {
    pub limit: Option<usize>,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

impl TradeHistoryQuery {
    pub fn get_limit(&self) -> usize {
        self.limit.unwrap_or(50).min(100).max(1)
    }
}

/// Trade history response
#[derive(Debug, Clone, Serialize)]
pub struct TradeHistoryResponse {
    pub trades: Vec<TradeRecord>,
    pub total_count: usize,
    pub has_more: bool,
}

/// Order history query parameters
#[derive(Debug, Clone, Deserialize, Default)]
pub struct OrderHistoryQuery {
    pub status: Option<String>,
    pub symbol: Option<String>,
    pub limit: Option<usize>,
    pub before: Option<i64>,
    pub after: Option<i64>,
}

impl OrderHistoryQuery {
    pub fn get_limit(&self) -> usize {
        self.limit.unwrap_or(50).min(100).max(1)
    }

    pub fn matches_status(&self, status: &str) -> bool {
        match &self.status {
            None => true,
            Some(filter) => filter == "all" || status == filter,
        }
    }

    pub fn matches_symbol(&self, symbol: &str) -> bool {
        match &self.symbol {
            None => true,
            Some(filter) => symbol == filter,
        }
    }

    pub fn matches_time(&self, timestamp: i64) -> bool {
        let matches_before = self.before.map_or(true, |ts| timestamp < ts);
        let matches_after = self.after.map_or(true, |ts| timestamp > ts);
        matches_before && matches_after
    }
}

/// Order history response
#[derive(Debug, Clone, Serialize)]
pub struct OrderHistoryResponse {
    pub orders: Vec<OrderHistoryRecord>,
    pub total_count: usize,
    pub has_more: bool,
}

// ============================================================================
// Error Types
// ============================================================================

/// Matching engine errors
#[derive(Debug, thiserror::Error)]
pub enum MatchingError {
    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    #[error("Order not found: {0}")]
    OrderNotFound(String),

    #[error("Invalid price: {0}")]
    InvalidPrice(String),

    #[error("Invalid amount: {0}")]
    InvalidAmount(String),

    #[error("Invalid side: {0}")]
    InvalidSide(String),

    #[error("Insufficient liquidity")]
    InsufficientLiquidity,

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

// ============================================================================
// Fee Configuration
// ============================================================================

/// Fee rates for trading
#[derive(Debug, Clone)]
pub struct FeeConfig {
    pub maker_fee_rate: Decimal,
    pub taker_fee_rate: Decimal,
}

impl Default for FeeConfig {
    fn default() -> Self {
        Self {
            maker_fee_rate: Decimal::new(2, 4),  // 0.02%
            taker_fee_rate: Decimal::new(5, 4),  // 0.05%
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_price_level_conversion() {
        let price = dec!(97500.50);
        let level = PriceLevel::from_decimal(price);
        let back = level.to_decimal();
        assert_eq!(price, back);
    }

    #[test]
    fn test_price_level_ordering() {
        let p1 = PriceLevel::from_decimal(dec!(100.0));
        let p2 = PriceLevel::from_decimal(dec!(200.0));
        assert!(p1 < p2);
    }

    #[test]
    fn test_order_history_query() {
        let query = OrderHistoryQuery {
            status: Some("filled".to_string()),
            symbol: Some("BTCUSDT".to_string()),
            limit: Some(10),
            before: None,
            after: None,
        };

        assert_eq!(query.get_limit(), 10);
        assert!(query.matches_status("filled"));
        assert!(!query.matches_status("open"));
        assert!(query.matches_symbol("BTCUSDT"));
        assert!(!query.matches_symbol("ETHUSDT"));
    }
}
