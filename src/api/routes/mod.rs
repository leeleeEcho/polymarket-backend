use axum::{
    middleware as axum_middleware,
    routing::{delete, get, post},
    Router,
};
use std::sync::Arc;

use crate::api::handlers;
use crate::auth::middleware::auth_middleware;
use crate::AppState;

pub fn create_router(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // Public routes (no auth required)
    let public_routes = Router::new()
        .route("/auth/login", post(handlers::auth::login))
        .route("/auth/nonce/:address", get(handlers::auth::get_nonce))
        .route("/markets", get(handlers::market::list_markets))
        .route("/markets/:symbol/orderbook", get(handlers::market::get_orderbook))
        .route("/markets/:symbol/trades", get(handlers::market::get_trades))
        .route("/markets/:symbol/ticker", get(handlers::market::get_ticker))
        .route("/markets/:symbol/price", get(handlers::market::get_price))
        // K-line/Candles
        .route("/markets/:symbol/candles", get(handlers::kline::get_candles))
        .route("/markets/:symbol/candles/latest", get(handlers::kline::get_latest_candle))
        // Alternative paths for frontend compatibility
        .route("/klines/:symbol/candles", get(handlers::kline::get_candles))
        .route("/klines/:symbol/candles/latest", get(handlers::kline::get_latest_candle))
        // Internal K-line endpoints
        .route("/internal/klines/import", post(handlers::kline::batch_import_klines))
        .route("/internal/klines/repair", get(handlers::kline::repair_klines))
        // Funding rate (public)
        .route("/funding-rates", get(handlers::funding_rate::get_all_funding_rates))
        .route("/funding-rates/:symbol", get(handlers::funding_rate::get_funding_rate))
        .route("/funding-rates/:symbol/history", get(handlers::funding_rate::get_funding_history))
        // Liquidation (public)
        .route("/liquidations/:symbol", get(handlers::liquidation::get_market_liquidations))
        .route("/liquidations/:symbol/config", get(handlers::liquidation::get_liquidation_config))
        .route("/insurance-fund/:symbol", get(handlers::liquidation::get_insurance_fund))
        // ADL (public)
        .route("/adl/:symbol/rankings", get(handlers::adl::get_adl_rankings))
        .route("/adl/:symbol/events", get(handlers::adl::get_market_adl_events))
        .route("/adl/:symbol/config", get(handlers::adl::get_adl_config))
        // Trigger orders config (public)
        .route("/trigger-orders/:symbol/config", get(handlers::trigger_orders::get_trigger_order_config))
        // On-chain referral data (public)
        .route("/referral/on-chain/user-rebate/:address", get(handlers::referral::get_on_chain_user_rebate))
        .route("/referral/on-chain/referral-info/:address", get(handlers::referral::get_on_chain_referral_info))
        .route("/referral/on-chain/claimed/:address", get(handlers::referral::get_on_chain_claimed))
        .route("/referral/on-chain/operator-status", get(handlers::referral::get_operator_status));

    // Protected routes (auth required)
    let protected_routes = Router::new()
        // Account
        .route("/account/profile", get(handlers::account::get_profile))
        .route("/account/balances", get(handlers::account::get_balances))
        .route("/account/positions", get(handlers::account::get_positions))
        .route("/account/orders", get(handlers::account::get_orders))
        .route("/account/trades", get(handlers::account::get_trades))
        // Orders
        .route("/orders", post(handlers::order::create_order))
        .route("/orders/:order_id", get(handlers::order::get_order))
        .route("/orders/:order_id", delete(handlers::order::cancel_order))
        .route("/orders/batch", post(handlers::order::batch_cancel))
        // Positions
        .route("/positions", get(handlers::position::get_positions))
        .route("/positions", post(handlers::position::open_position))
        .route("/positions/:position_id", get(handlers::position::get_position))
        .route("/positions/:position_id/close", post(handlers::position::close_position))
        .route("/positions/:position_id/collateral/add", post(handlers::position::add_collateral))
        .route("/positions/:position_id/collateral/remove", post(handlers::position::remove_collateral))
        .route("/positions/:position_id/liquidation", get(handlers::position::check_liquidation))
        // Position TP/SL
        .route("/positions/:position_id/tp-sl", post(handlers::trigger_orders::set_position_tp_sl))
        .route("/positions/:position_id/tp-sl", get(handlers::trigger_orders::get_position_tp_sl))
        // Trigger orders
        .route("/trigger-orders", post(handlers::trigger_orders::create_trigger_order))
        .route("/trigger-orders", get(handlers::trigger_orders::get_trigger_orders))
        .route("/trigger-orders/executions", get(handlers::trigger_orders::get_user_executions))
        .route("/trigger-orders/:order_id", get(handlers::trigger_orders::get_trigger_order))
        .route("/trigger-orders/:order_id", delete(handlers::trigger_orders::cancel_trigger_order))
        .route("/trigger-orders/:symbol/stats", get(handlers::trigger_orders::get_user_stats))
        // Deposits & Withdrawals
        .route("/deposit/prepare", post(handlers::deposit::prepare_deposit))
        .route("/deposit/history", get(handlers::deposit::get_history))
        .route("/withdraw/request", post(handlers::withdraw::request_withdraw))
        .route("/withdraw/history", get(handlers::withdraw::get_history))
        .route("/withdraw/:id", get(handlers::withdraw::get_withdrawal))
        .route("/withdraw/:id/cancel", delete(handlers::withdraw::cancel_withdraw))
        .route("/withdraw/:id/confirm", post(handlers::withdraw::confirm_withdraw))
        // Referral
        .route("/referral/codes", post(handlers::referral::create_code))
        .route("/referral/bind", post(handlers::referral::bind_code))
        .route("/referral/dashboard", get(handlers::referral::get_dashboard))
        .route("/referral/claim", post(handlers::referral::claim_earnings))
        .route("/referral/on-chain/claim-signature", post(handlers::referral::get_claim_signature))
        // Funding settlements (user-specific)
        .route("/funding/settlements", get(handlers::funding_rate::get_user_settlements))
        // Liquidation history (user-specific)
        .route("/liquidations/history", get(handlers::liquidation::get_user_liquidations))
        // ADL (user-specific)
        .route("/adl/history", get(handlers::adl::get_user_adl_history))
        .route("/adl/:symbol/stats", get(handlers::adl::get_user_adl_stats))
        .layer(axum_middleware::from_fn_with_state(state.clone(), auth_middleware));

    Router::new()
        .merge(public_routes)
        .merge(protected_routes)
}
