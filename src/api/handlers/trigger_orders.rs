//! Trigger Orders API Handlers
//!
//! Handlers for stop-loss, take-profit, trailing stop, and other trigger orders

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::services::trigger_orders::{
    CreateTriggerOrderRequest, SetPositionTpSlRequest, TriggerOrder, TriggerOrderConfig,
    TriggerOrderExecution, TriggerOrderStatus, PositionTpSl, UserTriggerOrderStats,
};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: &str) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct TriggerOrdersQuery {
    pub market_symbol: Option<String>,
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionsQuery {
    pub limit: Option<i64>,
}

/// Create a new trigger order
/// POST /trigger-orders
pub async fn create_trigger_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(request): Json<CreateTriggerOrderRequest>,
) -> (StatusCode, Json<ApiResponse<TriggerOrder>>) {
    match state
        .trigger_orders_service
        .create_trigger_order(&auth_user.address, request)
        .await
    {
        Ok(order) => (StatusCode::CREATED, Json(ApiResponse::success(order))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get user's trigger orders
/// GET /trigger-orders
pub async fn get_trigger_orders(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<TriggerOrdersQuery>,
) -> (StatusCode, Json<ApiResponse<Vec<TriggerOrder>>>) {
    let status = query.status.as_deref().and_then(|s| {
        match s.to_lowercase().as_str() {
            "active" => Some(TriggerOrderStatus::Active),
            "triggered" => Some(TriggerOrderStatus::Triggered),
            "executed" => Some(TriggerOrderStatus::Executed),
            "cancelled" => Some(TriggerOrderStatus::Cancelled),
            "expired" => Some(TriggerOrderStatus::Expired),
            "failed" => Some(TriggerOrderStatus::Failed),
            _ => None,
        }
    });

    let limit = query.limit.unwrap_or(100);

    match state
        .trigger_orders_service
        .get_user_trigger_orders(&auth_user.address, query.market_symbol.as_deref(), status, limit)
        .await
    {
        Ok(orders) => (StatusCode::OK, Json(ApiResponse::success(orders))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get a specific trigger order
/// GET /trigger-orders/:order_id
pub async fn get_trigger_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(order_id): Path<Uuid>,
) -> (StatusCode, Json<ApiResponse<TriggerOrder>>) {
    match state
        .trigger_orders_service
        .get_trigger_order(&auth_user.address, order_id)
        .await
    {
        Ok(Some(order)) => (StatusCode::OK, Json(ApiResponse::success(order))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("Trigger order not found")),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Cancel a trigger order
/// DELETE /trigger-orders/:order_id
pub async fn cancel_trigger_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(order_id): Path<Uuid>,
) -> (StatusCode, Json<ApiResponse<TriggerOrder>>) {
    match state
        .trigger_orders_service
        .cancel_trigger_order(&auth_user.address, order_id)
        .await
    {
        Ok(order) => (StatusCode::OK, Json(ApiResponse::success(order))),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Set position TP/SL
/// POST /positions/:position_id/tp-sl
pub async fn set_position_tp_sl(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
    Json(request): Json<SetPositionTpSlRequest>,
) -> (StatusCode, Json<ApiResponse<PositionTpSl>>) {
    // First get the position to verify ownership and get market symbol
    match state.position_service.get_position_by_id(position_id).await {
        Ok(Some(position)) if position.user_address == auth_user.address => {
            match state
                .trigger_orders_service
                .set_position_tp_sl(&auth_user.address, position_id, &position.symbol, request)
                .await
            {
                Ok(tp_sl) => (StatusCode::OK, Json(ApiResponse::success(tp_sl))),
                Err(e) => (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::error(&e.to_string())),
                ),
            }
        }
        Ok(Some(_)) => (
            StatusCode::FORBIDDEN,
            Json(ApiResponse::error("Position does not belong to you")),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("Position not found")),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get position TP/SL settings
/// GET /positions/:position_id/tp-sl
pub async fn get_position_tp_sl(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(position_id): Path<Uuid>,
) -> (StatusCode, Json<ApiResponse<Option<PositionTpSl>>>) {
    // Verify position ownership first
    match state.position_service.get_position_by_id(position_id).await {
        Ok(Some(position)) if position.user_address == auth_user.address => {
            match state.trigger_orders_service.get_position_tp_sl(position_id).await {
                Ok(tp_sl) => (StatusCode::OK, Json(ApiResponse::success(tp_sl))),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::error(&e.to_string())),
                ),
            }
        }
        Ok(Some(_)) => (
            StatusCode::FORBIDDEN,
            Json(ApiResponse::error("Position does not belong to you")),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error("Position not found")),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get trigger order config for a market
/// GET /trigger-orders/:symbol/config
pub async fn get_trigger_order_config(
    State(state): State<Arc<AppState>>,
    Path(symbol): Path<String>,
) -> (StatusCode, Json<ApiResponse<TriggerOrderConfig>>) {
    match state.trigger_orders_service.get_config(&symbol).await {
        Ok(config) => (StatusCode::OK, Json(ApiResponse::success(config))),
        Err(e) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get user's trigger order execution history
/// GET /trigger-orders/executions
pub async fn get_user_executions(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Query(query): Query<ExecutionsQuery>,
) -> (StatusCode, Json<ApiResponse<Vec<TriggerOrderExecution>>>) {
    let limit = query.limit.unwrap_or(100);

    match state
        .trigger_orders_service
        .get_user_executions(&auth_user.address, limit)
        .await
    {
        Ok(executions) => (StatusCode::OK, Json(ApiResponse::success(executions))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}

/// Get user's trigger order stats for a market
/// GET /trigger-orders/:symbol/stats
pub async fn get_user_stats(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(symbol): Path<String>,
) -> (StatusCode, Json<ApiResponse<Option<UserTriggerOrderStats>>>) {
    match state
        .trigger_orders_service
        .get_user_stats(&auth_user.address, &symbol)
        .await
    {
        Ok(stats) => (StatusCode::OK, Json(ApiResponse::success(stats))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::error(&e.to_string())),
        ),
    }
}
