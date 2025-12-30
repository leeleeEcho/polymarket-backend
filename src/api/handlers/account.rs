//! Account API Handlers
//!
//! Phase 9: Complete account data layer with real database queries

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Extension, Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, Serializer};
use std::sync::Arc;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::models::{BalanceResponse, UserProfile};
use crate::AppState;

// Helper module to serialize DateTime as milliseconds timestamp
mod datetime_as_millis {
    use chrono::{DateTime, Utc};
    use serde::Serializer;

    pub fn serialize<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(dt.timestamp_millis())
    }
}

#[derive(Debug, Serialize)]
pub struct BalancesResponse {
    pub balances: Vec<BalanceResponse>,
}

/// Simplified position response for API
#[derive(Debug, Serialize)]
pub struct PositionDetail {
    pub id: Uuid,
    pub symbol: String,
    pub side: String,
    pub size: Decimal,
    pub entry_price: Decimal,
    pub mark_price: Decimal,
    pub liquidation_price: Decimal,
    pub collateral_amount: Decimal,
    pub leverage: i32,
    pub unrealized_pnl: Decimal,
    pub realized_pnl: Decimal,
    pub margin_ratio: Decimal,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PositionsResponse {
    pub positions: Vec<PositionDetail>,
    pub total_unrealized_pnl: Decimal,
    pub total_collateral: Decimal,
}

#[derive(Debug, Serialize)]
pub struct OrdersResponse {
    pub orders: Vec<OrderDetail>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct TradesResponse {
    pub trades: Vec<TradeRecord>,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct OrderDetail {
    pub id: Uuid,
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    pub price: Decimal,  // Changed from Option<Decimal> to Decimal - price must never be null
    pub amount: Decimal,
    pub filled_amount: Decimal,
    pub leverage: i32,
    pub status: String,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TradeRecord {
    pub id: Uuid,
    pub order_id: Uuid,
    pub symbol: String,
    pub side: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub fee: Decimal,
    pub realized_pnl: Decimal,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    pub symbol: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    pub symbol: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

/// Get user profile
/// GET /account/profile
pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<UserProfile>, (StatusCode, Json<ErrorResponse>)> {
    // Try to fetch from database using tuple query
    let user: Option<(String, Option<String>, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT
            address,
            referral_code,
            referrer_address,
            created_at
        FROM users
        WHERE address = $1
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch user profile: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取用户信息失败".to_string(),
                code: "PROFILE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    // If user doesn't exist, create a new one
    if let Some((address, referral_code, referrer_address, created_at)) = user {
        Ok(Json(UserProfile {
            address,
            referral_code,
            referrer_address,
            created_at,
        }))
    } else {
        // Auto-create user record
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO users (address, created_at) VALUES ($1, $2) ON CONFLICT (address) DO NOTHING"
        )
        .bind(&auth_user.address.to_lowercase())
        .bind(now)
        .execute(&state.db.pool)
        .await
        .ok();

        Ok(Json(UserProfile {
            address: auth_user.address.to_lowercase(),
            referral_code: None,
            referrer_address: None,
            created_at: now,
        }))
    }
}

/// Get user balances
/// GET /account/balances
pub async fn get_balances(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<BalancesResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Fetch balances from database
    let rows: Vec<(String, Decimal, Decimal)> = sqlx::query_as(
        "SELECT token, available, frozen FROM balances WHERE user_address = $1"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch balances: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取余额失败".to_string(),
                code: "BALANCE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let balances: Vec<BalanceResponse> = rows
        .into_iter()
        .map(|(token, available, frozen)| {
            BalanceResponse {
                token,
                available,
                frozen,
                total: available + frozen,
            }
        })
        .collect();

    Ok(Json(BalancesResponse { balances }))
}

/// Get user positions
/// GET /account/positions
pub async fn get_positions(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<PositionsResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Fetch open positions from database - use size_in_usd and collateral_amount for GMX-style schema
    let rows: Vec<(Uuid, String, String, Decimal, Decimal, Decimal, i32, Decimal, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT
            id, symbol, side::text, size_in_usd, entry_price,
            collateral_amount, leverage, realized_pnl,
            created_at, updated_at
        FROM positions
        WHERE user_address = $1 AND status = 'open'
        ORDER BY created_at DESC
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch positions: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取仓位失败".to_string(),
                code: "POSITION_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let mut positions = Vec::new();
    let mut total_unrealized_pnl = Decimal::ZERO;
    let mut total_collateral = Decimal::ZERO;

    for (id, symbol, side, size, entry_price, collateral, leverage, realized_pnl, created_at, updated_at) in rows {
        // Get current mark price
        let mark_price = state.price_feed_service
            .get_mark_price(&symbol)
            .await
            .unwrap_or(entry_price);

        // Calculate unrealized PnL
        let is_long = side.to_lowercase() == "long";
        let size_in_tokens = if entry_price > Decimal::ZERO {
            size / entry_price
        } else {
            Decimal::ZERO
        };

        let unrealized_pnl = if is_long {
            (mark_price - entry_price) * size_in_tokens
        } else {
            (entry_price - mark_price) * size_in_tokens
        };

        // Calculate liquidation price
        let position_value = size;
        let maintenance_margin = position_value * Decimal::new(5, 3); // 0.5%
        let liq_distance = if size_in_tokens > Decimal::ZERO {
            (collateral - maintenance_margin) / size_in_tokens
        } else {
            Decimal::ZERO
        };

        let liquidation_price = if is_long {
            entry_price - liq_distance
        } else {
            entry_price + liq_distance
        };

        // Calculate margin ratio (collateral / position_value)
        let margin_ratio = if position_value > Decimal::ZERO {
            collateral / position_value
        } else {
            Decimal::ZERO
        };

        // Accumulate totals
        total_unrealized_pnl += unrealized_pnl;
        total_collateral += collateral;

        positions.push(PositionDetail {
            id,
            symbol,
            side,
            size,
            entry_price,
            mark_price,
            liquidation_price: liquidation_price.max(Decimal::ZERO),
            collateral_amount: collateral,
            leverage,
            unrealized_pnl,
            realized_pnl,
            margin_ratio,
            created_at,
            updated_at,
        });
    }

    Ok(Json(PositionsResponse { 
        positions,
        total_unrealized_pnl,
        total_collateral,
    }))
}

/// Get user orders with filtering
/// GET /account/orders
pub async fn get_orders(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<OrdersQuery>,
) -> Result<Json<OrdersResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    let user_address = auth_user.address.to_lowercase();

    // Build dynamic query based on filters
    // Cast ENUM types to TEXT to avoid sqlx decode errors
    // Use COALESCE to ensure price is never null (use 0 as fallback for market orders)
    let mut sql = String::from(
        r#"
        SELECT
            id, symbol, side::TEXT, order_type::TEXT, 
            COALESCE(price, 0) as price, amount,
            filled_amount, leverage, status::TEXT, created_at, updated_at
        FROM orders
        WHERE user_address = $1
        "#
    );
    let mut count_sql = String::from("SELECT COUNT(*) FROM orders WHERE user_address = $1");

    let mut param_idx = 2;
    let mut conditions = Vec::new();

    if query.symbol.is_some() {
        conditions.push(format!("AND symbol = ${}", param_idx));
        param_idx += 1;
    }

    if query.status.is_some() {
        // 需要显式转换为 order_status ENUM 类型
        conditions.push(format!("AND status = ${}::order_status", param_idx));
        param_idx += 1;
    }

    for cond in &conditions {
        sql.push(' ');
        sql.push_str(cond);
        count_sql.push(' ');
        count_sql.push_str(cond);
    }

    sql.push_str(" ORDER BY created_at DESC LIMIT $");
    sql.push_str(&param_idx.to_string());
    sql.push_str(" OFFSET $");
    sql.push_str(&(param_idx + 1).to_string());

    // Execute count query
    let total = match (&query.symbol, &query.status) {
        (Some(symbol), Some(status)) => {
            sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(&user_address)
                .bind(symbol)
                .bind(status)
                .fetch_one(&state.db.pool)
                .await
                .unwrap_or(0)
        }
        (Some(symbol), None) => {
            sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(&user_address)
                .bind(symbol)
                .fetch_one(&state.db.pool)
                .await
                .unwrap_or(0)
        }
        (None, Some(status)) => {
            sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(&user_address)
                .bind(status)
                .fetch_one(&state.db.pool)
                .await
                .unwrap_or(0)
        }
        (None, None) => {
            sqlx::query_scalar::<_, i64>(&count_sql)
                .bind(&user_address)
                .fetch_one(&state.db.pool)
                .await
                .unwrap_or(0)
        }
    };

    // Execute main query
    let rows: Vec<(Uuid, String, String, String, Decimal, Decimal, Decimal, i32, String, DateTime<Utc>, DateTime<Utc>)> = match (&query.symbol, &query.status) {
        (Some(symbol), Some(status)) => {
            sqlx::query_as(&sql)
                .bind(&user_address)
                .bind(symbol)
                .bind(status)
                .bind(limit)
                .bind(offset)
                .fetch_all(&state.db.pool)
                .await
        }
        (Some(symbol), None) => {
            sqlx::query_as(&sql)
                .bind(&user_address)
                .bind(symbol)
                .bind(limit)
                .bind(offset)
                .fetch_all(&state.db.pool)
                .await
        }
        (None, Some(status)) => {
            sqlx::query_as(&sql)
                .bind(&user_address)
                .bind(status)
                .bind(limit)
                .bind(offset)
                .fetch_all(&state.db.pool)
                .await
        }
        (None, None) => {
            sqlx::query_as(&sql)
                .bind(&user_address)
                .bind(limit)
                .bind(offset)
                .fetch_all(&state.db.pool)
                .await
        }
    }.map_err(|e| {
        tracing::error!("Failed to fetch orders: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取订单失败".to_string(),
                code: "ORDER_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let orders: Vec<OrderDetail> = rows
        .into_iter()
        .map(|(id, symbol, side, order_type, price, amount, filled_amount, leverage, status, created_at, updated_at)| {
            OrderDetail {
                id,
                symbol,
                side,
                order_type,
                price,
                amount,
                filled_amount,
                leverage,
                status,
                created_at,
                updated_at,
            }
        })
        .collect();

    Ok(Json(OrdersResponse { orders, total }))
}

/// Get user trades history
/// GET /account/trades
pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<TradesQuery>,
) -> Result<Json<TradesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    let user_address = auth_user.address.to_lowercase();

    // Build query based on filters
    // Note: trades table has maker_order_id/taker_order_id, maker_fee/taker_fee
    // 需要将 ENUM 类型转换为 TEXT 进行比较和返回
    let (sql, count_sql) = if query.symbol.is_some() {
        (
            r#"
            SELECT
                t.id,
                CASE WHEN t.maker_address = $1 THEN t.maker_order_id ELSE t.taker_order_id END as order_id,
                t.symbol,
                CASE WHEN t.maker_address = $1 THEN t.side::TEXT ELSE
                    CASE WHEN t.side::TEXT = 'buy' THEN 'sell' ELSE 'buy' END
                END as side,
                t.price,
                t.amount,
                CASE WHEN t.maker_address = $1 THEN t.maker_fee ELSE t.taker_fee END as fee,
                COALESCE(0::DECIMAL, 0::DECIMAL) as realized_pnl,
                t.created_at
            FROM trades t
            WHERE (t.maker_address = $1 OR t.taker_address = $1) AND t.symbol = $2
            ORDER BY t.created_at DESC
            LIMIT $3 OFFSET $4
            "#,
            "SELECT COUNT(*) FROM trades WHERE (maker_address = $1 OR taker_address = $1) AND symbol = $2"
        )
    } else {
        (
            r#"
            SELECT
                t.id,
                CASE WHEN t.maker_address = $1 THEN t.maker_order_id ELSE t.taker_order_id END as order_id,
                t.symbol,
                CASE WHEN t.maker_address = $1 THEN t.side::TEXT ELSE
                    CASE WHEN t.side::TEXT = 'buy' THEN 'sell' ELSE 'buy' END
                END as side,
                t.price,
                t.amount,
                CASE WHEN t.maker_address = $1 THEN t.maker_fee ELSE t.taker_fee END as fee,
                COALESCE(0::DECIMAL, 0::DECIMAL) as realized_pnl,
                t.created_at
            FROM trades t
            WHERE t.maker_address = $1 OR t.taker_address = $1
            ORDER BY t.created_at DESC
            LIMIT $2 OFFSET $3
            "#,
            "SELECT COUNT(*) FROM trades WHERE maker_address = $1 OR taker_address = $1"
        )
    };

    // Get total count
    let total = if let Some(ref symbol) = query.symbol {
        sqlx::query_scalar::<_, i64>(count_sql)
            .bind(&user_address)
            .bind(symbol)
            .fetch_one(&state.db.pool)
            .await
            .unwrap_or(0)
    } else {
        sqlx::query_scalar::<_, i64>(count_sql)
            .bind(&user_address)
            .fetch_one(&state.db.pool)
            .await
            .unwrap_or(0)
    };

    // Fetch trades
    let rows: Vec<(Uuid, Uuid, String, String, Decimal, Decimal, Decimal, Decimal, DateTime<Utc>)> = if let Some(ref symbol) = query.symbol {
        sqlx::query_as(sql)
            .bind(&user_address)
            .bind(symbol)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
    } else {
        sqlx::query_as(sql)
            .bind(&user_address)
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
    }.map_err(|e| {
        tracing::error!("Failed to fetch trades: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取交易历史失败".to_string(),
                code: "TRADE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let trades: Vec<TradeRecord> = rows
        .into_iter()
        .map(|(id, order_id, symbol, side, price, amount, fee, realized_pnl, timestamp)| {
            TradeRecord {
                id,
                order_id,
                symbol,
                side,
                price,
                amount,
                fee,
                realized_pnl,
                timestamp,
            }
        })
        .collect();

    Ok(Json(TradesResponse { trades, total }))
}
