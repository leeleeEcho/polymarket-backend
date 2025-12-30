//! Price Oracle Service for Prediction Markets
//!
//! Provides probability updates from multiple sources:
//! - Orderbook-based: Calculate weighted mid price from orderbook
//! - External oracle: Fetch from external price feeds (Chainlink, UMA, etc.)
//! - Manual: Admin can set probability directly

#![allow(dead_code)]

use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::services::matching::MatchingEngine;

/// Oracle error types
#[derive(Debug, thiserror::Error)]
pub enum OracleError {
    #[error("Market not found: {0}")]
    MarketNotFound(Uuid),

    #[error("Outcome not found: {0}")]
    OutcomeNotFound(Uuid),

    #[error("Market not active: {0}")]
    MarketNotActive(Uuid),

    #[error("Invalid probability: {0}. Must be between 0.01 and 0.99")]
    InvalidProbability(Decimal),

    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    #[error("External oracle error: {0}")]
    ExternalOracleError(String),
}

/// Price update event for WebSocket broadcast
#[derive(Debug, Clone)]
pub struct PriceUpdateEvent {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub probability: Decimal,
    pub source: PriceSource,
    pub timestamp: i64,
}

/// Source of price/probability update
#[derive(Debug, Clone, PartialEq)]
pub enum PriceSource {
    /// Calculated from orderbook
    Orderbook,
    /// From external oracle (Chainlink, UMA, etc.)
    External(String),
    /// Manually set by admin
    Manual,
    /// From trade execution
    Trade,
}

impl std::fmt::Display for PriceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PriceSource::Orderbook => write!(f, "orderbook"),
            PriceSource::External(name) => write!(f, "external:{}", name),
            PriceSource::Manual => write!(f, "manual"),
            PriceSource::Trade => write!(f, "trade"),
        }
    }
}

/// Price Oracle Service
pub struct PriceOracle {
    pool: PgPool,
    matching_engine: Arc<MatchingEngine>,
    price_sender: broadcast::Sender<PriceUpdateEvent>,
}

impl PriceOracle {
    /// Create a new PriceOracle
    pub fn new(pool: PgPool, matching_engine: Arc<MatchingEngine>) -> Self {
        let (price_sender, _) = broadcast::channel(1000);
        Self {
            pool,
            matching_engine,
            price_sender,
        }
    }

    /// Subscribe to price updates
    pub fn subscribe(&self) -> broadcast::Receiver<PriceUpdateEvent> {
        self.price_sender.subscribe()
    }

    /// Update probability from orderbook data
    ///
    /// Calculates weighted mid price from best bid/ask
    pub async fn update_from_orderbook(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
    ) -> Result<Decimal, OracleError> {
        // Build orderbook key
        let orderbook_key = format!("{}:{}:yes", market_id, outcome_id);

        // Get orderbook snapshot
        let snapshot = self.matching_engine.get_orderbook(&orderbook_key, 5);

        let probability = match snapshot {
            Ok(snap) => {
                // Parse best bid and ask
                let best_bid = snap.bids.first()
                    .and_then(|[price, _]| price.parse::<Decimal>().ok())
                    .unwrap_or(Decimal::ZERO);
                let best_ask = snap.asks.first()
                    .and_then(|[price, _]| price.parse::<Decimal>().ok())
                    .unwrap_or(Decimal::ONE);

                // Calculate weighted mid price
                if best_bid > Decimal::ZERO && best_ask < Decimal::ONE {
                    // Get volumes for weighting
                    let bid_vol = snap.bids.first()
                        .and_then(|[_, vol]| vol.parse::<Decimal>().ok())
                        .unwrap_or(Decimal::ONE);
                    let ask_vol = snap.asks.first()
                        .and_then(|[_, vol]| vol.parse::<Decimal>().ok())
                        .unwrap_or(Decimal::ONE);

                    // Weighted mid price: (bid * ask_vol + ask * bid_vol) / (bid_vol + ask_vol)
                    let total_vol = bid_vol + ask_vol;
                    if total_vol > Decimal::ZERO {
                        (best_bid * ask_vol + best_ask * bid_vol) / total_vol
                    } else {
                        (best_bid + best_ask) / Decimal::TWO
                    }
                } else if best_bid > Decimal::ZERO {
                    best_bid
                } else if best_ask < Decimal::ONE {
                    best_ask
                } else {
                    // No orderbook data, keep current probability
                    return self.get_current_probability(outcome_id).await;
                }
            }
            Err(_) => {
                // No orderbook, keep current probability
                return self.get_current_probability(outcome_id).await;
            }
        };

        // Clamp to valid range (0.01 to 0.99)
        let min_prob = Decimal::new(1, 2);  // 0.01
        let max_prob = Decimal::new(99, 2); // 0.99
        let probability = probability.max(min_prob).min(max_prob);

        // Update database
        self.update_probability(market_id, outcome_id, probability, PriceSource::Orderbook).await?;

        Ok(probability)
    }

    /// Update probability from trade execution
    pub async fn update_from_trade(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        trade_price: Decimal,
    ) -> Result<Decimal, OracleError> {
        // Trade price is the new probability for Yes shares
        let min_prob = Decimal::new(1, 2);  // 0.01
        let max_prob = Decimal::new(99, 2); // 0.99
        let probability = trade_price.max(min_prob).min(max_prob);

        self.update_probability(market_id, outcome_id, probability, PriceSource::Trade).await?;

        Ok(probability)
    }

    /// Set probability manually (admin only)
    pub async fn set_probability_manual(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        probability: Decimal,
    ) -> Result<(), OracleError> {
        // Validate probability range
        let min_prob = Decimal::new(1, 2);  // 0.01
        let max_prob = Decimal::new(99, 2); // 0.99
        if probability < min_prob || probability > max_prob {
            return Err(OracleError::InvalidProbability(probability));
        }

        // Verify market is active
        self.verify_market_active(market_id).await?;

        self.update_probability(market_id, outcome_id, probability, PriceSource::Manual).await
    }

    /// Fetch probability from external oracle (placeholder for integration)
    pub async fn fetch_from_external(
        &self,
        market_id: Uuid,
        oracle_name: &str,
    ) -> Result<Decimal, OracleError> {
        // Verify market exists
        let market: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, resolution_source FROM markets WHERE id = $1"
        )
        .bind(market_id)
        .fetch_optional(&self.pool)
        .await?;

        let (_, _resolution_source) = market.ok_or(OracleError::MarketNotFound(market_id))?;

        // TODO: Implement actual oracle integrations
        // For now, this is a placeholder that returns an error
        match oracle_name.to_lowercase().as_str() {
            "chainlink" => {
                warn!("Chainlink oracle not yet implemented for market {}", market_id);
                Err(OracleError::ExternalOracleError(
                    "Chainlink integration not yet implemented".to_string()
                ))
            }
            "uma" => {
                warn!("UMA oracle not yet implemented for market {}", market_id);
                Err(OracleError::ExternalOracleError(
                    "UMA integration not yet implemented".to_string()
                ))
            }
            "pyth" => {
                warn!("Pyth oracle not yet implemented for market {}", market_id);
                Err(OracleError::ExternalOracleError(
                    "Pyth integration not yet implemented".to_string()
                ))
            }
            _ => {
                Err(OracleError::ExternalOracleError(
                    format!("Unknown oracle: {}. Supported: chainlink, uma, pyth", oracle_name)
                ))
            }
        }
    }

    /// Batch update all market probabilities from orderbook
    pub async fn refresh_all_from_orderbook(&self) -> Result<usize, OracleError> {
        // Get all active markets with outcomes
        let markets: Vec<(Uuid, Uuid)> = sqlx::query_as(
            r#"
            SELECT m.id, o.id
            FROM markets m
            JOIN outcomes o ON o.market_id = m.id
            WHERE m.status = 'active' AND o.share_type = 'yes'
            "#
        )
        .fetch_all(&self.pool)
        .await?;

        let mut updated_count = 0;
        for (market_id, outcome_id) in markets {
            match self.update_from_orderbook(market_id, outcome_id).await {
                Ok(_) => updated_count += 1,
                Err(e) => {
                    debug!("Failed to update probability for market {}: {}", market_id, e);
                }
            }
        }

        info!("Refreshed {} market probabilities from orderbook", updated_count);
        Ok(updated_count)
    }

    // =========================================================================
    // Private helpers
    // =========================================================================

    /// Update probability in database and broadcast
    async fn update_probability(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        probability: Decimal,
        source: PriceSource,
    ) -> Result<(), OracleError> {
        // Update Yes outcome probability
        sqlx::query(
            "UPDATE outcomes SET probability = $1 WHERE id = $2"
        )
        .bind(probability)
        .bind(outcome_id)
        .execute(&self.pool)
        .await?;

        // Update complement (No) outcome probability
        let complement_prob = Decimal::ONE - probability;
        sqlx::query(
            r#"
            UPDATE outcomes
            SET probability = $1
            WHERE market_id = $2 AND id != $3
            "#
        )
        .bind(complement_prob)
        .bind(market_id)
        .bind(outcome_id)
        .execute(&self.pool)
        .await?;

        // Broadcast price update event
        let event = PriceUpdateEvent {
            market_id,
            outcome_id,
            probability,
            source: source.clone(),
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        if let Err(e) = self.price_sender.send(event) {
            debug!("No subscribers for price update: {}", e);
        }

        debug!(
            "Updated probability: market={}, outcome={}, prob={}, source={}",
            market_id, outcome_id, probability, source
        );

        Ok(())
    }

    /// Get current probability for an outcome
    async fn get_current_probability(&self, outcome_id: Uuid) -> Result<Decimal, OracleError> {
        let result: Option<(Decimal,)> = sqlx::query_as(
            "SELECT probability FROM outcomes WHERE id = $1"
        )
        .bind(outcome_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((prob,)) => Ok(prob),
            None => Err(OracleError::OutcomeNotFound(outcome_id)),
        }
    }

    /// Verify market is active
    async fn verify_market_active(&self, market_id: Uuid) -> Result<(), OracleError> {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT status::text FROM markets WHERE id = $1"
        )
        .bind(market_id)
        .fetch_optional(&self.pool)
        .await?;

        match result {
            Some((status,)) if status == "active" => Ok(()),
            Some(_) => Err(OracleError::MarketNotActive(market_id)),
            None => Err(OracleError::MarketNotFound(market_id)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_source_display() {
        assert_eq!(PriceSource::Orderbook.to_string(), "orderbook");
        assert_eq!(PriceSource::Manual.to_string(), "manual");
        assert_eq!(PriceSource::Trade.to_string(), "trade");
        assert_eq!(PriceSource::External("chainlink".to_string()).to_string(), "external:chainlink");
    }

    #[test]
    fn test_probability_bounds() {
        let min_prob = Decimal::new(1, 2);  // 0.01
        let max_prob = Decimal::new(99, 2); // 0.99

        // Test that probabilities are clamped to valid range
        let low = Decimal::new(1, 3).max(min_prob).min(max_prob); // 0.001
        assert_eq!(low, min_prob);

        let high = Decimal::new(999, 3).max(min_prob).min(max_prob); // 0.999
        assert_eq!(high, max_prob);

        let valid = Decimal::new(55, 2).max(min_prob).min(max_prob); // 0.55
        assert_eq!(valid, Decimal::new(55, 2));
    }
}
