//! Orderbook Implementation
//!
//! High-performance orderbook with lock-free concurrent access.

use super::types::*;
use dashmap::DashMap;
use parking_lot::RwLock;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
use uuid::Uuid;

/// A single market orderbook with concurrent access support
pub struct Orderbook {
    pub symbol: String,

    /// Bids sorted by price descending (highest first)
    /// Using RwLock for price level operations
    bids: RwLock<BTreeMap<PriceLevel, VecDeque<OrderEntry>>>,

    /// Asks sorted by price ascending (lowest first)
    asks: RwLock<BTreeMap<PriceLevel, VecDeque<OrderEntry>>>,

    /// Order ID to (side, price_level) mapping for O(1) cancellation
    order_index: DashMap<Uuid, (Side, PriceLevel)>,

    /// Last trade price
    last_trade_price: AtomicI64,

    /// Order count
    order_count: AtomicI64,
}

impl Orderbook {
    /// Create a new orderbook for a symbol
    pub fn new(symbol: String) -> Self {
        Self {
            symbol,
            bids: RwLock::new(BTreeMap::new()),
            asks: RwLock::new(BTreeMap::new()),
            order_index: DashMap::new(),
            last_trade_price: AtomicI64::new(0),
            order_count: AtomicI64::new(0),
        }
    }

    /// Get the symbol
    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// Get total order count
    pub fn order_count(&self) -> i64 {
        self.order_count.load(AtomicOrdering::Relaxed)
    }

    /// Get last trade price
    pub fn last_trade_price(&self) -> Option<Decimal> {
        let raw = self.last_trade_price.load(AtomicOrdering::Relaxed);
        if raw == 0 {
            None
        } else {
            Some(Decimal::from(raw) / Decimal::from(100_000_000))
        }
    }

    /// Set last trade price
    pub fn set_last_trade_price(&self, price: Decimal) {
        let raw = (price * Decimal::from(100_000_000)).to_string().parse::<i64>().unwrap_or(0);
        self.last_trade_price.store(raw, AtomicOrdering::Relaxed);
    }

    /// Get best bid price
    pub fn best_bid(&self) -> Option<Decimal> {
        let bids = self.bids.read();
        bids.keys().next_back().map(|p| p.to_decimal())
    }

    /// Get best ask price
    pub fn best_ask(&self) -> Option<Decimal> {
        let asks = self.asks.read();
        asks.keys().next().map(|p| p.to_decimal())
    }

    /// Get spread
    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask - bid),
            _ => None,
        }
    }

    /// Add an order to the orderbook
    pub fn add_order(&self, entry: OrderEntry) {
        let price_level = PriceLevel::from_decimal(entry.price);
        let side = entry.side;
        let order_id = entry.id;

        // Add to appropriate book
        match side {
            Side::Buy => {
                let mut bids = self.bids.write();
                bids.entry(price_level)
                    .or_insert_with(VecDeque::new)
                    .push_back(entry);
            }
            Side::Sell => {
                let mut asks = self.asks.write();
                asks.entry(price_level)
                    .or_insert_with(VecDeque::new)
                    .push_back(entry);
            }
        }

        // Add to index
        self.order_index.insert(order_id, (side, price_level));
        self.order_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// Cancel an order by ID
    pub fn cancel_order(&self, order_id: Uuid) -> Option<OrderEntry> {
        // Find and remove from index
        let (side, price_level) = self.order_index.remove(&order_id)?.1;

        // Remove from book
        let entry = match side {
            Side::Buy => {
                let mut bids = self.bids.write();
                if let Some(queue) = bids.get_mut(&price_level) {
                    let pos = queue.iter().position(|o| o.id == order_id);
                    if let Some(pos) = pos {
                        let entry = queue.remove(pos);
                        if queue.is_empty() {
                            bids.remove(&price_level);
                        }
                        entry
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            Side::Sell => {
                let mut asks = self.asks.write();
                if let Some(queue) = asks.get_mut(&price_level) {
                    let pos = queue.iter().position(|o| o.id == order_id);
                    if let Some(pos) = pos {
                        let entry = queue.remove(pos);
                        if queue.is_empty() {
                            asks.remove(&price_level);
                        }
                        entry
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
        };

        if entry.is_some() {
            self.order_count.fetch_sub(1, AtomicOrdering::Relaxed);
        }

        entry
    }

    /// Match an incoming order against the orderbook
    /// Returns (trades, remaining_amount)
    pub fn match_order(
        &self,
        taker_order_id: Uuid,
        taker_address: &str,
        side: Side,
        mut amount: Decimal,
        limit_price: Option<Decimal>,
        fee_config: &FeeConfig,
    ) -> (Vec<TradeExecution>, Decimal) {
        let mut trades = Vec::new();
        let now = chrono::Utc::now().timestamp_millis();

        match side {
            Side::Buy => {
                // Match against asks (lowest first)
                let mut asks = self.asks.write();
                let price_levels: Vec<PriceLevel> = asks.keys().cloned().collect();

                for price_level in price_levels {
                    if amount <= Decimal::ZERO {
                        break;
                    }

                    let level_price = price_level.to_decimal();

                    // Check price limit for limit orders
                    if let Some(limit) = limit_price {
                        if level_price > limit {
                            break;
                        }
                    }

                    if let Some(queue) = asks.get_mut(&price_level) {
                        while let Some(maker) = queue.front_mut() {
                            if amount <= Decimal::ZERO {
                                break;
                            }

                            let trade_amount = amount.min(maker.remaining_amount);
                            let trade_price = maker.price;

                            // Calculate fees
                            let trade_value = trade_amount * trade_price;
                            let maker_fee = trade_value * fee_config.maker_fee_rate;
                            let taker_fee = trade_value * fee_config.taker_fee_rate;

                            let trade = TradeExecution {
                                trade_id: Uuid::new_v4(),
                                maker_order_id: maker.id,
                                taker_order_id,
                                maker_address: maker.user_address.clone(),
                                price: trade_price,
                                amount: trade_amount,
                                maker_fee,
                                taker_fee,
                                timestamp: now,
                            };

                            trades.push(trade);
                            amount -= trade_amount;
                            maker.remaining_amount -= trade_amount;

                            // Update last trade price
                            self.set_last_trade_price(trade_price);

                            // Remove fully filled maker order
                            if maker.remaining_amount <= Decimal::ZERO {
                                let maker_id = maker.id;
                                queue.pop_front();
                                self.order_index.remove(&maker_id);
                                self.order_count.fetch_sub(1, AtomicOrdering::Relaxed);
                            }
                        }

                        if queue.is_empty() {
                            asks.remove(&price_level);
                        }
                    }
                }
            }
            Side::Sell => {
                // Match against bids (highest first)
                let mut bids = self.bids.write();
                let price_levels: Vec<PriceLevel> = bids.keys().rev().cloned().collect();

                for price_level in price_levels {
                    if amount <= Decimal::ZERO {
                        break;
                    }

                    let level_price = price_level.to_decimal();

                    // Check price limit for limit orders
                    if let Some(limit) = limit_price {
                        if level_price < limit {
                            break;
                        }
                    }

                    if let Some(queue) = bids.get_mut(&price_level) {
                        while let Some(maker) = queue.front_mut() {
                            if amount <= Decimal::ZERO {
                                break;
                            }

                            let trade_amount = amount.min(maker.remaining_amount);
                            let trade_price = maker.price;

                            // Calculate fees
                            let trade_value = trade_amount * trade_price;
                            let maker_fee = trade_value * fee_config.maker_fee_rate;
                            let taker_fee = trade_value * fee_config.taker_fee_rate;

                            let trade = TradeExecution {
                                trade_id: Uuid::new_v4(),
                                maker_order_id: maker.id,
                                taker_order_id,
                                maker_address: maker.user_address.clone(),
                                price: trade_price,
                                amount: trade_amount,
                                maker_fee,
                                taker_fee,
                                timestamp: now,
                            };

                            trades.push(trade);
                            amount -= trade_amount;
                            maker.remaining_amount -= trade_amount;

                            // Update last trade price
                            self.set_last_trade_price(trade_price);

                            // Remove fully filled maker order
                            if maker.remaining_amount <= Decimal::ZERO {
                                let maker_id = maker.id;
                                queue.pop_front();
                                self.order_index.remove(&maker_id);
                                self.order_count.fetch_sub(1, AtomicOrdering::Relaxed);
                            }
                        }

                        if queue.is_empty() {
                            bids.remove(&price_level);
                        }
                    }
                }
            }
        }

        (trades, amount)
    }

    /// Get orderbook snapshot
    pub fn snapshot(&self, depth: usize) -> OrderbookSnapshot {
        let mut bids_vec: Vec<[String; 2]> = Vec::new();
        let mut asks_vec: Vec<[String; 2]> = Vec::new();

        // Get bids (highest first)
        {
            let bids = self.bids.read();
            for (price_level, orders) in bids.iter().rev().take(depth) {
                let total: Decimal = orders.iter().map(|o| o.remaining_amount).sum();
                bids_vec.push([price_level.to_decimal().to_string(), total.to_string()]);
            }
        }

        // Get asks (lowest first)
        {
            let asks = self.asks.read();
            for (price_level, orders) in asks.iter().take(depth) {
                let total: Decimal = orders.iter().map(|o| o.remaining_amount).sum();
                asks_vec.push([price_level.to_decimal().to_string(), total.to_string()]);
            }
        }

        OrderbookSnapshot {
            symbol: self.symbol.clone(),
            bids: bids_vec,
            asks: asks_vec,
            last_price: self.last_trade_price(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    /// Get bid depth (total bids volume)
    pub fn bid_depth(&self) -> Decimal {
        let bids = self.bids.read();
        bids.values()
            .flat_map(|q| q.iter())
            .map(|o| o.remaining_amount)
            .sum()
    }

    /// Get ask depth (total asks volume)
    pub fn ask_depth(&self) -> Decimal {
        let asks = self.asks.read();
        asks.values()
            .flat_map(|q| q.iter())
            .map(|o| o.remaining_amount)
            .sum()
    }

    /// Check if an order exists
    pub fn has_order(&self, order_id: &Uuid) -> bool {
        self.order_index.contains_key(order_id)
    }

    /// Get order by ID
    pub fn get_order(&self, order_id: &Uuid) -> Option<OrderEntry> {
        let (side, price_level) = self.order_index.get(order_id)?.clone();

        match side {
            Side::Buy => {
                let bids = self.bids.read();
                bids.get(&price_level)?
                    .iter()
                    .find(|o| o.id == *order_id)
                    .cloned()
            }
            Side::Sell => {
                let asks = self.asks.read();
                asks.get(&price_level)?
                    .iter()
                    .find(|o| o.id == *order_id)
                    .cloned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn create_test_order(id: Uuid, price: Decimal, amount: Decimal, side: Side) -> OrderEntry {
        OrderEntry {
            id,
            user_address: "0x1234".to_string(),
            price,
            original_amount: amount,
            remaining_amount: amount,
            side,
            time_in_force: TimeInForce::GTC,
            timestamp: chrono::Utc::now().timestamp_millis(),
        }
    }

    #[test]
    fn test_add_and_cancel_order() {
        let book = Orderbook::new("BTCUSDT".to_string());
        let order_id = Uuid::new_v4();
        let order = create_test_order(order_id, dec!(100.0), dec!(1.0), Side::Buy);

        book.add_order(order);
        assert_eq!(book.order_count(), 1);
        assert!(book.has_order(&order_id));

        let cancelled = book.cancel_order(order_id);
        assert!(cancelled.is_some());
        assert_eq!(book.order_count(), 0);
        assert!(!book.has_order(&order_id));
    }

    #[test]
    fn test_best_bid_ask() {
        let book = Orderbook::new("BTCUSDT".to_string());

        // Add bids
        book.add_order(create_test_order(Uuid::new_v4(), dec!(100.0), dec!(1.0), Side::Buy));
        book.add_order(create_test_order(Uuid::new_v4(), dec!(101.0), dec!(1.0), Side::Buy));

        // Add asks
        book.add_order(create_test_order(Uuid::new_v4(), dec!(102.0), dec!(1.0), Side::Sell));
        book.add_order(create_test_order(Uuid::new_v4(), dec!(103.0), dec!(1.0), Side::Sell));

        assert_eq!(book.best_bid(), Some(dec!(101.0)));
        assert_eq!(book.best_ask(), Some(dec!(102.0)));
        assert_eq!(book.spread(), Some(dec!(1.0)));
    }

    #[test]
    fn test_match_buy_order() {
        let book = Orderbook::new("BTCUSDT".to_string());
        let fee_config = FeeConfig::default();

        // Add sell orders (asks)
        let ask1_id = Uuid::new_v4();
        book.add_order(create_test_order(ask1_id, dec!(100.0), dec!(1.0), Side::Sell));

        let ask2_id = Uuid::new_v4();
        book.add_order(create_test_order(ask2_id, dec!(101.0), dec!(2.0), Side::Sell));

        // Match a buy order
        let taker_id = Uuid::new_v4();
        let (trades, remaining) = book.match_order(
            taker_id,
            "0x5678",
            Side::Buy,
            dec!(1.5),
            Some(dec!(101.0)),
            &fee_config,
        );

        assert_eq!(trades.len(), 2);
        assert_eq!(remaining, dec!(0.0));

        // First trade should be at 100.0
        assert_eq!(trades[0].price, dec!(100.0));
        assert_eq!(trades[0].amount, dec!(1.0));

        // Second trade should be at 101.0
        assert_eq!(trades[1].price, dec!(101.0));
        assert_eq!(trades[1].amount, dec!(0.5));

        // Check remaining ask
        assert!(!book.has_order(&ask1_id)); // Fully filled
        assert!(book.has_order(&ask2_id));  // Partially filled
    }

    #[test]
    fn test_snapshot() {
        let book = Orderbook::new("BTCUSDT".to_string());

        book.add_order(create_test_order(Uuid::new_v4(), dec!(100.0), dec!(1.0), Side::Buy));
        book.add_order(create_test_order(Uuid::new_v4(), dec!(100.0), dec!(2.0), Side::Buy));
        book.add_order(create_test_order(Uuid::new_v4(), dec!(102.0), dec!(1.5), Side::Sell));

        let snapshot = book.snapshot(10);

        assert_eq!(snapshot.symbol, "BTCUSDT");
        assert_eq!(snapshot.bids.len(), 1);
        assert_eq!(snapshot.asks.len(), 1);
        assert_eq!(snapshot.bids[0][1], "3.0"); // Total bid at 100.0 (1.0 + 2.0)
        assert_eq!(snapshot.asks[0][1], "1.5");
    }
}
