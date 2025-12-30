//! Position API handlers

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use chrono::Utc;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::models::{ClosePositionRequest, OpenPositionRequest, PositionResponse, PositionSide};
use crate::models::order::{OrderResponse, OrderSide, OrderStatus, OrderType};
use crate::{AppState, OrderUpdateEvent};

/// Error response for position operations
#[derive(Debug, Serialize)]
pub struct PositionErrorResponse {
    pub error: String,
    pub code: String,
}

/// Response for position list
#[derive(Debug, Serialize)]
pub struct PositionsListResponse {
    pub positions: Vec<PositionResponse>,
    pub total_unrealized_pnl: Decimal,
    pub total_collateral: Decimal,
}

/// Response for position action
#[derive(Debug, Serialize)]
pub struct PositionActionResponse {
    pub success: bool,
    pub message: String,
    pub position: Option<PositionResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<OrderResponse>,
}

/// Add collateral request
#[derive(Debug, Deserialize)]
pub struct AddCollateralRequest {
    pub amount: Decimal,
}

/// Remove collateral request
#[derive(Debug, Deserialize)]
pub struct RemoveCollateralRequest {
    pub amount: Decimal,
}

/// Get all positions for authenticated user
pub async fn get_positions(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<PositionsListResponse>, StatusCode> {
    let positions = state
        .position_service
        .get_user_positions(&auth_user.address)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get current prices for PnL calculation
    let mut responses = Vec::new();
    let mut total_unrealized_pnl = Decimal::ZERO;
    let mut total_collateral = Decimal::ZERO;

    for position in positions {
        // Get mark price from price feed
        let mark_price = state
            .price_feed_service
            .get_mark_price(&position.symbol)
            .await
            .unwrap_or(position.entry_price);

        let response = state
            .position_service
            .position_to_response(&position, mark_price);

        total_unrealized_pnl += response.unrealized_pnl;
        total_collateral += response.collateral_amount;
        responses.push(response);
    }

    Ok(Json(PositionsListResponse {
        positions: responses,
        total_unrealized_pnl,
        total_collateral,
    }))
}

/// Get a specific position by ID
pub async fn get_position(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
) -> Result<Json<PositionResponse>, StatusCode> {
    let position = state
        .position_service
        .get_position_by_id(position_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Verify ownership
    if position.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get mark price
    let mark_price = state
        .price_feed_service
        .get_mark_price(&position.symbol)
        .await
        .unwrap_or(position.entry_price);

    let response = state
        .position_service
        .position_to_response(&position, mark_price);

    Ok(Json(response))
}

/// Open a new position or increase existing
pub async fn open_position(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<OpenPositionRequest>,
) -> Result<Json<PositionActionResponse>, (StatusCode, Json<PositionErrorResponse>)> {
    let user_address = auth_user.address.to_lowercase();

    // Check user balance before opening position
    let collateral_symbol = state.config.collateral_symbol();
    let balance: Option<(Decimal, Decimal)> = sqlx::query_as(
        "SELECT available, frozen FROM balances WHERE user_address = $1 AND token = $2"
    )
    .bind(&user_address)
    .bind(collateral_symbol)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch balance: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(PositionErrorResponse {
                error: "获取余额失败".to_string(),
                code: "BALANCE_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let available_balance = balance.map(|(available, _)| available).unwrap_or(Decimal::ZERO);

    // Check if user has enough balance for the collateral
    if available_balance < req.collateral_amount {
        tracing::warn!(
            "Insufficient balance for user {}: available={}, required={}",
            user_address,
            available_balance,
            req.collateral_amount
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(PositionErrorResponse {
                error: format!(
                    "余额不足: 可用余额 {} {}, 需要 {} {}",
                    available_balance, collateral_symbol, req.collateral_amount, collateral_symbol
                ),
                code: "INSUFFICIENT_BALANCE".to_string(),
            }),
        ));
    }

    // Get current mark price
    let mark_price = state
        .price_feed_service
        .get_mark_price(&req.symbol)
        .await
        .ok_or_else(|| {
            tracing::error!("No mark price available for symbol: {}", req.symbol);
            (
                StatusCode::BAD_REQUEST,
                Json(PositionErrorResponse {
                    error: "无法获取当前市场价格".to_string(),
                    code: "PRICE_UNAVAILABLE".to_string(),
                }),
            )
        })?;

    // Deduct collateral from user balance (freeze it)
    sqlx::query(
        r#"
        UPDATE balances
        SET available = available - $1, frozen = frozen + $1, updated_at = NOW()
        WHERE user_address = $2 AND token = $3
        "#
    )
    .bind(req.collateral_amount)
    .bind(&user_address)
    .bind(collateral_symbol)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to update balance: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(PositionErrorResponse {
                error: "更新余额失败".to_string(),
                code: "BALANCE_UPDATE_FAILED".to_string(),
            }),
        )
    })?;

    // Use the unified increase_position method which handles both new and existing positions
    let result = state
        .position_service
        .increase_position(
            &user_address,
            &req.symbol,
            req.side,
            req.collateral_amount,
            req.leverage,
            mark_price,
            false, // Enforce min size check for user-initiated position increases
        )
        .await
        .map_err(|e| {
            // Rollback the balance change on error
            let rollback_symbol = collateral_symbol.to_string();
            let rollback_result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    sqlx::query(
                        r#"
                        UPDATE balances
                        SET available = available + $1, frozen = frozen - $1, updated_at = NOW()
                        WHERE user_address = $2 AND token = $3
                        "#
                    )
                    .bind(req.collateral_amount)
                    .bind(&user_address)
                    .bind(&rollback_symbol)
                    .execute(&state.db.pool)
                    .await
                })
            });

            if let Err(rollback_err) = rollback_result {
                tracing::error!("Failed to rollback balance: {}", rollback_err);
            }

            tracing::error!("Failed to open/increase position: {:?}", e);
            (
                StatusCode::BAD_REQUEST,
                Json(PositionErrorResponse {
                    error: format!("开仓失败: {}", e),
                    code: "POSITION_OPEN_FAILED".to_string(),
                }),
            )
        })?;

    Ok(Json(PositionActionResponse {
        success: true,
        message: "Position opened successfully".to_string(),
        position: Some(result.position),
        order: None,
    }))
}

/// Close a position (fully or partially)
pub async fn close_position(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
    Json(req): Json<ClosePositionRequest>,
) -> Result<Json<PositionActionResponse>, StatusCode> {
    // Verify ownership
    let position = state
        .position_service
        .get_position_by_id(position_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if position.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get mark price
    let execution_price = req.price.unwrap_or(
        state
            .price_feed_service
            .get_mark_price(&position.symbol)
            .await
            .unwrap_or(position.entry_price),
    );

    // Convert token amount to USD amount
    // Frontend sends amount in tokens, but decrease_position expects USD
    let size_delta_usd = req.amount.map(|token_amount| {
        let usd_amount = token_amount * execution_price;
        tracing::info!(
            "Close position: converting token amount {} to USD amount {} at price {}",
            token_amount, usd_amount, execution_price
        );
        usd_amount
    });

    // Decrease/close position
    let result = state
        .position_service
        .decrease_position(position_id, size_delta_usd, execution_price)
        .await
        .map_err(|e| {
            tracing::error!("Failed to close position: {:?}", e);
            StatusCode::BAD_REQUEST
        })?;

    // Create a close order record
    // Long position closes with Sell order, Short position closes with Buy order
    let order_side = match position.side {
        PositionSide::Long => OrderSide::Sell,
        PositionSide::Short => OrderSide::Buy,
    };

    // Calculate the actual closed amount in tokens
    let closed_amount_tokens = result.size_delta_usd / execution_price;

    let order_id = Uuid::new_v4();
    let now = Utc::now();

    // Insert close order into database
    let insert_result = sqlx::query(
        r#"
        INSERT INTO orders (id, user_address, symbol, side, order_type, price, amount, filled_amount, leverage, status, signature, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $7, $8, 'filled', $9, $10, $10)
        "#
    )
    .bind(order_id)
    .bind(&auth_user.address.to_lowercase())
    .bind(&position.symbol)
    .bind(order_side)
    .bind(OrderType::Market)
    .bind(execution_price)
    .bind(closed_amount_tokens)
    .bind(position.leverage)
    .bind("close-position")
    .bind(now)
    .execute(&state.db.pool)
    .await;

    let order_response = match insert_result {
        Ok(_) => {
            tracing::info!(
                "Created close order {} for position {}: {} {} {} @ {}",
                order_id, position_id, order_side, closed_amount_tokens, position.symbol, execution_price
            );
            let order = OrderResponse {
                order_id,
                symbol: position.symbol.clone(),
                side: order_side,
                order_type: OrderType::Market,
                price: execution_price,
                amount: closed_amount_tokens,
                filled_amount: closed_amount_tokens,
                remaining_amount: Decimal::ZERO,
                leverage: position.leverage,
                status: OrderStatus::Filled,
                created_at: now,
            };

            // Send order update to WebSocket broadcast channel
            let event = OrderUpdateEvent {
                user_address: auth_user.address.to_lowercase(),
                order: order.clone(),
            };
            if let Err(e) = state.order_update_sender.send(event) {
                tracing::warn!("Failed to broadcast order update: {} (no receivers)", e);
            } else {
                tracing::info!("Broadcasted close order {} to WebSocket", order_id);
            }

            Some(order)
        }
        Err(e) => {
            tracing::error!("Failed to create close order: {}", e);
            None
        }
    };

    Ok(Json(PositionActionResponse {
        success: true,
        message: if result.is_fully_closed {
            "Position fully closed".to_string()
        } else {
            "Position partially closed".to_string()
        },
        position: result.position,
        order: order_response,
    }))
}

/// Add collateral to a position
pub async fn add_collateral(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
    Json(req): Json<AddCollateralRequest>,
) -> Result<Json<PositionActionResponse>, StatusCode> {
    // Verify ownership
    let position = state
        .position_service
        .get_position_by_id(position_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if position.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    let updated = state
        .position_service
        .add_collateral(position_id, req.amount)
        .await
        .map_err(|e| {
            tracing::error!("Failed to add collateral: {:?}", e);
            StatusCode::BAD_REQUEST
        })?;

    // Get mark price
    let mark_price = state
        .price_feed_service
        .get_mark_price(&updated.symbol)
        .await
        .unwrap_or(updated.entry_price);

    let response = state
        .position_service
        .position_to_response(&updated, mark_price);

    Ok(Json(PositionActionResponse {
        success: true,
        message: "Collateral added successfully".to_string(),
        position: Some(response),
        order: None,
    }))
}

/// Remove collateral from a position
pub async fn remove_collateral(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
    Json(req): Json<RemoveCollateralRequest>,
) -> Result<Json<PositionActionResponse>, StatusCode> {
    // Verify ownership
    let position = state
        .position_service
        .get_position_by_id(position_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if position.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get mark price for validation
    let mark_price = state
        .price_feed_service
        .get_mark_price(&position.symbol)
        .await
        .unwrap_or(position.entry_price);

    let updated = state
        .position_service
        .remove_collateral(position_id, req.amount, mark_price)
        .await
        .map_err(|e| {
            tracing::error!("Failed to remove collateral: {:?}", e);
            StatusCode::BAD_REQUEST
        })?;

    let response = state
        .position_service
        .position_to_response(&updated, mark_price);

    Ok(Json(PositionActionResponse {
        success: true,
        message: "Collateral removed successfully".to_string(),
        position: Some(response),
        order: None,
    }))
}

/// Check liquidation status for a position
pub async fn check_liquidation(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
) -> Result<Json<crate::models::LiquidationInfo>, StatusCode> {
    // Verify ownership
    let position = state
        .position_service
        .get_position_by_id(position_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if position.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Get mark price
    let mark_price = state
        .price_feed_service
        .get_mark_price(&position.symbol)
        .await
        .unwrap_or(position.entry_price);

    let info = state.position_service.check_liquidation(&position, mark_price);

    Ok(Json(info))
}
