//! Order Flow Orchestrator
//!
//! Orchestrates the complete order processing flow:
//! 1. Receive order from API
//! 2. Execute matching via MatchingEngine
//! 3. Process match results
//! 4. Persist to database asynchronously
//! 5. Broadcast updates via WebSocket

use super::engine::MatchingEngine;
use super::types::*;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::services::position::PositionService;
use crate::models::PositionSide;

/// Order flow orchestrator
///
/// Connects matching engine with database persistence and WebSocket broadcasting.
/// All database operations are async and non-blocking.
pub struct OrderFlowOrchestrator {
    /// The matching engine
    engine: Arc<MatchingEngine>,

    /// Database connection pool
    pool: PgPool,

    /// Trade event receiver for persistence
    trade_receiver: Option<broadcast::Receiver<TradeEvent>>,
}

impl OrderFlowOrchestrator {
    /// Create a new orchestrator
    pub fn new(engine: Arc<MatchingEngine>, pool: PgPool) -> Self {
        let trade_receiver = Some(engine.subscribe_trades());

        info!("OrderFlowOrchestrator initialized");

        Self {
            engine,
            pool,
            trade_receiver,
        }
    }

    /// Get reference to matching engine
    pub fn engine(&self) -> &Arc<MatchingEngine> {
        &self.engine
    }

    /// Start the background persistence worker
    pub fn start_persistence_worker(mut self) -> Arc<MatchingEngine> {
        let pool = self.pool.clone();
        let engine = Arc::clone(&self.engine);
        let receiver = self.trade_receiver.take();

        if let Some(mut rx) = receiver {
            tokio::spawn(async move {
                info!("Trade persistence worker started");

                loop {
                    match rx.recv().await {
                        Ok(trade) => {
                            if let Err(e) = Self::persist_trade(&pool, &trade).await {
                                error!("Failed to persist trade: {}", e);
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!("Trade persistence lagged {} messages", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("Trade channel closed, stopping persistence worker");
                            break;
                        }
                    }
                }
            });
        }

        engine
    }

    /// Process a new order
    ///
    /// This is the main entry point for order processing:
    /// 1. Validates the order
    /// 2. Submits to matching engine
    /// 3. Spawns async task for database persistence
    /// 4. Returns immediately with match result
    pub async fn process_order(
        &self,
        symbol: &str,
        user_address: &str,
        side: Side,
        order_type: OrderType,
        amount: Decimal,
        price: Option<Decimal>,
        leverage: u32,
    ) -> Result<MatchResult, MatchingError> {
        debug!(
            "Processing order: symbol={}, user={}, side={:?}, type={:?}, amount={}, price={:?}",
            symbol, user_address, side, order_type, amount, price
        );

        // Generate order ID
        let order_id = Uuid::new_v4();

        // Submit to matching engine (synchronous, in-memory)
        let result = self.engine.submit_order(
            order_id,
            symbol,
            user_address,
            side,
            order_type,
            amount,
            price,
            leverage,
        )?;

        // Spawn async task for database persistence
        let pool = self.pool.clone();
        let symbol = symbol.to_string();
        let user_address = user_address.to_string();
        let result_clone = result.clone();

        tokio::spawn(async move {
            if let Err(e) = Self::persist_order(
                &pool,
                &symbol,
                &user_address,
                &result_clone,
                side,
                order_type,
                amount,
                price,
                leverage,
            ).await {
                error!("Failed to persist order {}: {}", order_id, e);
            }
        });

        info!(
            "Order processed: id={}, status={:?}, filled={}",
            result.order_id, result.status, result.filled_amount
        );

        Ok(result)
    }

    /// Cancel an order
    pub async fn cancel_order(
        &self,
        symbol: &str,
        order_id: Uuid,
        user_address: &str,
    ) -> Result<bool, MatchingError> {
        debug!("Cancelling order: id={}, symbol={}", order_id, symbol);

        // Cancel in matching engine
        let cancelled = self.engine.cancel_order(symbol, order_id, user_address)?;

        if cancelled {
            // Update database asynchronously
            let pool = self.pool.clone();
            let order_id = order_id;

            tokio::spawn(async move {
                if let Err(e) = Self::update_order_status(&pool, order_id, "cancelled").await {
                    error!("Failed to update order status: {}", e);
                }
            });

            info!("Order cancelled: id={}", order_id);
        }

        Ok(cancelled)
    }

    /// Get orderbook
    pub fn get_orderbook(&self, symbol: &str, depth: usize) -> Result<OrderbookSnapshot, MatchingError> {
        self.engine.get_orderbook(symbol, depth)
    }

    /// Get trade history
    pub fn get_trades(&self, symbol: &str, query: &TradeHistoryQuery) -> TradeHistoryResponse {
        self.engine.get_trades(symbol, query)
    }

    /// Get order history
    pub fn get_orders(&self, user_address: &str, query: &OrderHistoryQuery) -> OrderHistoryResponse {
        self.engine.get_orders(user_address, query)
    }

    // ========================================================================
    // Database Persistence
    // ========================================================================

    /// Persist a trade to database and update positions
    pub async fn persist_trade(pool: &PgPool, trade: &TradeEvent) -> Result<(), sqlx::Error> {
        // Calculate fees (0.02% maker, 0.05% taker)
        let trade_value = trade.amount * trade.price;
        let maker_fee = trade_value * Decimal::from_str_exact("0.0002").unwrap();
        let taker_fee = trade_value * Decimal::from_str_exact("0.0005").unwrap();

        // 1. Save trade record
        sqlx::query(
            r#"
            INSERT INTO trades (id, symbol, maker_order_id, taker_order_id, maker_address, taker_address, side, price, amount, maker_fee, taker_fee, created_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7::order_side, $8, $9, $10, $11, to_timestamp($12::double precision / 1000))
            ON CONFLICT (id, created_at) DO NOTHING
            "#
        )
        .bind(trade.trade_id)
        .bind(&trade.symbol)
        .bind(trade.maker_order_id)
        .bind(trade.taker_order_id)
        .bind(&trade.maker_address)
        .bind(&trade.taker_address)
        .bind(&trade.side)
        .bind(trade.price)
        .bind(trade.amount)
        .bind(maker_fee)
        .bind(taker_fee)
        .bind(trade.timestamp as f64)
        .execute(pool)
        .await?;

        debug!("Persisted trade: {}", trade.trade_id);

        // 2. Get leverage info from orders
        let maker_leverage: Option<i32> = sqlx::query_scalar(
            "SELECT leverage FROM orders WHERE id = $1"
        )
        .bind(trade.maker_order_id)
        .fetch_optional(pool)
        .await?;

        let taker_leverage: Option<i32> = sqlx::query_scalar(
            "SELECT leverage FROM orders WHERE id = $1"
        )
        .bind(trade.taker_order_id)
        .fetch_optional(pool)
        .await?;

        info!(
            "Trade {}: maker_leverage={:?}, taker_leverage={:?}",
            trade.trade_id, maker_leverage, taker_leverage
        );

        // 3. Update positions for maker and taker
        let position_service = PositionService::new(pool.clone());

        // Maker position (opposite side from trade, because maker provides liquidity on the other side)
        if let Some(leverage) = maker_leverage {
            info!(
                "Updating maker position: address={}, symbol={}, leverage={}",
                trade.maker_address, trade.symbol, leverage
            );
            let maker_side = match trade.side.as_str() {
                "buy" => PositionSide::Short,  // Taker buys, maker sells
                "sell" => PositionSide::Long,   // Taker sells, maker buys
                _ => {
                    warn!("Unknown trade side: {}", trade.side);
                    return Ok(());
                }
            };

            // Calculate collateral (size_in_usd / leverage)
            let size_in_usd = trade.amount * trade.price;
            let collateral_amount = size_in_usd / Decimal::from(leverage);

            if let Err(e) = position_service.increase_position(
                &trade.maker_address,
                &trade.symbol,
                maker_side,
                collateral_amount,
                leverage,
                trade.price,
                true, // Skip min size check - trade already executed
            ).await {
                error!(
                    "Failed to update maker position for {} on {}: {:?}",
                    trade.maker_address, trade.symbol, e
                );
                // Don't fail the trade persistence, just log the error
            } else {
                info!(
                    "✅ Updated maker position for {} on {} (side={:?}, collateral={})",
                    trade.maker_address, trade.symbol, maker_side, collateral_amount
                );
            }
        } else {
            warn!(
                "⚠️ Maker order {} has no leverage, skipping position update",
                trade.maker_order_id
            );
        }

        // Taker position (same side as trade, because taker initiates the trade)
        if let Some(leverage) = taker_leverage {
            info!(
                "Updating taker position: address={}, symbol={}, leverage={}",
                trade.taker_address, trade.symbol, leverage
            );
            let taker_side = match trade.side.as_str() {
                "buy" => PositionSide::Long,  // Taker buys, opens long
                "sell" => PositionSide::Short, // Taker sells, opens short
                _ => {
                    warn!("Unknown trade side: {}", trade.side);
                    return Ok(());
                }
            };

            // Calculate collateral (size_in_usd / leverage)
            let size_in_usd = trade.amount * trade.price;
            let collateral_amount = size_in_usd / Decimal::from(leverage);

            if let Err(e) = position_service.increase_position(
                &trade.taker_address,
                &trade.symbol,
                taker_side,
                collateral_amount,
                leverage,
                trade.price,
                true, // Skip min size check - trade already executed
            ).await {
                error!(
                    "Failed to update taker position for {} on {}: {:?}",
                    trade.taker_address, trade.symbol, e
                );
                // Don't fail the trade persistence, just log the error
            } else {
                info!(
                    "✅ Updated taker position for {} on {} (side={:?}, collateral={})",
                    trade.taker_address, trade.symbol, taker_side, collateral_amount
                );
            }
        } else {
            warn!(
                "⚠️ Taker order {} has no leverage, skipping position update",
                trade.taker_order_id
            );
        }

        // 4. Handle referral commission
        // Check if maker has a referrer
        let maker_referrer: Option<(String, Decimal)> = sqlx::query_as(
            r#"
            SELECT rc.owner_address, rc.commission_rate
            FROM users u
            JOIN referral_codes rc ON u.referrer_address = rc.owner_address
            WHERE u.address = $1
            "#
        )
        .bind(&trade.maker_address.to_lowercase())
        .fetch_optional(pool)
        .await?;

        if let Some((referrer_address, commission_rate)) = maker_referrer {
            let commission = maker_fee * commission_rate;
            sqlx::query(
                r#"
                INSERT INTO referral_earnings 
                (id, referrer_address, referee_address, trade_id, event_type, volume, commission, token, status, created_at)
                VALUES ($1, $2, $3, $4, 'trade', $5, $6, 'USDT', 'pending', to_timestamp($7::double precision / 1000))
                "#
            )
            .bind(uuid::Uuid::new_v4())
            .bind(&referrer_address)
            .bind(&trade.maker_address.to_lowercase())
            .bind(trade.trade_id)
            .bind(trade_value)
            .bind(commission)
            .bind(trade.timestamp as f64)
            .execute(pool)
            .await?;

            debug!("Recorded referral commission {} for maker {} (referrer: {})", commission, trade.maker_address, referrer_address);
        }

        // Check if taker has a referrer
        let taker_referrer: Option<(String, Decimal)> = sqlx::query_as(
            r#"
            SELECT rc.owner_address, rc.commission_rate
            FROM users u
            JOIN referral_codes rc ON u.referrer_address = rc.owner_address
            WHERE u.address = $1
            "#
        )
        .bind(&trade.taker_address.to_lowercase())
        .fetch_optional(pool)
        .await?;

        if let Some((referrer_address, commission_rate)) = taker_referrer {
            let commission = taker_fee * commission_rate;
            sqlx::query(
                r#"
                INSERT INTO referral_earnings 
                (id, referrer_address, referee_address, trade_id, event_type, volume, commission, token, status, created_at)
                VALUES ($1, $2, $3, $4, 'trade', $5, $6, 'USDT', 'pending', to_timestamp($7::double precision / 1000))
                "#
            )
            .bind(uuid::Uuid::new_v4())
            .bind(&referrer_address)
            .bind(&trade.taker_address.to_lowercase())
            .bind(trade.trade_id)
            .bind(trade_value)
            .bind(commission)
            .bind(trade.timestamp as f64)
            .execute(pool)
            .await?;

            debug!("Recorded referral commission {} for taker {} (referrer: {})", commission, trade.taker_address, referrer_address);
        }

        Ok(())
    }

    /// Persist an order to database
    async fn persist_order(
        pool: &PgPool,
        symbol: &str,
        user_address: &str,
        result: &MatchResult,
        side: Side,
        order_type: OrderType,
        amount: Decimal,
        price: Option<Decimal>,
        leverage: u32,
    ) -> Result<(), sqlx::Error> {
        let status = match result.status {
            OrderStatus::Open => "open",
            OrderStatus::PartiallyFilled => "partially_filled",
            OrderStatus::Filled => "filled",
            OrderStatus::Cancelled => "cancelled",
            OrderStatus::Rejected => "rejected",
        };

        let side_str = match side {
            Side::Buy => "buy",
            Side::Sell => "sell",
        };

        let order_type_str = match order_type {
            OrderType::Limit => "limit",
            OrderType::Market => "market",
        };

        sqlx::query(
            r#"
            INSERT INTO orders (id, symbol, user_address, side, order_type, status, price, amount, filled_amount, leverage, created_at)
            VALUES ($1, $2, $3, $4::order_side, $5::order_type, $6::order_status, $7, $8, $9, $10, NOW())
            ON CONFLICT (id) DO UPDATE SET
                status = $6::order_status,
                filled_amount = $9,
                updated_at = NOW()
            "#
        )
        .bind(result.order_id)
        .bind(symbol)
        .bind(user_address)
        .bind(side_str)
        .bind(order_type_str)
        .bind(status)
        .bind(price)
        .bind(amount)
        .bind(result.filled_amount)
        .bind(leverage as i32)
        .execute(pool)
        .await?;

        // Update maker orders if there were trades
        for trade in &result.trades {
            sqlx::query(
                r#"
                UPDATE orders
                SET filled_amount = filled_amount + $1,
                    status = CASE
                        WHEN filled_amount + $1 >= amount THEN 'filled'::order_status
                        ELSE 'partially_filled'::order_status
                    END,
                    updated_at = NOW()
                WHERE id = $2
                "#
            )
            .bind(trade.amount)
            .bind(trade.maker_order_id)
            .execute(pool)
            .await?;
        }

        debug!("Persisted order: {}", result.order_id);
        Ok(())
    }

    /// Update order status
    async fn update_order_status(pool: &PgPool, order_id: Uuid, status: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            UPDATE orders
            SET status = $1::order_status, updated_at = NOW()
            WHERE id = $2
            "#
        )
        .bind(status)
        .bind(order_id)
        .execute(pool)
        .await?;

        debug!("Updated order status: id={}, status={}", order_id, status);
        Ok(())
    }

    /// Batch persist trades
    pub async fn batch_persist_trades(pool: &PgPool, trades: &[TradeEvent]) -> Result<usize, sqlx::Error> {
        if trades.is_empty() {
            return Ok(0);
        }

        let mut tx = pool.begin().await?;
        let mut count = 0;

        for trade in trades {
            // Calculate fees
            let trade_value = trade.amount * trade.price;
            let maker_fee = trade_value * Decimal::from_str_exact("0.0002").unwrap();
            let taker_fee = trade_value * Decimal::from_str_exact("0.0005").unwrap();

            sqlx::query(
                r#"
                INSERT INTO trades (id, symbol, maker_order_id, taker_order_id, maker_address, taker_address, side, price, amount, maker_fee, taker_fee, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7::order_side, $8, $9, $10, $11, to_timestamp($12::double precision / 1000))
                ON CONFLICT (id, created_at) DO NOTHING
                "#
            )
            .bind(trade.trade_id)
            .bind(&trade.symbol)
            .bind(trade.maker_order_id)
            .bind(trade.taker_order_id)
            .bind(&trade.maker_address)
            .bind(&trade.taker_address)
            .bind(&trade.side)
            .bind(trade.price)
            .bind(trade.amount)
            .bind(maker_fee)
            .bind(taker_fee)
            .bind(trade.timestamp as f64)
            .execute(&mut *tx)
            .await?;

            // Handle referral commission for maker
            let maker_referrer: Option<(String, Decimal)> = sqlx::query_as(
                r#"
                SELECT rc.owner_address, rc.commission_rate
                FROM users u
                JOIN referral_codes rc ON u.referrer_address = rc.owner_address
                WHERE u.address = $1
                "#
            )
            .bind(&trade.maker_address.to_lowercase())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((referrer_address, commission_rate)) = maker_referrer {
                let commission = maker_fee * commission_rate;
                sqlx::query(
                    r#"
                    INSERT INTO referral_earnings 
                    (id, referrer_address, referee_address, trade_id, event_type, volume, commission, token, status, created_at)
                    VALUES ($1, $2, $3, $4, 'trade', $5, $6, 'USDT', 'pending', to_timestamp($7::double precision / 1000))
                    "#
                )
                .bind(uuid::Uuid::new_v4())
                .bind(&referrer_address)
                .bind(&trade.maker_address.to_lowercase())
                .bind(trade.trade_id)
                .bind(trade_value)
                .bind(commission)
                .bind(trade.timestamp as f64)
                .execute(&mut *tx)
                .await?;
            }

            // Handle referral commission for taker
            let taker_referrer: Option<(String, Decimal)> = sqlx::query_as(
                r#"
                SELECT rc.owner_address, rc.commission_rate
                FROM users u
                JOIN referral_codes rc ON u.referrer_address = rc.owner_address
                WHERE u.address = $1
                "#
            )
            .bind(&trade.taker_address.to_lowercase())
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((referrer_address, commission_rate)) = taker_referrer {
                let commission = taker_fee * commission_rate;
                sqlx::query(
                    r#"
                    INSERT INTO referral_earnings 
                    (id, referrer_address, referee_address, trade_id, event_type, volume, commission, token, status, created_at)
                    VALUES ($1, $2, $3, $4, 'trade', $5, $6, 'USDT', 'pending', to_timestamp($7::double precision / 1000))
                    "#
                )
                .bind(uuid::Uuid::new_v4())
                .bind(&referrer_address)
                .bind(&trade.taker_address.to_lowercase())
                .bind(trade.trade_id)
                .bind(trade_value)
                .bind(commission)
                .bind(trade.timestamp as f64)
                .execute(&mut *tx)
                .await?;
            }

            count += 1;
        }

        tx.commit().await?;
        info!("Batch persisted {} trades", count);
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    // Integration tests would require a database connection
    // Unit tests are in engine.rs
}
