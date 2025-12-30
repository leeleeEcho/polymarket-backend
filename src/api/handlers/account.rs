//! Account API Handlers for Prediction Markets
//!
//! Provides endpoints for user profile, balances, shares, orders, and trades.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::models::market::ShareType;
use crate::models::{BalanceResponse, UserProfile};
use crate::services::settlement::{SettlementService, SettlementError};
use crate::AppState;

// ============================================================================
// Helper Modules
// ============================================================================

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

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct BalancesResponse {
    pub balances: Vec<BalanceResponse>,
}

/// User's share holdings in prediction markets
#[derive(Debug, Serialize)]
pub struct ShareDetail {
    pub id: Uuid,
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub amount: Decimal,
    pub avg_cost: Decimal,
    pub current_price: Decimal,
    pub unrealized_pnl: Decimal,
    pub market_question: Option<String>,
    pub outcome_name: Option<String>,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SharesResponse {
    pub shares: Vec<ShareDetail>,
    pub total_value: Decimal,
    pub total_cost: Decimal,
    pub total_unrealized_pnl: Decimal,
}

/// Order detail for prediction markets
#[derive(Debug, Serialize)]
pub struct OrderDetail {
    pub id: Uuid,
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub side: String,
    pub order_type: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub filled_amount: Decimal,
    pub status: String,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub created_at: DateTime<Utc>,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct OrdersResponse {
    pub orders: Vec<OrderDetail>,
    pub total: i64,
}

/// Trade record for prediction markets
#[derive(Debug, Serialize)]
pub struct TradeRecord {
    pub id: Uuid,
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub side: String,
    pub price: Decimal,
    pub amount: Decimal,
    pub fee: Decimal,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TradesResponse {
    pub trades: Vec<TradeRecord>,
    pub total: i64,
}

// ============================================================================
// Query Parameters
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct OrdersQuery {
    pub market_id: Option<Uuid>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    pub market_id: Option<Uuid>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SharesQuery {
    pub market_id: Option<Uuid>,
    /// Filter: only show non-zero positions
    pub active_only: Option<bool>,
}

// ============================================================================
// Handlers
// ============================================================================

/// Get user profile
/// GET /account/profile
pub async fn get_profile(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<UserProfile>, (StatusCode, Json<ErrorResponse>)> {
    let user: Option<UserProfile> = sqlx::query_as(
        r#"
        SELECT address, username, avatar_url, created_at, updated_at
        FROM users
        WHERE address = $1
        "#,
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch user profile: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取用户资料失败".to_string(),
                code: "PROFILE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    match user {
        Some(profile) => Ok(Json(profile)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "用户不存在".to_string(),
                code: "USER_NOT_FOUND".to_string(),
            }),
        )),
    }
}

/// Get user balances
/// GET /account/balances
pub async fn get_balances(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<BalancesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let rows: Vec<(String, Decimal, Decimal)> = sqlx::query_as(
        r#"
        SELECT token, available, frozen
        FROM balances
        WHERE user_address = $1
        ORDER BY token
        "#,
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
        .map(|(token, available, frozen)| BalanceResponse {
            token,
            available,
            frozen,
            total: available + frozen,
        })
        .collect();

    Ok(Json(BalancesResponse { balances }))
}

/// Get user orders
/// GET /account/orders
pub async fn get_orders(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<OrdersQuery>,
) -> Result<Json<OrdersResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);

    // Build query with optional filters
    let mut sql = String::from(
        r#"
        SELECT id, market_id, outcome_id, share_type::text, side::text, order_type::text,
               price, amount, filled_amount, status::text, created_at, updated_at
        FROM orders
        WHERE user_address = $1
        "#,
    );

    if query.market_id.is_some() {
        sql.push_str(" AND market_id = $4");
    }
    if query.status.is_some() {
        sql.push_str(" AND status::text = $5");
    }

    sql.push_str(" ORDER BY created_at DESC LIMIT $2 OFFSET $3");

    // Execute query
    let rows: Vec<(
        Uuid,
        Uuid,
        Uuid,
        String,
        String,
        String,
        Decimal,
        Decimal,
        Decimal,
        String,
        DateTime<Utc>,
        DateTime<Utc>,
    )> = if query.market_id.is_some() && query.status.is_some() {
        sqlx::query_as(&sql)
            .bind(&auth_user.address.to_lowercase())
            .bind(limit)
            .bind(offset)
            .bind(query.market_id.unwrap())
            .bind(query.status.as_ref().unwrap())
            .fetch_all(&state.db.pool)
            .await
    } else if query.market_id.is_some() {
        sqlx::query_as(&sql)
            .bind(&auth_user.address.to_lowercase())
            .bind(limit)
            .bind(offset)
            .bind(query.market_id.unwrap())
            .fetch_all(&state.db.pool)
            .await
    } else if query.status.is_some() {
        // Need to adjust SQL for this case
        let sql = r#"
            SELECT id, market_id, outcome_id, share_type::text, side::text, order_type::text,
                   price, amount, filled_amount, status::text, created_at, updated_at
            FROM orders
            WHERE user_address = $1 AND status::text = $4
            ORDER BY created_at DESC LIMIT $2 OFFSET $3
        "#;
        sqlx::query_as(sql)
            .bind(&auth_user.address.to_lowercase())
            .bind(limit)
            .bind(offset)
            .bind(query.status.as_ref().unwrap())
            .fetch_all(&state.db.pool)
            .await
    } else {
        let sql = r#"
            SELECT id, market_id, outcome_id, share_type::text, side::text, order_type::text,
                   price, amount, filled_amount, status::text, created_at, updated_at
            FROM orders
            WHERE user_address = $1
            ORDER BY created_at DESC LIMIT $2 OFFSET $3
        "#;
        sqlx::query_as(sql)
            .bind(&auth_user.address.to_lowercase())
            .bind(limit)
            .bind(offset)
            .fetch_all(&state.db.pool)
            .await
    }
    .map_err(|e| {
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
        .map(
            |(
                id,
                market_id,
                outcome_id,
                share_type,
                side,
                order_type,
                price,
                amount,
                filled_amount,
                status,
                created_at,
                updated_at,
            )| {
                OrderDetail {
                    id,
                    market_id,
                    outcome_id,
                    share_type: share_type.parse().unwrap_or(ShareType::Yes),
                    side,
                    order_type,
                    price,
                    amount,
                    filled_amount,
                    status,
                    created_at,
                    updated_at,
                }
            },
        )
        .collect();

    let total = orders.len() as i64;

    Ok(Json(OrdersResponse { orders, total }))
}

/// Get user trades
/// GET /account/trades
pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<TradesQuery>,
) -> Result<Json<TradesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);
    let user_address = auth_user.address.to_lowercase();

    let rows: Vec<(
        Uuid,
        Uuid,
        Uuid,
        String,
        String,
        Decimal,
        Decimal,
        Decimal,
        DateTime<Utc>,
    )> = if let Some(market_id) = query.market_id {
        sqlx::query_as(
            r#"
            SELECT id, market_id, outcome_id, share_type::text, side::text,
                   price, amount,
                   CASE WHEN maker_address = $1 THEN maker_fee ELSE taker_fee END as fee,
                   created_at
            FROM trades
            WHERE (maker_address = $1 OR taker_address = $1) AND market_id = $4
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(&user_address)
        .bind(limit)
        .bind(offset)
        .bind(market_id)
        .fetch_all(&state.db.pool)
        .await
    } else {
        sqlx::query_as(
            r#"
            SELECT id, market_id, outcome_id, share_type::text, side::text,
                   price, amount,
                   CASE WHEN maker_address = $1 THEN maker_fee ELSE taker_fee END as fee,
                   created_at
            FROM trades
            WHERE maker_address = $1 OR taker_address = $1
            ORDER BY created_at DESC
            LIMIT $2 OFFSET $3
            "#,
        )
        .bind(&user_address)
        .bind(limit)
        .bind(offset)
        .fetch_all(&state.db.pool)
        .await
    }
    .map_err(|e| {
        tracing::error!("Failed to fetch trades: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取交易记录失败".to_string(),
                code: "TRADE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let trades: Vec<TradeRecord> = rows
        .into_iter()
        .map(
            |(id, market_id, outcome_id, share_type, side, price, amount, fee, timestamp)| {
                TradeRecord {
                    id,
                    market_id,
                    outcome_id,
                    share_type: share_type.parse().unwrap_or(ShareType::Yes),
                    side,
                    price,
                    amount,
                    fee,
                    timestamp,
                }
            },
        )
        .collect();

    let total = trades.len() as i64;

    Ok(Json(TradesResponse { trades, total }))
}

/// Get user share holdings
/// GET /account/shares
pub async fn get_shares(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<SharesQuery>,
) -> Result<Json<SharesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let user_address = auth_user.address.to_lowercase();
    let active_only = query.active_only.unwrap_or(true);

    // Build query based on filters
    let rows: Vec<(
        Uuid,      // shares.id
        Uuid,      // shares.market_id
        Uuid,      // shares.outcome_id
        String,    // shares.share_type
        Decimal,   // shares.amount
        Decimal,   // shares.avg_cost
        DateTime<Utc>, // shares.created_at
        DateTime<Utc>, // shares.updated_at
        String,    // markets.question
        String,    // outcomes.name
        Decimal,   // outcomes.probability
    )> = if let Some(market_id) = query.market_id {
        let sql = if active_only {
            r#"
            SELECT s.id, s.market_id, s.outcome_id, s.share_type::text, s.amount, s.avg_cost,
                   s.created_at, s.updated_at,
                   m.question, o.name, o.probability
            FROM shares s
            JOIN markets m ON s.market_id = m.id
            JOIN outcomes o ON s.outcome_id = o.id
            WHERE s.user_address = $1 AND s.market_id = $2 AND s.amount > 0
            ORDER BY s.updated_at DESC
            "#
        } else {
            r#"
            SELECT s.id, s.market_id, s.outcome_id, s.share_type::text, s.amount, s.avg_cost,
                   s.created_at, s.updated_at,
                   m.question, o.name, o.probability
            FROM shares s
            JOIN markets m ON s.market_id = m.id
            JOIN outcomes o ON s.outcome_id = o.id
            WHERE s.user_address = $1 AND s.market_id = $2
            ORDER BY s.updated_at DESC
            "#
        };
        sqlx::query_as(sql)
            .bind(&user_address)
            .bind(market_id)
            .fetch_all(&state.db.pool)
            .await
    } else {
        let sql = if active_only {
            r#"
            SELECT s.id, s.market_id, s.outcome_id, s.share_type::text, s.amount, s.avg_cost,
                   s.created_at, s.updated_at,
                   m.question, o.name, o.probability
            FROM shares s
            JOIN markets m ON s.market_id = m.id
            JOIN outcomes o ON s.outcome_id = o.id
            WHERE s.user_address = $1 AND s.amount > 0
            ORDER BY s.updated_at DESC
            "#
        } else {
            r#"
            SELECT s.id, s.market_id, s.outcome_id, s.share_type::text, s.amount, s.avg_cost,
                   s.created_at, s.updated_at,
                   m.question, o.name, o.probability
            FROM shares s
            JOIN markets m ON s.market_id = m.id
            JOIN outcomes o ON s.outcome_id = o.id
            WHERE s.user_address = $1
            ORDER BY s.updated_at DESC
            "#
        };
        sqlx::query_as(sql)
            .bind(&user_address)
            .fetch_all(&state.db.pool)
            .await
    }
    .map_err(|e| {
        tracing::error!("Failed to fetch shares: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "获取持仓失败".to_string(),
                code: "SHARES_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let mut total_value = Decimal::ZERO;
    let mut total_cost = Decimal::ZERO;

    let shares: Vec<ShareDetail> = rows
        .into_iter()
        .map(
            |(
                id,
                market_id,
                outcome_id,
                share_type,
                amount,
                avg_cost,
                created_at,
                updated_at,
                question,
                outcome_name,
                probability,
            )| {
                // Current price is the probability for Yes shares, or 1-probability for No shares
                let share_type_parsed = share_type.parse().unwrap_or(ShareType::Yes);
                let current_price = match share_type_parsed {
                    ShareType::Yes => probability,
                    ShareType::No => Decimal::ONE - probability,
                };

                // Calculate value and PnL
                let position_value = amount * current_price;
                let position_cost = amount * avg_cost;
                let unrealized_pnl = position_value - position_cost;

                total_value += position_value;
                total_cost += position_cost;

                ShareDetail {
                    id,
                    market_id,
                    outcome_id,
                    share_type: share_type_parsed,
                    amount,
                    avg_cost,
                    current_price,
                    unrealized_pnl,
                    market_question: Some(question),
                    outcome_name: Some(outcome_name),
                    created_at,
                    updated_at,
                }
            },
        )
        .collect();

    let total_unrealized_pnl = total_value - total_cost;

    Ok(Json(SharesResponse {
        shares,
        total_value,
        total_cost,
        total_unrealized_pnl,
    }))
}

// ============================================================================
// Settlement Types
// ============================================================================

/// Settlement result response
#[derive(Debug, Serialize)]
pub struct SettlementResponse {
    pub market_id: Uuid,
    pub settlement_type: String,
    pub shares_settled: Vec<ShareSettlementDetail>,
    pub total_payout: Decimal,
    pub message: String,
}

/// Individual share settlement detail
#[derive(Debug, Serialize)]
pub struct ShareSettlementDetail {
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub amount: Decimal,
    pub payout_per_share: Decimal,
    pub total_payout: Decimal,
}

/// Settlement status response
#[derive(Debug, Serialize)]
pub struct SettlementStatusResponse {
    pub market_id: Uuid,
    pub market_status: String,
    pub is_settled: bool,
    pub can_settle: bool,
    pub potential_payout: Decimal,
    pub share_count: usize,
}

// ============================================================================
// Settlement Handlers
// ============================================================================

/// Settle user's shares for a resolved or cancelled market
/// POST /account/settle/:market_id
pub async fn settle_market(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    axum::extract::Path(market_id): axum::extract::Path<Uuid>,
) -> Result<Json<SettlementResponse>, (StatusCode, Json<ErrorResponse>)> {
    let user_address = auth_user.address.to_lowercase();

    let result = SettlementService::settle_user_shares(&state.db.pool, market_id, &user_address)
        .await
        .map_err(|e| {
            let (status, code, message) = match &e {
                SettlementError::MarketNotFound(_) => (
                    StatusCode::NOT_FOUND,
                    "MARKET_NOT_FOUND",
                    "市场不存在",
                ),
                SettlementError::MarketNotSettleable(_) => (
                    StatusCode::BAD_REQUEST,
                    "MARKET_NOT_SETTLEABLE",
                    "市场尚未结算或取消",
                ),
                SettlementError::NoWinningOutcome(_) => (
                    StatusCode::BAD_REQUEST,
                    "NO_WINNING_OUTCOME",
                    "市场未设置获胜结果",
                ),
                SettlementError::NoSharesToSettle(_) => (
                    StatusCode::BAD_REQUEST,
                    "NO_SHARES",
                    "没有可结算的份额",
                ),
                SettlementError::AlreadySettled(_) => (
                    StatusCode::BAD_REQUEST,
                    "ALREADY_SETTLED",
                    "已经结算过",
                ),
                SettlementError::DatabaseError(e) => {
                    tracing::error!("Settlement database error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DATABASE_ERROR",
                        "数据库错误",
                    )
                }
            };

            (
                status,
                Json(ErrorResponse {
                    error: message.to_string(),
                    code: code.to_string(),
                }),
            )
        })?;

    let settlement_type_str = match result.settlement_type {
        crate::services::settlement::SettlementType::Resolution => "resolution",
        crate::services::settlement::SettlementType::Cancellation => "cancellation",
    };

    let shares_settled: Vec<ShareSettlementDetail> = result
        .shares_settled
        .into_iter()
        .map(|s| ShareSettlementDetail {
            outcome_id: s.outcome_id,
            share_type: s.share_type,
            amount: s.amount,
            payout_per_share: s.payout_per_share,
            total_payout: s.total_payout,
        })
        .collect();

    let message = format!(
        "成功结算 {} USDC",
        result.total_payout
    );

    Ok(Json(SettlementResponse {
        market_id: result.market_id,
        settlement_type: settlement_type_str.to_string(),
        shares_settled,
        total_payout: result.total_payout,
        message,
    }))
}

/// Get settlement status for a market
/// GET /account/settle/:market_id/status
pub async fn get_settlement_status(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    axum::extract::Path(market_id): axum::extract::Path<Uuid>,
) -> Result<Json<SettlementStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let user_address = auth_user.address.to_lowercase();

    let status = SettlementService::get_settlement_status(&state.db.pool, market_id, &user_address)
        .await
        .map_err(|e| {
            tracing::error!("Failed to get settlement status: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "获取结算状态失败".to_string(),
                    code: "SETTLEMENT_STATUS_FAILED".to_string(),
                }),
            )
        })?;

    match status {
        Some(s) => Ok(Json(SettlementStatusResponse {
            market_id: s.market_id,
            market_status: s.market_status,
            is_settled: s.is_settled,
            can_settle: s.can_settle,
            potential_payout: s.potential_payout,
            share_count: s.share_count,
        })),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "没有找到相关持仓".to_string(),
                code: "NO_SHARES_FOUND".to_string(),
            }),
        )),
    }
}
