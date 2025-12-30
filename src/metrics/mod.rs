//! Metrics Module for Prediction Market Platform
//!
//! Provides Prometheus-compatible metrics for monitoring:
//! - API request metrics (latency, count, errors)
//! - Matching engine metrics (orders, trades, latency)
//! - Market metrics (active markets, volume, probabilities)
//! - Cache metrics (hits, misses, latency)
//! - Database metrics (query latency, connections)
//! - WebSocket metrics (connections, messages)

#![allow(dead_code)]

use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::{Matcher, PrometheusBuilder, PrometheusHandle};
use std::time::Instant;

/// Metric names as constants for consistency
pub mod names {
    // API Metrics
    pub const HTTP_REQUESTS_TOTAL: &str = "http_requests_total";
    pub const HTTP_REQUEST_DURATION_SECONDS: &str = "http_request_duration_seconds";
    pub const HTTP_REQUESTS_IN_FLIGHT: &str = "http_requests_in_flight";

    // Matching Engine Metrics
    pub const ORDERS_SUBMITTED_TOTAL: &str = "orders_submitted_total";
    pub const ORDERS_MATCHED_TOTAL: &str = "orders_matched_total";
    pub const ORDERS_CANCELLED_TOTAL: &str = "orders_cancelled_total";
    pub const ORDER_MATCH_DURATION_SECONDS: &str = "order_match_duration_seconds";
    pub const TRADES_EXECUTED_TOTAL: &str = "trades_executed_total";
    pub const TRADE_VOLUME_USDC: &str = "trade_volume_usdc";

    // Mint/Merge Metrics
    pub const MINT_OPERATIONS_TOTAL: &str = "mint_operations_total";
    pub const MERGE_OPERATIONS_TOTAL: &str = "merge_operations_total";

    // Market Metrics
    pub const ACTIVE_MARKETS: &str = "active_markets";
    pub const MARKET_VOLUME_24H_USDC: &str = "market_volume_24h_usdc";
    pub const MARKET_PROBABILITY: &str = "market_probability";
    pub const ORDERBOOK_DEPTH: &str = "orderbook_depth";
    pub const ORDERBOOK_SPREAD: &str = "orderbook_spread";

    // Cache Metrics
    pub const CACHE_HITS_TOTAL: &str = "cache_hits_total";
    pub const CACHE_MISSES_TOTAL: &str = "cache_misses_total";
    pub const CACHE_OPERATION_DURATION_SECONDS: &str = "cache_operation_duration_seconds";

    // Database Metrics
    pub const DB_QUERY_DURATION_SECONDS: &str = "db_query_duration_seconds";
    pub const DB_CONNECTIONS_ACTIVE: &str = "db_connections_active";
    pub const DB_CONNECTIONS_IDLE: &str = "db_connections_idle";

    // WebSocket Metrics
    pub const WS_CONNECTIONS_ACTIVE: &str = "ws_connections_active";
    pub const WS_MESSAGES_SENT_TOTAL: &str = "ws_messages_sent_total";
    pub const WS_MESSAGES_RECEIVED_TOTAL: &str = "ws_messages_received_total";

    // Settlement Metrics
    pub const SETTLEMENTS_TOTAL: &str = "settlements_total";
    pub const SETTLEMENT_AMOUNT_USDC: &str = "settlement_amount_usdc";

    // Oracle Metrics
    pub const ORACLE_UPDATES_TOTAL: &str = "oracle_updates_total";
    pub const ORACLE_ERRORS_TOTAL: &str = "oracle_errors_total";
}

/// Label keys
pub mod labels {
    pub const METHOD: &str = "method";
    pub const ENDPOINT: &str = "endpoint";
    pub const STATUS: &str = "status";
    pub const ORDER_SIDE: &str = "side";
    pub const ORDER_TYPE: &str = "order_type";
    pub const MATCH_TYPE: &str = "match_type";
    pub const MARKET_ID: &str = "market_id";
    pub const OUTCOME_ID: &str = "outcome_id";
    pub const SHARE_TYPE: &str = "share_type";
    pub const CACHE_TYPE: &str = "cache_type";
    pub const OPERATION: &str = "operation";
    pub const QUERY_TYPE: &str = "query_type";
    pub const SOURCE: &str = "source";
}

/// Initialize Prometheus metrics exporter
///
/// Returns a handle that can be used to render metrics
pub fn init_metrics() -> PrometheusHandle {
    // Configure histogram buckets
    let builder = PrometheusBuilder::new()
        // HTTP request duration buckets (in seconds)
        .set_buckets_for_metric(
            Matcher::Full(names::HTTP_REQUEST_DURATION_SECONDS.to_string()),
            &[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0],
        )
        .unwrap()
        // Order matching duration buckets (in seconds) - should be fast
        .set_buckets_for_metric(
            Matcher::Full(names::ORDER_MATCH_DURATION_SECONDS.to_string()),
            &[0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.5],
        )
        .unwrap()
        // Cache operation duration buckets
        .set_buckets_for_metric(
            Matcher::Full(names::CACHE_OPERATION_DURATION_SECONDS.to_string()),
            &[0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1],
        )
        .unwrap()
        // Database query duration buckets
        .set_buckets_for_metric(
            Matcher::Full(names::DB_QUERY_DURATION_SECONDS.to_string()),
            &[0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 5.0],
        )
        .unwrap();

    builder
        .install_recorder()
        .expect("Failed to install Prometheus recorder")
}

// ============================================================================
// HTTP Metrics
// ============================================================================

/// Record HTTP request
pub fn record_http_request(method: &str, endpoint: &str, status: u16, duration_secs: f64) {
    let status_str = status.to_string();
    counter!(
        names::HTTP_REQUESTS_TOTAL,
        labels::METHOD => method.to_string(),
        labels::ENDPOINT => endpoint.to_string(),
        labels::STATUS => status_str.clone()
    )
    .increment(1);

    histogram!(
        names::HTTP_REQUEST_DURATION_SECONDS,
        labels::METHOD => method.to_string(),
        labels::ENDPOINT => endpoint.to_string(),
        labels::STATUS => status_str
    )
    .record(duration_secs);
}

/// Track in-flight requests
pub fn set_http_requests_in_flight(count: i64) {
    gauge!(names::HTTP_REQUESTS_IN_FLIGHT).set(count as f64);
}

// ============================================================================
// Matching Engine Metrics
// ============================================================================

/// Record order submission
pub fn record_order_submitted(side: &str, order_type: &str) {
    counter!(
        names::ORDERS_SUBMITTED_TOTAL,
        labels::ORDER_SIDE => side.to_string(),
        labels::ORDER_TYPE => order_type.to_string()
    )
    .increment(1);
}

/// Record order matched
pub fn record_order_matched(match_type: &str) {
    counter!(
        names::ORDERS_MATCHED_TOTAL,
        labels::MATCH_TYPE => match_type.to_string()
    )
    .increment(1);
}

/// Record order cancelled
pub fn record_order_cancelled() {
    counter!(names::ORDERS_CANCELLED_TOTAL).increment(1);
}

/// Record order matching duration
pub fn record_order_match_duration(duration_secs: f64) {
    histogram!(names::ORDER_MATCH_DURATION_SECONDS).record(duration_secs);
}

/// Record trade execution
pub fn record_trade_executed(match_type: &str, volume_usdc: f64) {
    counter!(
        names::TRADES_EXECUTED_TOTAL,
        labels::MATCH_TYPE => match_type.to_string()
    )
    .increment(1);

    counter!(names::TRADE_VOLUME_USDC).increment(volume_usdc as u64);
}

/// Record mint operation
pub fn record_mint_operation() {
    counter!(names::MINT_OPERATIONS_TOTAL).increment(1);
}

/// Record merge operation
pub fn record_merge_operation() {
    counter!(names::MERGE_OPERATIONS_TOTAL).increment(1);
}

// ============================================================================
// Market Metrics
// ============================================================================

/// Set active markets count
pub fn set_active_markets(count: i64) {
    gauge!(names::ACTIVE_MARKETS).set(count as f64);
}

/// Set market 24h volume
pub fn set_market_volume_24h(market_id: &str, volume_usdc: f64) {
    gauge!(
        names::MARKET_VOLUME_24H_USDC,
        labels::MARKET_ID => market_id.to_string()
    )
    .set(volume_usdc);
}

/// Set market probability
pub fn set_market_probability(market_id: &str, outcome_id: &str, share_type: &str, probability: f64) {
    gauge!(
        names::MARKET_PROBABILITY,
        labels::MARKET_ID => market_id.to_string(),
        labels::OUTCOME_ID => outcome_id.to_string(),
        labels::SHARE_TYPE => share_type.to_string()
    )
    .set(probability);
}

/// Set orderbook depth
pub fn set_orderbook_depth(market_id: &str, outcome_id: &str, share_type: &str, side: &str, depth: i64) {
    gauge!(
        names::ORDERBOOK_DEPTH,
        labels::MARKET_ID => market_id.to_string(),
        labels::OUTCOME_ID => outcome_id.to_string(),
        labels::SHARE_TYPE => share_type.to_string(),
        labels::ORDER_SIDE => side.to_string()
    )
    .set(depth as f64);
}

/// Set orderbook spread
pub fn set_orderbook_spread(market_id: &str, outcome_id: &str, share_type: &str, spread: f64) {
    gauge!(
        names::ORDERBOOK_SPREAD,
        labels::MARKET_ID => market_id.to_string(),
        labels::OUTCOME_ID => outcome_id.to_string(),
        labels::SHARE_TYPE => share_type.to_string()
    )
    .set(spread);
}

// ============================================================================
// Cache Metrics
// ============================================================================

/// Record cache hit
pub fn record_cache_hit(cache_type: &str) {
    counter!(
        names::CACHE_HITS_TOTAL,
        labels::CACHE_TYPE => cache_type.to_string()
    )
    .increment(1);
}

/// Record cache miss
pub fn record_cache_miss(cache_type: &str) {
    counter!(
        names::CACHE_MISSES_TOTAL,
        labels::CACHE_TYPE => cache_type.to_string()
    )
    .increment(1);
}

/// Record cache operation duration
pub fn record_cache_operation(cache_type: &str, operation: &str, duration_secs: f64) {
    histogram!(
        names::CACHE_OPERATION_DURATION_SECONDS,
        labels::CACHE_TYPE => cache_type.to_string(),
        labels::OPERATION => operation.to_string()
    )
    .record(duration_secs);
}

// ============================================================================
// Database Metrics
// ============================================================================

/// Record database query duration
pub fn record_db_query(query_type: &str, duration_secs: f64) {
    histogram!(
        names::DB_QUERY_DURATION_SECONDS,
        labels::QUERY_TYPE => query_type.to_string()
    )
    .record(duration_secs);
}

/// Set database connection pool stats
pub fn set_db_connections(active: i64, idle: i64) {
    gauge!(names::DB_CONNECTIONS_ACTIVE).set(active as f64);
    gauge!(names::DB_CONNECTIONS_IDLE).set(idle as f64);
}

// ============================================================================
// WebSocket Metrics
// ============================================================================

/// Set active WebSocket connections
pub fn set_ws_connections(count: i64) {
    gauge!(names::WS_CONNECTIONS_ACTIVE).set(count as f64);
}

/// Record WebSocket message sent
pub fn record_ws_message_sent() {
    counter!(names::WS_MESSAGES_SENT_TOTAL).increment(1);
}

/// Record WebSocket message received
pub fn record_ws_message_received() {
    counter!(names::WS_MESSAGES_RECEIVED_TOTAL).increment(1);
}

// ============================================================================
// Settlement Metrics
// ============================================================================

/// Record settlement
pub fn record_settlement(settlement_type: &str, amount_usdc: f64) {
    counter!(
        names::SETTLEMENTS_TOTAL,
        labels::OPERATION => settlement_type.to_string()
    )
    .increment(1);

    counter!(names::SETTLEMENT_AMOUNT_USDC).increment(amount_usdc as u64);
}

// ============================================================================
// Oracle Metrics
// ============================================================================

/// Record oracle update
pub fn record_oracle_update(source: &str) {
    counter!(
        names::ORACLE_UPDATES_TOTAL,
        labels::SOURCE => source.to_string()
    )
    .increment(1);
}

/// Record oracle error
pub fn record_oracle_error(source: &str) {
    counter!(
        names::ORACLE_ERRORS_TOTAL,
        labels::SOURCE => source.to_string()
    )
    .increment(1);
}

// ============================================================================
// Timer Helper
// ============================================================================

/// Timer for measuring durations
pub struct Timer {
    start: Instant,
}

impl Timer {
    /// Create a new timer
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Get elapsed time in seconds
    pub fn elapsed_secs(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timer() {
        let timer = Timer::new();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let elapsed = timer.elapsed_secs();
        assert!(elapsed >= 0.01);
        assert!(elapsed < 0.1);
    }

    #[test]
    fn test_metric_names() {
        assert_eq!(names::HTTP_REQUESTS_TOTAL, "http_requests_total");
        assert_eq!(names::ORDERS_SUBMITTED_TOTAL, "orders_submitted_total");
        assert_eq!(names::CACHE_HITS_TOTAL, "cache_hits_total");
    }

    #[test]
    fn test_label_keys() {
        assert_eq!(labels::METHOD, "method");
        assert_eq!(labels::MARKET_ID, "market_id");
        assert_eq!(labels::CACHE_TYPE, "cache_type");
    }
}
