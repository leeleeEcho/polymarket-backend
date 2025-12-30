//! Redis Connection Management
//!
//! Provides connection pooling, automatic reconnection, and graceful degradation
//! when Redis is unavailable.

use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client, RedisError};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;

/// Redis connection configuration
#[derive(Debug, Clone)]
pub struct RedisConfig {
    /// Redis URL (e.g., redis://localhost:6379)
    pub url: String,
    /// Connection timeout in milliseconds
    pub timeout_ms: u64,
    /// Maximum retry attempts for operations
    pub max_retries: u32,
    /// Retry delay in milliseconds
    pub retry_delay_ms: u64,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            timeout_ms: 5000,
            max_retries: 3,
            retry_delay_ms: 100,
        }
    }
}

/// Redis client wrapper with connection management
pub struct RedisClient {
    config: RedisConfig,
    connection: Arc<RwLock<Option<ConnectionManager>>>,
    client: Client,
}

impl RedisClient {
    /// Create a new Redis client
    pub async fn new(config: RedisConfig) -> Result<Self, RedisError> {
        let client = Client::open(config.url.as_str())?;

        let redis_client = Self {
            config,
            connection: Arc::new(RwLock::new(None)),
            client,
        };

        // Try to establish initial connection
        redis_client.ensure_connected().await?;

        Ok(redis_client)
    }

    /// Create with default configuration
    pub async fn default() -> Result<Self, RedisError> {
        Self::new(RedisConfig::default()).await
    }

    /// Create from URL string
    pub async fn from_url(url: &str) -> Result<Self, RedisError> {
        Self::new(RedisConfig {
            url: url.to_string(),
            ..Default::default()
        }).await
    }

    /// Ensure connection is established
    async fn ensure_connected(&self) -> Result<(), RedisError> {
        let mut conn = self.connection.write().await;
        if conn.is_none() {
            tracing::info!("Establishing Redis connection to {}", self.config.url);
            let manager = ConnectionManager::new(self.client.clone()).await?;
            *conn = Some(manager);
            tracing::info!("Redis connection established");
        }
        Ok(())
    }

    /// Get connection manager, reconnecting if necessary
    pub async fn get_connection(&self) -> Result<ConnectionManager, RedisError> {
        self.ensure_connected().await?;
        let conn = self.connection.read().await;
        conn.clone().ok_or_else(|| {
            RedisError::from((
                redis::ErrorKind::IoError,
                "Connection not available",
            ))
        })
    }

    /// Execute operation with retry logic
    pub async fn with_retry<F, Fut, T>(&self, mut operation: F) -> Result<T, RedisError>
    where
        F: FnMut(ConnectionManager) -> Fut,
        Fut: std::future::Future<Output = Result<T, RedisError>>,
    {
        let mut last_error = None;

        for attempt in 0..self.config.max_retries {
            match self.get_connection().await {
                Ok(conn) => {
                    match operation(conn).await {
                        Ok(result) => return Ok(result),
                        Err(e) => {
                            tracing::warn!(
                                "Redis operation failed (attempt {}/{}): {}",
                                attempt + 1,
                                self.config.max_retries,
                                e
                            );
                            last_error = Some(e);

                            // Clear connection on error to force reconnect
                            if attempt < self.config.max_retries - 1 {
                                let mut conn = self.connection.write().await;
                                *conn = None;
                                tokio::time::sleep(Duration::from_millis(
                                    self.config.retry_delay_ms * (attempt as u64 + 1)
                                )).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Redis connection failed (attempt {}/{}): {}",
                        attempt + 1,
                        self.config.max_retries,
                        e
                    );
                    last_error = Some(e);

                    if attempt < self.config.max_retries - 1 {
                        tokio::time::sleep(Duration::from_millis(
                            self.config.retry_delay_ms * (attempt as u64 + 1)
                        )).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            RedisError::from((
                redis::ErrorKind::IoError,
                "Max retries exceeded",
            ))
        }))
    }

    // ==================== Basic Operations ====================

    /// GET operation
    pub async fn get<T: redis::FromRedisValue>(&self, key: &str) -> Result<Option<T>, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.get(&key).await }
        }).await
    }

    /// SET operation with optional expiry
    pub async fn set<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        key: &str,
        value: T,
        ttl_secs: Option<u64>,
    ) -> Result<(), RedisError> {
        let value = value.clone();
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let value = value.clone();
            async move {
                if let Some(ttl) = ttl_secs {
                    conn.set_ex(&key, value, ttl).await
                } else {
                    conn.set(&key, value).await
                }
            }
        }).await
    }

    /// SET with expiry (convenience method)
    pub async fn set_ex<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        key: &str,
        value: T,
        ttl_secs: u64,
    ) -> Result<(), RedisError> {
        self.set(key, value, Some(ttl_secs)).await
    }

    /// DELETE operation
    pub async fn del(&self, key: &str) -> Result<bool, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move {
                let count: i32 = conn.del(&key).await?;
                Ok(count > 0)
            }
        }).await
    }

    /// EXISTS operation
    pub async fn exists(&self, key: &str) -> Result<bool, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.exists(&key).await }
        }).await
    }

    /// EXPIRE operation
    pub async fn expire(&self, key: &str, ttl_secs: u64) -> Result<bool, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.expire(&key, ttl_secs as i64).await }
        }).await
    }

    /// INCR operation
    pub async fn incr(&self, key: &str) -> Result<i64, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.incr(&key, 1i64).await }
        }).await
    }

    /// INCRBY operation
    pub async fn incrby(&self, key: &str, amount: i64) -> Result<i64, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.incr(&key, amount).await }
        }).await
    }

    /// INCRBYFLOAT operation (for Decimal increments)
    pub async fn incr_float(&self, key: &str, amount: String) -> Result<String, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let amount = amount.clone();
            async move {
                redis::cmd("INCRBYFLOAT")
                    .arg(&key)
                    .arg(&amount)
                    .query_async(&mut conn)
                    .await
            }
        }).await
    }

    // ==================== Hash Operations ====================

    /// HGET operation
    pub async fn hget<T: redis::FromRedisValue>(
        &self,
        key: &str,
        field: &str,
    ) -> Result<Option<T>, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let field = field.to_string();
            async move { conn.hget(&key, &field).await }
        }).await
    }

    /// HSET operation
    pub async fn hset<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        key: &str,
        field: &str,
        value: T,
    ) -> Result<(), RedisError> {
        let value = value.clone();
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let field = field.to_string();
            let value = value.clone();
            async move { conn.hset(&key, &field, value).await }
        }).await
    }

    /// HGETALL operation
    pub async fn hgetall<T: redis::FromRedisValue>(
        &self,
        key: &str,
    ) -> Result<T, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.hgetall(&key).await }
        }).await
    }

    /// HDEL operation
    pub async fn hdel(&self, key: &str, field: &str) -> Result<bool, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let field = field.to_string();
            async move {
                let count: i32 = conn.hdel(&key, &field).await?;
                Ok(count > 0)
            }
        }).await
    }

    // ==================== Sorted Set Operations ====================

    /// ZADD operation
    pub async fn zadd<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        key: &str,
        score: f64,
        member: T,
    ) -> Result<bool, RedisError> {
        let member = member.clone();
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let member = member.clone();
            async move {
                let count: i32 = conn.zadd(&key, member, score).await?;
                Ok(count > 0)
            }
        }).await
    }

    /// ZREM operation
    pub async fn zrem<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        key: &str,
        member: T,
    ) -> Result<bool, RedisError> {
        let member = member.clone();
        self.with_retry(|mut conn| {
            let key = key.to_string();
            let member = member.clone();
            async move {
                let count: i32 = conn.zrem(&key, member).await?;
                Ok(count > 0)
            }
        }).await
    }

    /// ZRANGE operation (ascending order)
    pub async fn zrange<T: redis::FromRedisValue>(
        &self,
        key: &str,
        start: isize,
        stop: isize,
    ) -> Result<Vec<T>, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.zrange(&key, start, stop).await }
        }).await
    }

    /// ZREVRANGE operation (descending order)
    pub async fn zrevrange<T: redis::FromRedisValue>(
        &self,
        key: &str,
        start: isize,
        stop: isize,
    ) -> Result<Vec<T>, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.zrevrange(&key, start, stop).await }
        }).await
    }

    /// ZRANGEBYSCORE operation
    pub async fn zrangebyscore<T: redis::FromRedisValue>(
        &self,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<Vec<T>, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.zrangebyscore(&key, min, max).await }
        }).await
    }

    /// ZREMRANGEBYSCORE operation
    pub async fn zremrangebyscore(
        &self,
        key: &str,
        min: f64,
        max: f64,
    ) -> Result<i32, RedisError> {
        self.with_retry(|mut conn| {
            let key = key.to_string();
            async move { conn.zrembyscore(&key, min, max).await }
        }).await
    }

    // ==================== Pub/Sub Operations ====================

    /// PUBLISH operation
    pub async fn publish<T: redis::ToRedisArgs + Send + Sync + Clone>(
        &self,
        channel: &str,
        message: T,
    ) -> Result<i32, RedisError> {
        let message = message.clone();
        self.with_retry(|mut conn| {
            let channel = channel.to_string();
            let message = message.clone();
            async move { conn.publish(&channel, message).await }
        }).await
    }

    // ==================== Utility Operations ====================

    /// PING operation (health check)
    pub async fn ping(&self) -> Result<bool, RedisError> {
        self.with_retry(|mut conn| async move {
            let result: String = redis::cmd("PING").query_async(&mut conn).await?;
            Ok(result == "PONG")
        }).await
    }

    /// Check if Redis is available
    pub async fn is_available(&self) -> bool {
        self.ping().await.unwrap_or(false)
    }

    /// Get connection info
    pub fn config(&self) -> &RedisConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_redis_config_default() {
        let config = RedisConfig::default();
        assert_eq!(config.url, "redis://127.0.0.1:6379");
        assert_eq!(config.timeout_ms, 5000);
        assert_eq!(config.max_retries, 3);
    }
}
