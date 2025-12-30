//! Market API Handlers for Prediction Markets
//!
//! Provides endpoints for listing markets, getting orderbooks, trades, and prices.

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::models::market::ShareType;
use crate::AppState;

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

/// Outcome information for a prediction market
#[derive(Debug, Serialize)]
pub struct OutcomeInfo {
    pub id: Uuid,
    pub name: String,
    pub probability: Decimal,
}

/// Prediction market information
#[derive(Debug, Serialize)]
pub struct MarketInfo {
    pub id: Uuid,
    pub question: String,
    pub description: Option<String>,
    pub category: String,
    pub outcomes: Vec<OutcomeInfo>,
    pub status: String,
    pub resolution_source: Option<String>,
    pub end_time: Option<i64>,
    pub volume_24h: Decimal,
    pub total_volume: Decimal,
    pub liquidity: Decimal,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct MarketsResponse {
    pub markets: Vec<MarketInfo>,
    pub total: i64,
}

#[derive(Debug, Deserialize)]
pub struct MarketsQuery {
    pub category: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Orderbook level
#[derive(Debug, Serialize)]
pub struct OrderbookLevel {
    pub price: String,
    pub amount: String,
}

/// Orderbook response for a market outcome
#[derive(Debug, Serialize)]
pub struct OrderbookResponse {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub share_type: ShareType,
    pub bids: Vec<OrderbookLevel>,
    pub asks: Vec<OrderbookLevel>,
    pub timestamp: i64,
}

/// Trade record
#[derive(Debug, Serialize)]
pub struct TradeInfo {
    pub id: Uuid,
    pub price: Decimal,
    pub amount: Decimal,
    pub side: String,
    pub share_type: ShareType,
    pub timestamp: i64,
}

#[derive(Debug, Serialize)]
pub struct TradesResponse {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub trades: Vec<TradeInfo>,
}

/// Price/ticker information for a market
#[derive(Debug, Serialize)]
pub struct TickerResponse {
    pub market_id: Uuid,
    pub outcomes: Vec<OutcomeTicker>,
    pub volume_24h: Decimal,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
pub struct OutcomeTicker {
    pub outcome_id: Uuid,
    pub name: String,
    pub yes_price: Decimal,
    pub no_price: Decimal,
    pub probability: Decimal,
}

#[derive(Debug, Deserialize)]
pub struct OrderbookQuery {
    pub outcome_id: Uuid,
    pub share_type: Option<String>,
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct TradesQuery {
    pub outcome_id: Uuid,
    pub limit: Option<i64>,
}

// ============================================================================
// Handlers
// ============================================================================

/// List all available prediction markets
/// GET /markets
pub async fn list_markets(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MarketsQuery>,
) -> Result<Json<MarketsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(100);
    let offset = query.offset.unwrap_or(0);

    // Query markets from database
    let markets_data: Vec<(
        Uuid,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<DateTime<Utc>>,
        Decimal,
        Decimal,
        DateTime<Utc>,
    )> = sqlx::query_as(
        r#"
        SELECT m.id, m.question, m.description, m.category, m.status::text,
               m.resolution_source, m.end_time, m.volume_24h, m.total_volume, m.created_at
        FROM markets m
        WHERE ($1::text IS NULL OR m.category = $1)
        AND ($2::text IS NULL OR m.status::text = $2)
        ORDER BY m.volume_24h DESC
        LIMIT $3 OFFSET $4
        "#,
    )
    .bind(query.category.as_ref())
    .bind(query.status.as_ref())
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch markets: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to fetch markets".to_string(),
                code: "MARKET_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    // Get total count
    let total: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*) FROM markets
        WHERE ($1::text IS NULL OR category = $1)
        AND ($2::text IS NULL OR status::text = $2)
        "#,
    )
    .bind(query.category.as_ref())
    .bind(query.status.as_ref())
    .fetch_one(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to count markets: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to count markets".to_string(),
                code: "MARKET_COUNT_FAILED".to_string(),
            }),
        )
    })?;

    let mut markets = Vec::new();

    for (id, question, description, category, status, resolution_source, end_time, volume_24h, total_volume, created_at) in markets_data {
        // Get outcomes for each market
        let outcomes_data: Vec<(Uuid, String, Decimal)> = sqlx::query_as(
            r#"
            SELECT id, name, probability
            FROM outcomes
            WHERE market_id = $1
            ORDER BY name
            "#,
        )
        .bind(id)
        .fetch_all(&state.db.pool)
        .await
        .unwrap_or_default();

        let outcomes: Vec<OutcomeInfo> = outcomes_data
            .into_iter()
            .map(|(oid, name, probability)| OutcomeInfo {
                id: oid,
                name,
                probability,
            })
            .collect();

        // Calculate liquidity (sum of orderbook depth)
        let liquidity = Decimal::ZERO; // TODO: Calculate from orderbook

        markets.push(MarketInfo {
            id,
            question,
            description,
            category,
            outcomes,
            status,
            resolution_source,
            end_time: end_time.map(|t| t.timestamp_millis()),
            volume_24h,
            total_volume,
            liquidity,
            created_at: created_at.timestamp_millis(),
        });
    }

    Ok(Json(MarketsResponse {
        markets,
        total: total.0,
    }))
}

/// Get orderbook for a market outcome
/// GET /markets/:market_id/orderbook
pub async fn get_orderbook(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Query(query): Query<OrderbookQuery>,
) -> Result<Json<OrderbookResponse>, (StatusCode, Json<ErrorResponse>)> {
    let depth = query.depth.unwrap_or(20).min(100);
    let share_type: ShareType = query
        .share_type
        .as_ref()
        .and_then(|s| s.parse().ok())
        .unwrap_or(ShareType::Yes);

    // Validate market exists
    let market_exists: Option<(Uuid,)> = sqlx::query_as("SELECT id FROM markets WHERE id = $1")
        .bind(market_id)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to check market: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to check market".to_string(),
                    code: "MARKET_CHECK_FAILED".to_string(),
                }),
            )
        })?;

    if market_exists.is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market not found".to_string(),
                code: "MARKET_NOT_FOUND".to_string(),
            }),
        ));
    }

    // Build orderbook key for matching engine
    let orderbook_key = format!("{}:{}:{}", market_id, query.outcome_id, share_type);

    // Try to get orderbook from matching engine
    match state.matching_engine.get_orderbook(&orderbook_key, depth) {
        Ok(snapshot) => {
            let bids: Vec<OrderbookLevel> = snapshot
                .bids
                .into_iter()
                .map(|[price, amount]| OrderbookLevel { price, amount })
                .collect();
            let asks: Vec<OrderbookLevel> = snapshot
                .asks
                .into_iter()
                .map(|[price, amount]| OrderbookLevel { price, amount })
                .collect();

            Ok(Json(OrderbookResponse {
                market_id,
                outcome_id: query.outcome_id,
                share_type,
                bids,
                asks,
                timestamp: snapshot.timestamp,
            }))
        }
        Err(_) => {
            // Return empty orderbook
            Ok(Json(OrderbookResponse {
                market_id,
                outcome_id: query.outcome_id,
                share_type,
                bids: vec![],
                asks: vec![],
                timestamp: chrono::Utc::now().timestamp_millis(),
            }))
        }
    }
}

/// Get recent trades for a market outcome
/// GET /markets/:market_id/trades
pub async fn get_trades(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Query(query): Query<TradesQuery>,
) -> Result<Json<TradesResponse>, (StatusCode, Json<ErrorResponse>)> {
    let limit = query.limit.unwrap_or(50).min(100);

    let rows: Vec<(Uuid, Decimal, Decimal, String, String, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT id, price, amount, side::text, share_type::text, created_at
        FROM trades
        WHERE market_id = $1 AND outcome_id = $2
        ORDER BY created_at DESC
        LIMIT $3
        "#,
    )
    .bind(market_id)
    .bind(query.outcome_id)
    .bind(limit)
    .fetch_all(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch trades: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to fetch trades".to_string(),
                code: "TRADES_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let trades: Vec<TradeInfo> = rows
        .into_iter()
        .map(|(id, price, amount, side, share_type, created_at)| TradeInfo {
            id,
            price,
            amount,
            side,
            share_type: share_type.parse().unwrap_or(ShareType::Yes),
            timestamp: created_at.timestamp_millis(),
        })
        .collect();

    Ok(Json(TradesResponse {
        market_id,
        outcome_id: query.outcome_id,
        trades,
    }))
}

/// Get ticker/price info for a market
/// GET /markets/:market_id/ticker
pub async fn get_ticker(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
) -> Result<Json<TickerResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Get market info
    let market_data: Option<(Uuid, Decimal)> = sqlx::query_as(
        "SELECT id, volume_24h FROM markets WHERE id = $1",
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to fetch market".to_string(),
                code: "MARKET_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let (_, volume_24h) = market_data.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market not found".to_string(),
                code: "MARKET_NOT_FOUND".to_string(),
            }),
        )
    })?;

    // Get outcomes with probabilities
    let outcomes_data: Vec<(Uuid, String, Decimal)> = sqlx::query_as(
        r#"
        SELECT id, name, probability
        FROM outcomes
        WHERE market_id = $1
        ORDER BY name
        "#,
    )
    .bind(market_id)
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default();

    let outcomes: Vec<OutcomeTicker> = outcomes_data
        .into_iter()
        .map(|(outcome_id, name, probability)| {
            // In prediction markets, Yes price = probability, No price = 1 - probability
            let yes_price = probability;
            let no_price = Decimal::ONE - probability;
            OutcomeTicker {
                outcome_id,
                name,
                yes_price,
                no_price,
                probability,
            }
        })
        .collect();

    Ok(Json(TickerResponse {
        market_id,
        outcomes,
        volume_24h,
        updated_at: chrono::Utc::now().timestamp_millis(),
    }))
}

/// Get price for a specific outcome
/// GET /markets/:market_id/price
pub async fn get_price(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
) -> Result<Json<TickerResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Same as ticker for now
    get_ticker(State(state), Path(market_id)).await
}

// ============================================================================
// Admin Handlers for Market Management
// ============================================================================

/// Create market request
#[derive(Debug, Deserialize)]
pub struct CreateMarketRequest {
    /// Gnosis Conditional Tokens conditionId
    pub condition_id: String,
    /// Market question
    pub question: String,
    /// Market description
    pub description: Option<String>,
    /// Market category
    pub category: Option<String>,
    /// Resolution source (UMA, Chainlink, Manual)
    pub resolution_source: Option<String>,
    /// End time (timestamp in milliseconds)
    pub end_time: Option<i64>,
    /// Yes outcome token ID
    pub yes_token_id: String,
    /// No outcome token ID
    pub no_token_id: String,
}

/// Create market response
#[derive(Debug, Serialize)]
pub struct CreateMarketResponse {
    pub market_id: Uuid,
    pub yes_outcome_id: Uuid,
    pub no_outcome_id: Uuid,
    pub message: String,
}

/// Close market request (stops trading)
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CloseMarketRequest {
    pub reason: Option<String>,
}

/// Resolve market request
#[derive(Debug, Deserialize)]
pub struct ResolveMarketRequest {
    /// Which outcome won: "yes" or "no"
    pub winning_outcome: String,
}

/// Market status response
#[derive(Debug, Serialize)]
pub struct MarketStatusResponse {
    pub market_id: Uuid,
    pub status: String,
    pub message: String,
}

/// Get single market details
/// GET /markets/:market_id
pub async fn get_market(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
) -> Result<Json<MarketInfo>, (StatusCode, Json<ErrorResponse>)> {
    use crate::cache::{CachedMarket, CachedOutcome};

    // Try cache first
    if let Some(market_cache) = state.cache.market_opt() {
        if let Ok(Some(cached)) = market_cache.get_market(market_id).await {
            tracing::debug!("Cache hit for market {}", market_id);
            return Ok(Json(MarketInfo {
                id: cached.id,
                question: cached.question,
                description: cached.description,
                category: cached.category.unwrap_or_default(),
                outcomes: cached
                    .outcomes
                    .into_iter()
                    .map(|o| OutcomeInfo {
                        id: o.id,
                        name: o.name,
                        probability: o.probability,
                    })
                    .collect(),
                status: cached.status,
                resolution_source: cached.resolution_source,
                end_time: cached.end_time,
                volume_24h: cached.volume_24h,
                total_volume: cached.total_volume,
                liquidity: Decimal::ZERO,
                created_at: cached.created_at,
            }));
        }
    }

    // Query market from database
    let market_data: Option<(
        Uuid,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<DateTime<Utc>>,
        Decimal,
        Decimal,
        DateTime<Utc>,
    )> = sqlx::query_as(
        r#"
        SELECT id, question, description, category, status::text,
               resolution_source, end_time, volume_24h, total_volume, created_at
        FROM markets
        WHERE id = $1
        "#,
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to fetch market".to_string(),
                code: "MARKET_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let (id, question, description, category, status, resolution_source, end_time, volume_24h, total_volume, created_at) =
        market_data.ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Market not found".to_string(),
                    code: "MARKET_NOT_FOUND".to_string(),
                }),
            )
        })?;

    // Get outcomes for the market
    let outcomes_data: Vec<(Uuid, String, Decimal)> = sqlx::query_as(
        r#"
        SELECT id, name, probability
        FROM outcomes
        WHERE market_id = $1
        ORDER BY name
        "#,
    )
    .bind(id)
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default();

    let outcomes: Vec<OutcomeInfo> = outcomes_data
        .iter()
        .map(|(oid, name, probability)| OutcomeInfo {
            id: *oid,
            name: name.clone(),
            probability: *probability,
        })
        .collect();

    let liquidity = Decimal::ZERO; // TODO: Calculate from orderbook

    // Cache the result
    if let Some(market_cache) = state.cache.market_opt() {
        let cached_market = CachedMarket {
            id,
            question: question.clone(),
            description: description.clone(),
            category: Some(category.clone()),
            status: status.clone(),
            resolution_source: resolution_source.clone(),
            end_time: end_time.map(|t| t.timestamp_millis()),
            outcomes: outcomes_data
                .into_iter()
                .map(|(oid, name, probability)| CachedOutcome {
                    id: oid,
                    name,
                    probability,
                })
                .collect(),
            volume_24h,
            total_volume,
            created_at: created_at.timestamp_millis(),
            updated_at: chrono::Utc::now().timestamp_millis(),
        };
        if let Err(e) = market_cache.set_market(&cached_market).await {
            tracing::warn!("Failed to cache market {}: {}", market_id, e);
        }
    }

    Ok(Json(MarketInfo {
        id,
        question,
        description,
        category,
        outcomes,
        status,
        resolution_source,
        end_time: end_time.map(|t| t.timestamp_millis()),
        volume_24h,
        total_volume,
        liquidity,
        created_at: created_at.timestamp_millis(),
    }))
}

/// Create a new prediction market (Admin only)
/// POST /admin/markets
pub async fn create_market(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateMarketRequest>,
) -> Result<Json<CreateMarketResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate condition_id format (should be 66 chars hex string with 0x prefix)
    if !req.condition_id.starts_with("0x") || req.condition_id.len() != 66 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid condition_id format. Must be 0x + 64 hex chars".to_string(),
                code: "INVALID_CONDITION_ID".to_string(),
            }),
        ));
    }

    // Check if market with this condition_id already exists
    let existing: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM markets WHERE condition_id = $1",
    )
    .bind(&req.condition_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to check existing market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    if existing.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "Market with this condition_id already exists".to_string(),
                code: "MARKET_EXISTS".to_string(),
            }),
        ));
    }

    let market_id = Uuid::new_v4();
    let yes_outcome_id = Uuid::new_v4();
    let no_outcome_id = Uuid::new_v4();
    let category = req.category.unwrap_or_else(|| "general".to_string());
    let resolution_source = req.resolution_source.unwrap_or_else(|| "UMA".to_string());
    let end_time = req.end_time.map(|ts| {
        chrono::DateTime::from_timestamp_millis(ts)
            .unwrap_or_else(chrono::Utc::now)
    });

    // Start transaction
    let mut tx = state.db.pool.begin().await.map_err(|e| {
        tracing::error!("Failed to start transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    // Create market
    sqlx::query(
        r#"
        INSERT INTO markets (id, condition_id, question, description, category, resolution_source, end_time)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(market_id)
    .bind(&req.condition_id)
    .bind(&req.question)
    .bind(&req.description)
    .bind(&category)
    .bind(&resolution_source)
    .bind(end_time)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to create market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create market".to_string(),
                code: "MARKET_CREATE_FAILED".to_string(),
            }),
        )
    })?;

    // Create Yes outcome
    sqlx::query(
        r#"
        INSERT INTO outcomes (id, market_id, token_id, name, share_type, complement_id, probability)
        VALUES ($1, $2, $3, 'Yes', 'yes', $4, 0.5)
        "#,
    )
    .bind(yes_outcome_id)
    .bind(market_id)
    .bind(&req.yes_token_id)
    .bind(no_outcome_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to create Yes outcome: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create outcomes".to_string(),
                code: "OUTCOME_CREATE_FAILED".to_string(),
            }),
        )
    })?;

    // Create No outcome
    sqlx::query(
        r#"
        INSERT INTO outcomes (id, market_id, token_id, name, share_type, complement_id, probability)
        VALUES ($1, $2, $3, 'No', 'no', $4, 0.5)
        "#,
    )
    .bind(no_outcome_id)
    .bind(market_id)
    .bind(&req.no_token_id)
    .bind(yes_outcome_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to create No outcome: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create outcomes".to_string(),
                code: "OUTCOME_CREATE_FAILED".to_string(),
            }),
        )
    })?;

    // Commit transaction
    tx.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to create market".to_string(),
                code: "TX_COMMIT_FAILED".to_string(),
            }),
        )
    })?;

    tracing::info!(
        "Created market {} with question: {}",
        market_id,
        req.question
    );

    Ok(Json(CreateMarketResponse {
        market_id,
        yes_outcome_id,
        no_outcome_id,
        message: "Market created successfully".to_string(),
    }))
}

/// Close a market (pause trading) - Admin only
/// POST /admin/markets/:market_id/close
pub async fn close_market(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Json(_req): Json<CloseMarketRequest>,
) -> Result<Json<MarketStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Check market exists and is active
    let market_status: Option<(String,)> = sqlx::query_as(
        "SELECT status::text FROM markets WHERE id = $1",
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (current_status,) = market_status.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market not found".to_string(),
                code: "MARKET_NOT_FOUND".to_string(),
            }),
        )
    })?;

    if current_status != "active" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Cannot close market with status: {}", current_status),
                code: "INVALID_STATUS".to_string(),
            }),
        ));
    }

    // Update market status to paused
    sqlx::query("UPDATE markets SET status = 'paused' WHERE id = $1")
        .bind(market_id)
        .execute(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to close market: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to close market".to_string(),
                    code: "MARKET_CLOSE_FAILED".to_string(),
                }),
            )
        })?;

    tracing::info!("Closed market {}", market_id);

    Ok(Json(MarketStatusResponse {
        market_id,
        status: "paused".to_string(),
        message: "Market has been closed. Trading is now paused.".to_string(),
    }))
}

/// Resolve a market (set winning outcome) - Admin only
/// POST /admin/markets/:market_id/resolve
pub async fn resolve_market(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Json(req): Json<ResolveMarketRequest>,
) -> Result<Json<MarketStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate winning_outcome
    let winning_share_type = match req.winning_outcome.to_lowercase().as_str() {
        "yes" => "yes",
        "no" => "no",
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "winning_outcome must be 'yes' or 'no'".to_string(),
                    code: "INVALID_OUTCOME".to_string(),
                }),
            ));
        }
    };

    // Check market exists and is active or paused
    let market_status: Option<(String,)> = sqlx::query_as(
        "SELECT status::text FROM markets WHERE id = $1",
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (current_status,) = market_status.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market not found".to_string(),
                code: "MARKET_NOT_FOUND".to_string(),
            }),
        )
    })?;

    if current_status == "resolved" || current_status == "cancelled" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Cannot resolve market with status: {}", current_status),
                code: "INVALID_STATUS".to_string(),
            }),
        ));
    }

    // Get winning outcome ID
    let winning_outcome: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM outcomes WHERE market_id = $1 AND share_type = $2::share_type",
    )
    .bind(market_id)
    .bind(winning_share_type)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch outcome: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (winning_outcome_id,) = winning_outcome.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Winning outcome not found".to_string(),
                code: "OUTCOME_NOT_FOUND".to_string(),
            }),
        )
    })?;

    // Update market status and winning outcome
    sqlx::query(
        r#"
        UPDATE markets
        SET status = 'resolved',
            winning_outcome_id = $1,
            resolved_at = NOW()
        WHERE id = $2
        "#,
    )
    .bind(winning_outcome_id)
    .bind(market_id)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to resolve market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to resolve market".to_string(),
                code: "MARKET_RESOLVE_FAILED".to_string(),
            }),
        )
    })?;

    // Update probabilities: winning = 1.0, losing = 0.0
    sqlx::query(
        r#"
        UPDATE outcomes
        SET probability = CASE
            WHEN id = $1 THEN 1.0
            ELSE 0.0
        END
        WHERE market_id = $2
        "#,
    )
    .bind(winning_outcome_id)
    .bind(market_id)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to update outcome probabilities: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to update probabilities".to_string(),
                code: "PROBABILITY_UPDATE_FAILED".to_string(),
            }),
        )
    })?;

    tracing::info!(
        "Resolved market {} with winning outcome: {}",
        market_id,
        winning_share_type
    );

    Ok(Json(MarketStatusResponse {
        market_id,
        status: "resolved".to_string(),
        message: format!("Market resolved. Winning outcome: {}", winning_share_type),
    }))
}

/// Update probability request
#[derive(Debug, Deserialize)]
pub struct UpdateProbabilityRequest {
    /// Outcome ID to update (must be a Yes outcome)
    pub outcome_id: Uuid,
    /// New probability (0.01 - 0.99)
    pub probability: Decimal,
}

/// Update probability response
#[derive(Debug, Serialize)]
pub struct UpdateProbabilityResponse {
    pub market_id: Uuid,
    pub outcome_id: Uuid,
    pub yes_probability: Decimal,
    pub no_probability: Decimal,
    pub message: String,
}

/// Refresh probability request
#[derive(Debug, Deserialize)]
pub struct RefreshProbabilityRequest {
    /// Source: "orderbook" or oracle name like "chainlink", "uma"
    pub source: Option<String>,
}

/// Update market probability manually - Admin only
/// POST /admin/markets/:market_id/probability
pub async fn update_probability(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Json(req): Json<UpdateProbabilityRequest>,
) -> Result<Json<UpdateProbabilityResponse>, (StatusCode, Json<ErrorResponse>)> {
    use crate::services::oracle::{OracleError, PriceOracle};

    // Validate probability range
    if req.probability < Decimal::new(1, 2) || req.probability > Decimal::new(99, 2) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Probability must be between 0.01 and 0.99".to_string(),
                code: "INVALID_PROBABILITY".to_string(),
            }),
        ));
    }

    // Create oracle service
    let oracle = PriceOracle::new(state.db.pool.clone(), state.matching_engine.clone());

    // Update probability
    oracle
        .set_probability_manual(market_id, req.outcome_id, req.probability)
        .await
        .map_err(|e| {
            let (status, code, message) = match &e {
                OracleError::MarketNotFound(_) => (
                    StatusCode::NOT_FOUND,
                    "MARKET_NOT_FOUND",
                    "市场不存在",
                ),
                OracleError::OutcomeNotFound(_) => (
                    StatusCode::NOT_FOUND,
                    "OUTCOME_NOT_FOUND",
                    "结果不存在",
                ),
                OracleError::MarketNotActive(_) => (
                    StatusCode::BAD_REQUEST,
                    "MARKET_NOT_ACTIVE",
                    "市场不是活跃状态",
                ),
                OracleError::InvalidProbability(_) => (
                    StatusCode::BAD_REQUEST,
                    "INVALID_PROBABILITY",
                    "无效的概率值",
                ),
                OracleError::DatabaseError(e) => {
                    tracing::error!("Database error updating probability: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "DATABASE_ERROR",
                        "数据库错误",
                    )
                }
                OracleError::ExternalOracleError(e) => {
                    tracing::error!("Oracle error: {}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "ORACLE_ERROR",
                        "预言机错误",
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

    let no_probability = Decimal::ONE - req.probability;

    tracing::info!(
        "Updated probability for market {}: yes={}, no={}",
        market_id, req.probability, no_probability
    );

    Ok(Json(UpdateProbabilityResponse {
        market_id,
        outcome_id: req.outcome_id,
        yes_probability: req.probability,
        no_probability,
        message: format!("Probability updated to {}", req.probability),
    }))
}

/// Refresh probability from orderbook - Admin only
/// POST /admin/markets/:market_id/refresh-probability
pub async fn refresh_probability(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
    Json(req): Json<RefreshProbabilityRequest>,
) -> Result<Json<UpdateProbabilityResponse>, (StatusCode, Json<ErrorResponse>)> {
    use crate::services::oracle::PriceOracle;

    let source = req.source.unwrap_or_else(|| "orderbook".to_string());

    // Get Yes outcome for this market
    let outcome: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM outcomes WHERE market_id = $1 AND share_type = 'yes'"
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch outcome: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (outcome_id,) = outcome.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market or outcome not found".to_string(),
                code: "NOT_FOUND".to_string(),
            }),
        )
    })?;

    // Create oracle service
    let oracle = PriceOracle::new(state.db.pool.clone(), state.matching_engine.clone());

    // Refresh probability based on source
    let probability = if source == "orderbook" {
        oracle
            .update_from_orderbook(market_id, outcome_id)
            .await
            .map_err(|e| {
                tracing::error!("Failed to refresh from orderbook: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to refresh: {}", e),
                        code: "REFRESH_FAILED".to_string(),
                    }),
                )
            })?
    } else {
        // Try external oracle
        oracle
            .fetch_from_external(market_id, &source)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("{}", e),
                        code: "ORACLE_ERROR".to_string(),
                    }),
                )
            })?
    };

    let no_probability = Decimal::ONE - probability;

    Ok(Json(UpdateProbabilityResponse {
        market_id,
        outcome_id,
        yes_probability: probability,
        no_probability,
        message: format!("Probability refreshed from {} to {}", source, probability),
    }))
}

/// Cancel a market - Admin only
/// POST /admin/markets/:market_id/cancel
pub async fn cancel_market(
    State(state): State<Arc<AppState>>,
    Path(market_id): Path<Uuid>,
) -> Result<Json<MarketStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Check market exists and is not already cancelled/resolved
    let market_status: Option<(String,)> = sqlx::query_as(
        "SELECT status::text FROM markets WHERE id = $1",
    )
    .bind(market_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch market: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Database error".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (current_status,) = market_status.ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Market not found".to_string(),
                code: "MARKET_NOT_FOUND".to_string(),
            }),
        )
    })?;

    if current_status == "resolved" || current_status == "cancelled" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Cannot cancel market with status: {}", current_status),
                code: "INVALID_STATUS".to_string(),
            }),
        ));
    }

    // Update market status to cancelled
    sqlx::query("UPDATE markets SET status = 'cancelled' WHERE id = $1")
        .bind(market_id)
        .execute(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to cancel market: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to cancel market".to_string(),
                    code: "MARKET_CANCEL_FAILED".to_string(),
                }),
            )
        })?;

    tracing::info!("Cancelled market {}", market_id);

    Ok(Json(MarketStatusResponse {
        market_id,
        status: "cancelled".to_string(),
        message: "Market has been cancelled. All positions will be refunded.".to_string(),
    }))
}
