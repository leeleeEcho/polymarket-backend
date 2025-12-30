//! Order API Handlers
//!
//! Phase 8: Complete order execution pipeline with balance checking and matching engine integration

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::auth::eip712::{
    verify_create_order_signature_with_debug, verify_cancel_order_signature, verify_batch_cancel_signature,
    get_create_order_typed_data,
    CreateOrderMessage, CancelOrderMessage, BatchCancelMessage,
};
use crate::models::{CreateOrderRequest, Order, OrderResponse, OrderStatus, OrderType, OrderSide};
use crate::services::matching::{Side as MatchingSide, OrderType as MatchingOrderType, OrderStatus as MatchingOrderStatus};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct CancelOrderRequest {
    pub signature: String,
    pub timestamp: u64,
}

#[derive(Debug, Deserialize)]
pub struct BatchCancelRequest {
    pub order_ids: Vec<Uuid>,
    pub signature: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct BatchCancelResponse {
    pub cancelled: Vec<Uuid>,
    pub failed: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct CreateOrderResponse {
    pub order_id: Uuid,
    pub status: OrderStatus,
    pub filled_amount: Decimal,
    pub remaining_amount: Decimal,
    pub average_price: Decimal,  // Changed from Option<Decimal> to Decimal - use 0 if no fill
    #[serde(serialize_with = "serialize_datetime_as_millis")]
    pub created_at: chrono::DateTime<chrono::Utc>,
}

// Helper function to serialize DateTime as milliseconds timestamp
fn serialize_datetime_as_millis<S>(
    dt: &chrono::DateTime<chrono::Utc>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_i64(dt.timestamp_millis())
}


/// Validate timestamp (within 5 minutes)
fn validate_timestamp(timestamp: u64) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now.abs_diff(timestamp) <= 300
}

// Note: is_valid_symbol now uses config.is_valid_trading_pair() instead of hardcoded values

/// Create a new order
/// POST /orders
pub async fn create_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<CreateOrderRequest>,
) -> Result<Json<CreateOrderResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate symbol using config
    if !state.config.is_valid_trading_pair(&req.symbol) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("不支持的交易对: {}. 支持的交易对: {:?}", req.symbol, state.config.get_trading_pairs()),
                code: "INVALID_SYMBOL".to_string(),
            }),
        ));
    }

    // Validate timestamp (skip if auth is disabled for development)
    if !state.config.is_auth_disabled() && !validate_timestamp(req.timestamp) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
            }),
        ));
    }

    // Validate leverage
    if req.leverage < 1 || req.leverage > 50 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "杠杆倍数必须在1-50之间".to_string(),
                code: "INVALID_LEVERAGE".to_string(),
            }),
        ));
    }

    // Validate amount
    if req.amount <= Decimal::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "订单数量必须大于0".to_string(),
                code: "INVALID_AMOUNT".to_string(),
            }),
        ));
    }

    // Validate limit order has price
    if req.order_type == OrderType::Limit && req.price.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "限价单必须指定价格".to_string(),
                code: "PRICE_REQUIRED".to_string(),
            }),
        ));
    }

    // EIP-712 签名验证
    if !state.config.is_auth_disabled() {
        let order_msg = CreateOrderMessage {
            wallet: auth_user.address.to_lowercase(),
            symbol: req.symbol.clone(),
            side: format!("{}", req.side),           // 使用 Display: "buy"/"sell"
            order_type: format!("{}", req.order_type), // 使用 Display: "limit"/"market"
            price: req.price.map(|p| p.to_string()).unwrap_or_else(|| "0".to_string()),
            amount: req.amount.to_string(),
            leverage: req.leverage as u32,
            timestamp: req.timestamp,
        };

        // 详细调试日志 - 输出后端期望的 typed data
        let expected_typed_data = get_create_order_typed_data(&order_msg);
        tracing::debug!(
            "Create order message: wallet={}, symbol={}, side={}, order_type={}, price={}, amount={}, leverage={}, timestamp={}",
            order_msg.wallet, order_msg.symbol, order_msg.side, order_msg.order_type,
            order_msg.price, order_msg.amount, order_msg.leverage, order_msg.timestamp
        );
        tracing::debug!("Expected typed data for signing: {}", serde_json::to_string(&expected_typed_data).unwrap_or_default());

        let verify_result = match verify_create_order_signature_with_debug(&order_msg, &req.signature, &auth_user.address) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Create order signature verification error: {}", e);
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "签名格式无效".to_string(),
                        code: "INVALID_SIGNATURE_FORMAT".to_string(),
                    }),
                ));
            }
        };

        if !verify_result.is_valid {
            tracing::warn!(
                "Create order signature verification failed: recovered={}, expected={}, domain_separator={}, struct_hash={}, message_hash={}",
                verify_result.recovered_address,
                verify_result.expected_address,
                verify_result.domain_separator,
                verify_result.struct_hash,
                verify_result.message_hash
            );
            tracing::warn!("Expected typed data: {}", serde_json::to_string_pretty(&expected_typed_data).unwrap_or_default());
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "订单签名验证失败".to_string(),
                    code: "SIGNATURE_INVALID".to_string(),
                }),
            ));
        }

        tracing::info!("EIP-712 order signature verified for address: {}", auth_user.address);
    }

    // Check balance (collateral token from config) - skip if auth disabled for development
    let collateral_token = state.config.collateral_token();
    let collateral_symbol = state.config.collateral_symbol();
    let required_margin = calculate_required_margin_with_user(&req, &state, &auth_user.address.to_lowercase()).await;

    // --- BYPASS MARGIN CHECK FOR CLOSING POSITIONS ---
    let mut should_skip_balance_check = false;
    if !state.config.is_auth_disabled() {
        // Attempt to fetch position to check if it's a close.
        // We use 'size' column. If your DB uses 'size_in_tokens' or 'amount', please adjust.
        // We use unwrap_or(None) to safely ignore query errors (e.g. column not found).
        let pos_check: Option<(String, Decimal)> = sqlx::query_as(
             "SELECT side, size FROM positions WHERE user_address = $1 AND symbol = $2"
        )
        .bind(&auth_user.address.to_lowercase())
        .bind(&req.symbol)
        .fetch_optional(&state.db.pool)
        .await
        .unwrap_or(None);

        if let Some((pos_side, pos_size)) = pos_check {
             let is_buy = matches!(req.side, OrderSide::Buy);
             let is_short = pos_side.to_lowercase() == "short";
             let is_long = pos_side.to_lowercase() == "long";
             
             if (is_buy && is_short) || (!is_buy && is_long) {
                 if req.amount <= pos_size {
                     tracing::info!("Order {} {} <= Position {} {}: Closing, skipping balance check", 
                        if is_buy { "Buy" } else { "Sell" }, req.amount, pos_side, pos_size);
                     should_skip_balance_check = true;
                 }
             }
        }
    }
    // ------------------------------------------------

    if !state.config.is_auth_disabled() {
        let balance: Option<(Decimal, Decimal)> = sqlx::query_as(
            "SELECT available, frozen FROM balances WHERE user_address = $1 AND token = $2"
        )
        .bind(&auth_user.address.to_lowercase())
        .bind(collateral_symbol)
        .fetch_optional(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to check balance: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "检查余额失败".to_string(),
                    code: "BALANCE_CHECK_FAILED".to_string(),
                }),
            )
        })?;

        let available_balance = balance.map(|(a, _)| a).unwrap_or(Decimal::ZERO);

        if available_balance < required_margin && !should_skip_balance_check {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("可用余额不足，需要 {} {} 作为保证金", required_margin, collateral_symbol),
                    code: "INSUFFICIENT_BALANCE".to_string(),
                }),
            ));
        }
    } else {
        tracing::debug!("Auth disabled - skipping balance check");
    }

    // Create order in database
    let order_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    // Begin transaction
    let mut tx = state.db.pool.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库事务失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    // Freeze margin (skip if auth disabled for development)
    if !state.config.is_auth_disabled() {
        sqlx::query(
            r#"
            INSERT INTO balances (user_address, token, available, frozen)
            VALUES ($1, $2, 0, $3)
            ON CONFLICT (user_address, token)
            DO UPDATE SET
                available = balances.available - $3,
                frozen = balances.frozen + $3
            "#
        )
        .bind(&auth_user.address.to_lowercase())
        .bind(collateral_symbol)
        .bind(required_margin)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            tracing::error!("Failed to freeze margin: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "冻结保证金失败".to_string(),
                    code: "MARGIN_FREEZE_FAILED".to_string(),
                }),
            )
        })?;
    }

    // Insert order into database
    sqlx::query(
        r#"
        INSERT INTO orders (id, user_address, symbol, side, order_type, price, amount, filled_amount, leverage, status, signature, created_at, updated_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 0, $8, 'pending', $9, $10, $10)
        "#
    )
    .bind(order_id)
    .bind(&auth_user.address.to_lowercase())
    .bind(&req.symbol)
    .bind(req.side)
    .bind(req.order_type)
    .bind(req.price)
    .bind(req.amount)
    .bind(req.leverage)
    .bind(&req.signature)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to insert order: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "创建订单失败".to_string(),
                code: "ORDER_CREATE_FAILED".to_string(),
            }),
        )
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "事务提交失败".to_string(),
                code: "TX_COMMIT_FAILED".to_string(),
            }),
        )
    })?;

    // Submit to matching engine
    // Convert model types to matching engine types
    let matching_side = match req.side {
        OrderSide::Buy => MatchingSide::Buy,
        OrderSide::Sell => MatchingSide::Sell,
    };
    let matching_order_type = match req.order_type {
        OrderType::Limit => MatchingOrderType::Limit,
        OrderType::Market => MatchingOrderType::Market,
    };

    // PRE-CHECK: For market orders, ensure liquidity exists before submitting
    // This prevents the order from being auto-cancelled due to lack of counterparty
    if req.order_type == OrderType::Market && state.auto_market_maker.is_enabled() {
        tracing::info!(
            "Pre-checking liquidity for market order {} ({})", 
            order_id,
            req.symbol
        );
        
        // Call auto market maker to create liquidity if needed
        // This happens BEFORE submitting to matching engine
        match state.auto_market_maker.check_and_fill_order(
            &req.symbol,
            matching_side,
            req.amount,
            Decimal::ZERO, // filled_amount is 0 since we haven't submitted yet
            order_id,
        ).await {
            Ok(liquidity_provided) if liquidity_provided > Decimal::ZERO => {
                tracing::info!(
                    "Auto market maker pre-created liquidity: {} for order {}",
                    liquidity_provided,
                    order_id
                );
                // Brief delay to ensure limit order is in orderbook
                tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            }
            Ok(_) => {
                tracing::debug!("No auto-fill needed, orderbook has sufficient liquidity");
            }
            Err(e) => {
                tracing::warn!("Auto market maker pre-check failed: {}", e);
                // Continue anyway, let the normal flow handle it
            }
        }
    }

    let match_result = state.matching_engine.submit_order(
        order_id,
        &req.symbol,
        &auth_user.address.to_lowercase(),
        matching_side,
        matching_order_type,
        req.amount,
        req.price,
        req.leverage as u32,
    ).map_err(|e| {
        tracing::error!("Matching engine error: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "撮合引擎错误".to_string(),
                code: "MATCHING_ERROR".to_string(),
            }),
        )
    })?;

    // Post-match check: If market order is still not fully filled (unlikely with pre-check)
    // this serves as a fallback
    if req.order_type == OrderType::Market && match_result.filled_amount < req.amount {
        tracing::warn!(
            "Market order {} not fully filled despite pre-check: {}/{}",
            order_id,
            match_result.filled_amount,
            req.amount
        );
    }

    // Convert matching engine status back to model status
    let order_status = match match_result.status {
        MatchingOrderStatus::Open => OrderStatus::Open,
        MatchingOrderStatus::PartiallyFilled => OrderStatus::PartiallyFilled,
        MatchingOrderStatus::Filled => OrderStatus::Filled,
        MatchingOrderStatus::Cancelled => OrderStatus::Cancelled,
        MatchingOrderStatus::Rejected => OrderStatus::Rejected,
    };

    // Update order status in database
    // For market orders, also update price to the average fill price
    sqlx::query(
        "UPDATE orders SET status = $1, filled_amount = $2, price = COALESCE($3, price) WHERE id = $4"
    )
    .bind(order_status)
    .bind(match_result.filled_amount)
    .bind(match_result.average_price)  // Update price with average fill price
    .bind(order_id)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to update order status: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "更新订单状态失败".to_string(),
                code: "ORDER_UPDATE_FAILED".to_string(),
            }),
        )
    })?;

    // If order is filled or cancelled, unfreeze remaining margin
    if order_status == OrderStatus::Filled || order_status == OrderStatus::Cancelled {
        let remaining_margin = if order_status == OrderStatus::Filled {
            Decimal::ZERO // All margin used for position
        } else {
            required_margin // Return all margin for cancelled market orders
        };

        if remaining_margin > Decimal::ZERO {
            sqlx::query(
                "UPDATE balances SET available = available + $1, frozen = frozen - $1 WHERE user_address = $2 AND token = $3"
            )
            .bind(remaining_margin)
            .bind(&auth_user.address.to_lowercase())
            .bind(collateral_symbol)
            .execute(&state.db.pool)
            .await
            .ok();
        }
    }

    tracing::info!(
        "Order {} created: {} {} {} @ {:?}, status: {:?}",
        order_id,
        auth_user.address,
        req.side,
        req.amount,
        req.price,
        order_status
    );

    Ok(Json(CreateOrderResponse {
        order_id,
        status: order_status,
        filled_amount: match_result.filled_amount,
        remaining_amount: match_result.remaining_amount,
        average_price: match_result.average_price.unwrap_or(Decimal::ZERO),  // Use 0 if no fill
        created_at: now,
    }))
}

/// Calculate required margin for an order
async fn calculate_required_margin(req: &CreateOrderRequest, state: &Arc<AppState>) -> Decimal {
    // Get current price for market orders
    let price = match req.price {
        Some(p) => p,
        None => {
            // Use mark price for market orders
            state.price_feed_service
                .get_mark_price(&req.symbol)
                .await
                .unwrap_or(Decimal::ZERO)
        }
    };

    // Notional value = size * price
    let notional_value = req.amount * price;

    // Required margin = notional value / leverage
    // Add 0.5% buffer for fees and slippage
    let margin = notional_value / Decimal::from(req.leverage);
    let buffer = margin * Decimal::new(5, 3); // 0.5%

    margin + buffer
}

async fn calculate_required_margin_with_user(
    req: &CreateOrderRequest,
    state: &Arc<AppState>,
    user_address: &str,
) -> Decimal {
    use crate::models::{PositionSide, PositionStatus};

    // Get current price for market orders
    let price = match req.price {
        Some(p) => p,
        None => {
            // Use mark price for market orders
            state.price_feed_service
                .get_mark_price(&req.symbol)
                .await
                .unwrap_or(Decimal::ZERO)
        }
    };

    if price.is_zero() {
        // If no price available, use the old calculation
        return calculate_required_margin(req, state).await;
    }

    // Check if user has an opposite position (indicating this might be a closing order)
    let opposite_side = match req.side {
        OrderSide::Buy => PositionSide::Short,  // Buying closes short
        OrderSide::Sell => PositionSide::Long,  // Selling closes long
    };

    // Try to get the opposite position
    let existing_position = sqlx::query_as::<_, (Decimal, Decimal)>(
        r#"
        SELECT size_in_usd, size_in_tokens 
        FROM positions 
        WHERE user_address = $1 AND symbol = $2 AND side = $3 AND status = 'open'
        "#
    )
    .bind(user_address)
    .bind(&req.symbol)
    .bind(opposite_side)
    .fetch_optional(&state.db.pool)
    .await
    .ok()
    .flatten();

    if let Some((position_size_usd, position_size_tokens)) = existing_position {
        // User has an opposite position - this order might be closing it
        let order_size_usd = req.amount * price;

        if order_size_usd <= position_size_usd {
            // Order is fully within the existing position - this is a pure close
            // No additional margin required! Just need to cover fees
            let fee_estimate = order_size_usd * Decimal::new(5, 3); // 0.5% for fees
            
            tracing::info!(
                "Order for {} {} {} @ {} is closing existing {} position (size: {} USD). Required margin (fees only): {}",
                req.amount, req.symbol, req.side, price, opposite_side, position_size_usd, fee_estimate
            );
            
            return fee_estimate;
        } else {
            // Order is larger than existing position - partially closing
            // Only require margin for the net new position
            let net_new_size_usd = order_size_usd - position_size_usd;
            let net_new_margin = net_new_size_usd / Decimal::from(req.leverage);
            let buffer = net_new_margin * Decimal::new(5, 3); // 0.5%
            
            tracing::info!(
                "Order for {} {} {} @ {} is partially closing {} position (existing: {} USD, new: {} USD). Required margin: {}",
                req.amount, req.symbol, req.side, price, opposite_side, position_size_usd, net_new_size_usd, net_new_margin + buffer
            );
            
            return net_new_margin + buffer;
        }
    }

    // No opposite position - this is a pure open/increase
    let notional_value = req.amount * price;
    let margin = notional_value / Decimal::from(req.leverage);
    let buffer = margin * Decimal::new(5, 3); // 0.5%

    margin + buffer
}

/// Cancel a single order
/// DELETE /orders/:order_id
pub async fn cancel_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(order_id): Path<Uuid>,
    Json(req): Json<CancelOrderRequest>,
) -> Result<Json<OrderResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate timestamp (skip if auth is disabled for development)
    if !state.config.is_auth_disabled() && !validate_timestamp(req.timestamp) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
            }),
        ));
    }

    // EIP-712 签名验证
    if !state.config.is_auth_disabled() {
        let cancel_msg = CancelOrderMessage {
            wallet: auth_user.address.to_lowercase(),
            order_id: order_id.to_string(),
            timestamp: req.timestamp,
        };

        let valid = match verify_cancel_order_signature(&cancel_msg, &req.signature, &auth_user.address) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Cancel order signature verification error: {}", e);
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "签名格式无效".to_string(),
                        code: "INVALID_SIGNATURE_FORMAT".to_string(),
                    }),
                ));
            }
        };

        if !valid {
            tracing::warn!("Cancel order signature verification failed for order: {}", order_id);
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "取消订单签名验证失败".to_string(),
                    code: "SIGNATURE_INVALID".to_string(),
                }),
            ));
        }

        tracing::info!("EIP-712 cancel order signature verified for order: {}", order_id);
    }

    // Check order exists and belongs to user
    let order: Option<Order> = sqlx::query_as(
        "SELECT * FROM orders WHERE id = $1"
    )
    .bind(order_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch order: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "查询订单失败".to_string(),
                code: "ORDER_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let order = order.ok_or((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "订单不存在".to_string(),
            code: "ORDER_NOT_FOUND".to_string(),
        }),
    ))?;

    if order.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "无权取消此订单".to_string(),
                code: "ORDER_NOT_OWNED".to_string(),
            }),
        ));
    }

    if order.status != OrderStatus::Open && order.status != OrderStatus::PartiallyFilled && order.status != OrderStatus::Pending {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("订单状态为 {:?}，无法取消", order.status),
                code: "ORDER_NOT_CANCELLABLE".to_string(),
            }),
        ));
    }

    // Cancel in matching engine
    let cancelled = state.matching_engine.cancel_order(&order.symbol, order_id, &auth_user.address.to_lowercase()).map_err(|e| {
        tracing::error!("Failed to cancel order in matching engine: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "撮合引擎取消订单失败".to_string(),
                code: "MATCHING_CANCEL_FAILED".to_string(),
            }),
        )
    })?;

    if !cancelled {
        // Order might already be filled, update from DB
        tracing::warn!("Order {} not found in matching engine", order_id);
    }

    // Update database
    sqlx::query("UPDATE orders SET status = 'cancelled' WHERE id = $1")
        .bind(order_id)
        .execute(&state.db.pool)
        .await
        .map_err(|e| {
            tracing::error!("Failed to update order status: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "更新订单状态失败".to_string(),
                    code: "ORDER_UPDATE_FAILED".to_string(),
                }),
            )
        })?;

    // Unfreeze remaining margin
    // Must match the calculation in calculate_required_margin() which includes 0.5% buffer
    let collateral_symbol = state.config.collateral_symbol();
    let remaining_amount = order.amount - order.filled_amount;

    // Get price: use order price for limit orders, or mark price for market orders
    let price = match order.price {
        Some(p) => p,
        None => {
            // For market orders, use mark price
            state.price_feed_service
                .get_mark_price(&order.symbol)
                .await
                .unwrap_or(Decimal::from(100000))
        }
    };

    // Calculate margin the same way as when creating the order
    let notional_value = remaining_amount * price;
    let base_margin = notional_value / Decimal::from(order.leverage);
    let buffer = base_margin * Decimal::new(5, 3); // 0.5% buffer (same as creation)
    let remaining_margin = base_margin + buffer;

    sqlx::query(
        "UPDATE balances SET available = available + $1, frozen = frozen - $1 WHERE user_address = $2 AND token = $3"
    )
    .bind(remaining_margin)
    .bind(&auth_user.address.to_lowercase())
    .bind(collateral_symbol)
    .execute(&state.db.pool)
    .await
    .ok();

    tracing::info!("Order cancelled: {} by {}", order_id, auth_user.address);

    Ok(Json(OrderResponse {
        order_id,
        symbol: order.symbol,
        side: order.side,
        order_type: order.order_type,
        price: order.price.unwrap_or(Decimal::ZERO),
        amount: order.amount,
        filled_amount: order.filled_amount,
        remaining_amount: order.amount - order.filled_amount,
        leverage: order.leverage,
        status: OrderStatus::Cancelled,
        created_at: order.created_at,
    }))
}

/// Batch cancel orders
/// POST /orders/batch
pub async fn batch_cancel(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<BatchCancelRequest>,
) -> Result<Json<BatchCancelResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate timestamp (skip if auth is disabled for development)
    if !state.config.is_auth_disabled() && !validate_timestamp(req.timestamp) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
            }),
        ));
    }

    // EIP-712 签名验证
    if !state.config.is_auth_disabled() {
        let batch_cancel_msg = BatchCancelMessage {
            wallet: auth_user.address.to_lowercase(),
            order_ids: req.order_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
            timestamp: req.timestamp,
        };

        let valid = match verify_batch_cancel_signature(&batch_cancel_msg, &req.signature, &auth_user.address) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("Batch cancel signature verification error: {}", e);
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "签名格式无效".to_string(),
                        code: "INVALID_SIGNATURE_FORMAT".to_string(),
                    }),
                ));
            }
        };

        if !valid {
            tracing::warn!("Batch cancel signature verification failed for {} orders", req.order_ids.len());
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(ErrorResponse {
                    error: "批量取消订单签名验证失败".to_string(),
                    code: "SIGNATURE_INVALID".to_string(),
                }),
            ));
        }

        tracing::info!("EIP-712 batch cancel signature verified for {} orders", req.order_ids.len());
    }

    let mut cancelled = Vec::new();
    let mut failed = Vec::new();

    for order_id in req.order_ids {
        // Check order ownership
        let order: Option<Order> = sqlx::query_as(
            "SELECT * FROM orders WHERE id = $1 AND user_address = $2 AND status IN ('open', 'partially_filled', 'pending')"
        )
        .bind(order_id)
        .bind(&auth_user.address.to_lowercase())
        .fetch_optional(&state.db.pool)
        .await
        .ok()
        .flatten();

        if let Some(order) = order {
            // Try to cancel in matching engine
            let result = state.matching_engine.cancel_order(&order.symbol, order_id, &auth_user.address.to_lowercase());

            // Update database regardless
            let db_result = sqlx::query("UPDATE orders SET status = 'cancelled' WHERE id = $1")
                .bind(order_id)
                .execute(&state.db.pool)
                .await;

            if result.is_ok() || db_result.is_ok() {
                // Unfreeze remaining margin (must match calculate_required_margin logic with 0.5% buffer)
                let remaining_amount = order.amount - order.filled_amount;
                if remaining_amount > Decimal::ZERO {
                    let collateral_symbol = state.config.collateral_symbol();

                    // Get price: use order price for limit orders, or mark price for market orders
                    let price = match order.price {
                        Some(p) => p,
                        None => {
                            state.price_feed_service
                                .get_mark_price(&order.symbol)
                                .await
                                .unwrap_or(Decimal::from(100000))
                        }
                    };

                    // Calculate margin the same way as when creating the order
                    let notional_value = remaining_amount * price;
                    let base_margin = notional_value / Decimal::from(order.leverage);
                    let buffer = base_margin * Decimal::new(5, 3); // 0.5% buffer
                    let remaining_margin = base_margin + buffer;

                    sqlx::query(
                        "UPDATE balances SET available = available + $1, frozen = frozen - $1 WHERE user_address = $2 AND token = $3"
                    )
                    .bind(remaining_margin)
                    .bind(&auth_user.address.to_lowercase())
                    .bind(collateral_symbol)
                    .execute(&state.db.pool)
                    .await
                    .ok();
                }

                cancelled.push(order_id);
            } else {
                failed.push(order_id);
            }
        } else {
            failed.push(order_id);
        }
    }

    tracing::info!(
        "Batch cancel: {} cancelled, {} failed by {}",
        cancelled.len(),
        failed.len(),
        auth_user.address
    );

    Ok(Json(BatchCancelResponse { cancelled, failed }))
}

/// Get a single order by ID
/// GET /orders/:order_id
pub async fn get_order(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<OrderResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Fetch order from database
    let order: Option<Order> = sqlx::query_as(
        "SELECT * FROM orders WHERE id = $1"
    )
    .bind(order_id)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch order: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "查询订单失败".to_string(),
                code: "ORDER_FETCH_FAILED".to_string(),
            }),
        )
    })?;

    let order = order.ok_or((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "订单不存在".to_string(),
            code: "ORDER_NOT_FOUND".to_string(),
        }),
    ))?;

    // Check if user owns the order
    if order.user_address.to_lowercase() != auth_user.address.to_lowercase() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "无权访问此订单".to_string(),
                code: "ORDER_ACCESS_DENIED".to_string(),
            }),
        ));
    }

    Ok(Json(OrderResponse {
        order_id: order.id,
        symbol: order.symbol,
        side: order.side,
        order_type: order.order_type,
        price: order.price.unwrap_or(Decimal::ZERO),  // Ensure price is never null
        amount: order.amount,
        filled_amount: order.filled_amount,
        remaining_amount: order.amount - order.filled_amount,
        leverage: order.leverage,
        status: order.status,
        created_at: order.created_at,
    }))
}
