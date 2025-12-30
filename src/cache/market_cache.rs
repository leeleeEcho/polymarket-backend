//! Market Cache for Prediction Markets
//!
//! Provides Redis-based caching for prediction market data:
//! - Market details and list
//! - Outcome probabilities
//! - User share holdings
//! - Market orderbook snapshots

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::debug;
use uuid::Uuid;

use super::keys::{ttl, CacheKey};
use super::redis_client::RedisClient;
use super::CacheError;

/// Cached market data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMarket {
    pub id: Uuid,
    pub question: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub status: String,
    pub resolution_source: Option<String>,
    pub end_time: Option<i64>,
    pub outcomes: Vec<CachedOutcome>,
    pub volume_24h: Decimal,
    pub total_volume: Decimal,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Cached outcome data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedOutcome {
    pub id: Uuid,
    pub name: String,
    pub probability: Decimal,
}

/// Cached user share holding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedShareHolding {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: String,
    pub amount: Decimal,
    pub avg_cost: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
}

/// Cached orderbook snapshot for prediction markets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPMOrderbook {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: String,
    pub bids: Vec<[String; 2]>, // [price, amount]
    pub asks: Vec<[String; 2]>,
    pub timestamp: i64,
}

/// Market cache service
pub struct MarketCache {
    redis: Arc<RedisClient>,
}

impl MarketCache {
    /// Create a new market cache
    pub fn new(redis: Arc<RedisClient>) -> Self {
        Self { redis }
    }

    // ==================== Market Data ====================

    /// Get cached market by ID
    pub async fn get_market(&self, market_id: Uuid) -> Result<Option<CachedMarket>, CacheError> {
        let key = CacheKey::market(&market_id.to_string());
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(json) => {
                let market: CachedMarket = serde_json::from_str(&json)?;
                debug!("Cache hit for market {}", market_id);
                Ok(Some(market))
            }
            None => {
                debug!("Cache miss for market {}", market_id);
                Ok(None)
            }
        }
    }

    /// Cache market data
    pub async fn set_market(&self, market: &CachedMarket) -> Result<(), CacheError> {
        let key = CacheKey::market(&market.id.to_string());
        let json = serde_json::to_string(market)?;
        self.redis.set_ex(&key, &json, ttl::MARKET).await?;
        debug!("Cached market {}", market.id);
        Ok(())
    }

    /// Invalidate market cache
    pub async fn invalidate_market(&self, market_id: Uuid) -> Result<(), CacheError> {
        let key = CacheKey::market(&market_id.to_string());
        self.redis.del(&key).await?;
        debug!("Invalidated market cache {}", market_id);
        Ok(())
    }

    /// Get cached market list
    pub async fn get_market_list(
        &self,
        category: Option<&str>,
    ) -> Result<Option<Vec<CachedMarket>>, CacheError> {
        let key = CacheKey::market_list(category);
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(json) => {
                let markets: Vec<CachedMarket> = serde_json::from_str(&json)?;
                debug!("Cache hit for market list (category: {:?})", category);
                Ok(Some(markets))
            }
            None => {
                debug!("Cache miss for market list (category: {:?})", category);
                Ok(None)
            }
        }
    }

    /// Cache market list
    pub async fn set_market_list(
        &self,
        markets: &[CachedMarket],
        category: Option<&str>,
    ) -> Result<(), CacheError> {
        let key = CacheKey::market_list(category);
        let json = serde_json::to_string(markets)?;
        self.redis.set_ex(&key, &json, ttl::MARKET_LIST).await?;
        debug!(
            "Cached {} markets (category: {:?})",
            markets.len(),
            category
        );
        Ok(())
    }

    /// Invalidate market list cache
    pub async fn invalidate_market_list(&self, category: Option<&str>) -> Result<(), CacheError> {
        let key = CacheKey::market_list(category);
        self.redis.del(&key).await?;
        // Also invalidate the "all" list
        if category.is_some() {
            let all_key = CacheKey::market_list(None);
            self.redis.del(&all_key).await?;
        }
        debug!("Invalidated market list cache (category: {:?})", category);
        Ok(())
    }

    // ==================== Probability Data ====================

    /// Get cached probability for an outcome
    pub async fn get_probability(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
    ) -> Result<Option<Decimal>, CacheError> {
        let key = CacheKey::probability(&market_id.to_string(), &outcome_id.to_string());
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(s) => {
                let prob: Decimal = s.parse().map_err(|_| {
                    CacheError::SerializationError("Invalid probability format".to_string())
                })?;
                Ok(Some(prob))
            }
            None => Ok(None),
        }
    }

    /// Cache probability for an outcome
    pub async fn set_probability(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        probability: Decimal,
    ) -> Result<(), CacheError> {
        let key = CacheKey::probability(&market_id.to_string(), &outcome_id.to_string());
        self.redis
            .set_ex(&key, &probability.to_string(), ttl::PROBABILITY)
            .await?;
        debug!(
            "Cached probability for market {} outcome {}: {}",
            market_id, outcome_id, probability
        );
        Ok(())
    }

    /// Get all probabilities for a market (as hash)
    pub async fn get_market_probabilities(
        &self,
        market_id: Uuid,
    ) -> Result<Option<Vec<(Uuid, Decimal)>>, CacheError> {
        let key = CacheKey::market_probabilities(&market_id.to_string());
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(json) => {
                let probs: Vec<(String, String)> = serde_json::from_str(&json)?;
                let mut result = Vec::with_capacity(probs.len());
                for (id, prob) in probs {
                    let uuid = Uuid::parse_str(&id).map_err(|_| {
                        CacheError::SerializationError("Invalid UUID".to_string())
                    })?;
                    let probability: Decimal = prob.parse().map_err(|_| {
                        CacheError::SerializationError("Invalid probability format".to_string())
                    })?;
                    result.push((uuid, probability));
                }
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    /// Cache all probabilities for a market
    pub async fn set_market_probabilities(
        &self,
        market_id: Uuid,
        probabilities: &[(Uuid, Decimal)],
    ) -> Result<(), CacheError> {
        let key = CacheKey::market_probabilities(&market_id.to_string());
        let data: Vec<(String, String)> = probabilities
            .iter()
            .map(|(id, prob)| (id.to_string(), prob.to_string()))
            .collect();
        let json = serde_json::to_string(&data)?;
        self.redis.set_ex(&key, &json, ttl::PROBABILITY).await?;
        debug!(
            "Cached {} probabilities for market {}",
            probabilities.len(),
            market_id
        );
        Ok(())
    }

    // ==================== User Shares ====================

    /// Get cached user shares
    pub async fn get_user_shares(
        &self,
        address: &str,
        market_id: Option<Uuid>,
    ) -> Result<Option<Vec<CachedShareHolding>>, CacheError> {
        let key = CacheKey::user_shares(address, market_id.map(|id| id.to_string()).as_deref());
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(json) => {
                let shares: Vec<CachedShareHolding> = serde_json::from_str(&json)?;
                debug!(
                    "Cache hit for user shares {} (market: {:?})",
                    address, market_id
                );
                Ok(Some(shares))
            }
            None => {
                debug!(
                    "Cache miss for user shares {} (market: {:?})",
                    address, market_id
                );
                Ok(None)
            }
        }
    }

    /// Cache user shares
    pub async fn set_user_shares(
        &self,
        address: &str,
        market_id: Option<Uuid>,
        shares: &[CachedShareHolding],
    ) -> Result<(), CacheError> {
        let key = CacheKey::user_shares(address, market_id.map(|id| id.to_string()).as_deref());
        let json = serde_json::to_string(shares)?;
        self.redis.set_ex(&key, &json, ttl::SHARES).await?;
        debug!(
            "Cached {} shares for user {} (market: {:?})",
            shares.len(),
            address,
            market_id
        );
        Ok(())
    }

    /// Invalidate user shares cache
    pub async fn invalidate_user_shares(
        &self,
        address: &str,
        market_id: Option<Uuid>,
    ) -> Result<(), CacheError> {
        let key = CacheKey::user_shares(address, market_id.map(|id| id.to_string()).as_deref());
        self.redis.del(&key).await?;

        // Also invalidate the "all markets" cache for this user
        if market_id.is_some() {
            let all_key = CacheKey::user_shares(address, None);
            self.redis.del(&all_key).await?;
        }

        debug!(
            "Invalidated shares cache for user {} (market: {:?})",
            address, market_id
        );
        Ok(())
    }

    // ==================== Orderbook ====================

    /// Get cached orderbook snapshot
    pub async fn get_orderbook(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        share_type: &str,
    ) -> Result<Option<CachedPMOrderbook>, CacheError> {
        let key = CacheKey::pm_orderbook_snapshot(
            &market_id.to_string(),
            &outcome_id.to_string(),
            share_type,
        );
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(json) => {
                let orderbook: CachedPMOrderbook = serde_json::from_str(&json)?;
                debug!(
                    "Cache hit for orderbook {}:{}:{}",
                    market_id, outcome_id, share_type
                );
                Ok(Some(orderbook))
            }
            None => {
                debug!(
                    "Cache miss for orderbook {}:{}:{}",
                    market_id, outcome_id, share_type
                );
                Ok(None)
            }
        }
    }

    /// Cache orderbook snapshot
    pub async fn set_orderbook(&self, orderbook: &CachedPMOrderbook) -> Result<(), CacheError> {
        let key = CacheKey::pm_orderbook_snapshot(
            &orderbook.market_id.to_string(),
            &orderbook.outcome_id.to_string(),
            &orderbook.share_type,
        );
        let json = serde_json::to_string(orderbook)?;
        self.redis
            .set_ex(&key, &json, ttl::MARKET_ORDERBOOK)
            .await?;
        debug!(
            "Cached orderbook {}:{}:{}",
            orderbook.market_id, orderbook.outcome_id, orderbook.share_type
        );
        Ok(())
    }

    /// Invalidate orderbook cache
    pub async fn invalidate_orderbook(
        &self,
        market_id: Uuid,
        outcome_id: Uuid,
        share_type: &str,
    ) -> Result<(), CacheError> {
        let key = CacheKey::pm_orderbook_snapshot(
            &market_id.to_string(),
            &outcome_id.to_string(),
            share_type,
        );
        self.redis.del(&key).await?;
        debug!(
            "Invalidated orderbook cache {}:{}:{}",
            market_id, outcome_id, share_type
        );
        Ok(())
    }

    // ==================== Volume ====================

    /// Increment market volume
    pub async fn incr_volume(&self, market_id: Uuid, amount: Decimal) -> Result<(), CacheError> {
        let key = CacheKey::market_volume(&market_id.to_string());
        // Use INCRBYFLOAT for atomic increment
        self.redis.incr_float(&key, amount.to_string()).await?;
        debug!("Incremented volume for market {} by {}", market_id, amount);
        Ok(())
    }

    /// Get cached volume
    pub async fn get_volume(&self, market_id: Uuid) -> Result<Option<Decimal>, CacheError> {
        let key = CacheKey::market_volume(&market_id.to_string());
        let data: Option<String> = self.redis.get(&key).await?;

        match data {
            Some(s) => {
                let volume: Decimal = s.parse().map_err(|_| {
                    CacheError::SerializationError("Invalid volume format".to_string())
                })?;
                Ok(Some(volume))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cached_market_serialization() {
        let market = CachedMarket {
            id: Uuid::new_v4(),
            question: "Will BTC reach $100k?".to_string(),
            description: Some("Test market".to_string()),
            category: Some("crypto".to_string()),
            status: "active".to_string(),
            resolution_source: None,
            end_time: Some(1735689600),
            outcomes: vec![CachedOutcome {
                id: Uuid::new_v4(),
                name: "Yes".to_string(),
                probability: Decimal::new(55, 2),
            }],
            volume_24h: Decimal::new(1000, 0),
            total_volume: Decimal::new(50000, 0),
            created_at: 1735600000,
            updated_at: 1735600000,
        };

        let json = serde_json::to_string(&market).unwrap();
        let parsed: CachedMarket = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, market.id);
        assert_eq!(parsed.question, market.question);
    }

    #[test]
    fn test_cached_share_serialization() {
        let share = CachedShareHolding {
            market_id: Uuid::new_v4(),
            outcome_id: Uuid::new_v4(),
            share_type: "yes".to_string(),
            amount: Decimal::new(100, 0),
            avg_cost: Decimal::new(55, 2),
            current_price: Decimal::new(60, 2),
            unrealized_pnl: Decimal::new(5, 0),
        };

        let json = serde_json::to_string(&share).unwrap();
        let parsed: CachedShareHolding = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.market_id, share.market_id);
        assert_eq!(parsed.amount, share.amount);
    }

    #[test]
    fn test_cached_orderbook_serialization() {
        let orderbook = CachedPMOrderbook {
            market_id: Uuid::new_v4(),
            outcome_id: Uuid::new_v4(),
            share_type: "yes".to_string(),
            bids: vec![
                ["0.55".to_string(), "100".to_string()],
                ["0.54".to_string(), "200".to_string()],
            ],
            asks: vec![
                ["0.56".to_string(), "150".to_string()],
                ["0.57".to_string(), "250".to_string()],
            ],
            timestamp: 1735600000,
        };

        let json = serde_json::to_string(&orderbook).unwrap();
        let parsed: CachedPMOrderbook = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.market_id, orderbook.market_id);
        assert_eq!(parsed.bids.len(), 2);
        assert_eq!(parsed.asks.len(), 2);
    }
}
