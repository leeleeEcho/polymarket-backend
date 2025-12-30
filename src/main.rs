use std::net::SocketAddr;
use std::sync::Arc;

use axum::{routing::get, Router};
use serde::Serialize;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Order update event for real-time WebSocket push
#[derive(Debug, Clone, Serialize)]
pub struct OrderUpdateEvent {
    pub user_address: String,
    pub order: models::order::OrderResponse,
}

mod api;
mod auth;
mod cache;
mod config;
mod db;
mod metrics;
mod models;
mod services;
mod utils;
mod websocket;

use crate::cache::{CacheConfig, CacheManager};
use crate::config::AppConfig;
use crate::db::Database;
use crate::services::matching::MatchingEngine;
use crate::services::market::MarketService;
use metrics_exporter_prometheus::PrometheusHandle;

pub struct AppState {
    pub config: AppConfig,
    pub db: Database,
    pub cache: Arc<CacheManager>,
    pub matching_engine: Arc<MatchingEngine>,
    pub market_service: Arc<MarketService>,
    pub order_update_sender: broadcast::Sender<OrderUpdateEvent>,
    pub metrics_handle: PrometheusHandle,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "polymarket_backend=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configuration
    dotenvy::dotenv().ok();
    let config = AppConfig::load()?;

    tracing::info!("Starting Polymarket Backend v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Environment: {}", config.environment);

    // Initialize Prometheus metrics
    let metrics_handle = metrics::init_metrics();
    tracing::info!("Prometheus metrics initialized");

    // Initialize EIP-712 domain from config
    crate::auth::eip712::init_domain(config.chain_id, &config.vault_address);

    // Initialize database
    let db = Database::connect(&config.database_url).await?;
    tracing::info!("Database connected");

    // Initialize cache manager (Redis)
    let cache_config = CacheConfig::from_env();
    let cache = Arc::new(CacheManager::new(cache_config).await?);
    if cache.is_available() {
        tracing::info!("Cache manager initialized with Redis at {}", cache.config().redis_url);
    } else {
        tracing::warn!("Cache manager running without Redis (graceful degradation)");
    }

    // Initialize market service
    let market_service = Arc::new(MarketService::new());
    tracing::info!("Market service initialized");

    // Initialize matching engine
    let matching_engine = Arc::new(MatchingEngine::new());
    tracing::info!("Matching engine initialized");

    // Recover open limit orders from database
    match matching_engine.recover_orders_from_db(&db.pool).await {
        Ok(count) => {
            if count > 0 {
                tracing::info!("Recovered {} open limit orders to orderbook", count);
            } else {
                tracing::info!("No open orders to recover");
            }
        }
        Err(e) => {
            tracing::error!("Failed to recover orders from database: {}", e);
            tracing::warn!("Starting with empty orderbook");
        }
    }

    // Create order update broadcast channel for real-time WebSocket push
    let (order_update_sender, _) = broadcast::channel::<OrderUpdateEvent>(1000);
    tracing::info!("Order update broadcast channel created");

    // Build application state
    let state = Arc::new(AppState {
        config: config.clone(),
        db,
        cache,
        matching_engine,
        market_service,
        order_update_sender,
        metrics_handle,
    });

    // Start trade persistence worker
    let mut trade_receiver = state.matching_engine.subscribe_trades();
    let db_pool = state.db.pool.clone();
    tokio::spawn(async move {
        use crate::services::matching::OrderFlowOrchestrator;
        tracing::info!("Trade persistence worker started");

        while let Ok(trade_event) = trade_receiver.recv().await {
            match OrderFlowOrchestrator::persist_trade(&db_pool, &trade_event).await {
                Ok(_) => {
                    tracing::debug!(
                        "Persisted trade {} (maker: {}, taker: {})",
                        trade_event.trade_id,
                        trade_event.maker_address,
                        trade_event.taker_address
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to persist trade {}: {}",
                        trade_event.trade_id,
                        e
                    );
                }
            }
        }
        tracing::warn!("Trade persistence worker stopped");
    });
    tracing::info!("Trade persistence worker spawned");

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_endpoint))
        .nest("/api/v1", api::routes::create_router(state.clone()))
        .nest("/ws", websocket::routes::create_router(state.clone()))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    // Start server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> &'static str {
    "OK"
}

/// Prometheus metrics endpoint
async fn metrics_endpoint(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> String {
    state.metrics_handle.render()
}
