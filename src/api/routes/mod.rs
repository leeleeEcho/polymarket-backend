use axum::{
    middleware as axum_middleware,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

use crate::api::handlers;
use crate::auth::middleware::{admin_middleware, auth_middleware};
use crate::AppState;

pub fn create_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Public routes (no auth required)
    let public_routes = Router::new()
        // Auth
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/nonce/:address", get(handlers::auth::get_nonce))
        // Markets (prediction market specific)
        .route("/markets", get(handlers::market::list_markets))
        .route("/markets/:market_id", get(handlers::market::get_market))
        .route("/markets/:market_id/orderbook", get(handlers::market::get_orderbook))
        .route("/markets/:market_id/trades", get(handlers::market::get_trades))
        .route("/markets/:market_id/ticker", get(handlers::market::get_ticker))
        .route("/markets/:market_id/price", get(handlers::market::get_price));

    // Protected routes (auth required)
    let protected_routes = Router::new()
        // Account
        .route("/account/profile", get(handlers::account::get_profile))
        .route("/account/balances", get(handlers::account::get_balances))
        .route("/account/shares", get(handlers::account::get_shares))
        .route("/account/orders", get(handlers::account::get_orders))
        .route("/account/trades", get(handlers::account::get_trades))
        // Settlement
        .route("/account/settle/:market_id", post(handlers::account::settle_market))
        .route("/account/settle/:market_id/status", get(handlers::account::get_settlement_status))
        // Orders
        .route("/orders", post(handlers::order::create_order))
        .route("/orders/:order_id", get(handlers::order::get_order))
        .route("/orders/:order_id", delete(handlers::order::cancel_order))
        .route("/orders/batch", post(handlers::order::batch_cancel))
        // Deposits & Withdrawals
        .route("/deposit/prepare", post(handlers::deposit::prepare_deposit))
        .route("/deposit/history", get(handlers::deposit::get_history))
        .route("/withdraw/request", post(handlers::withdraw::request_withdraw))
        .route("/withdraw/history", get(handlers::withdraw::get_history))
        .route("/withdraw/:id", get(handlers::withdraw::get_withdrawal))
        .route("/withdraw/:id/cancel", delete(handlers::withdraw::cancel_withdraw))
        .route("/withdraw/:id/confirm", post(handlers::withdraw::confirm_withdraw))
        .layer(axum_middleware::from_fn_with_state(state.clone(), auth_middleware));

    // Admin routes (auth required + admin role check)
    let admin_routes = Router::new()
        .route("/admin/markets", post(handlers::market::create_market))
        .route("/admin/markets/:market_id/close", post(handlers::market::close_market))
        .route("/admin/markets/:market_id/resolve", post(handlers::market::resolve_market))
        .route("/admin/markets/:market_id/cancel", post(handlers::market::cancel_market))
        .route("/admin/markets/:market_id/probability", post(handlers::market::update_probability))
        .route("/admin/markets/:market_id/refresh-probability", post(handlers::market::refresh_probability))
        // Admin middleware must come BEFORE auth middleware in the layer chain
        // (layers are applied in reverse order, so auth runs first, then admin)
        .layer(axum_middleware::from_fn(admin_middleware))
        .layer(axum_middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
        .merge(admin_routes)
}
