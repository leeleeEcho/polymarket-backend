use std::net::SocketAddr;
use std::str::FromStr;
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
mod models;
mod services;
mod utils;
mod websocket;

use crate::cache::{CacheConfig, CacheManager};
use crate::config::AppConfig;
use crate::db::Database;
use crate::services::blockchain::BlockchainService;
use crate::services::funding_rate::FundingRateService;
use crate::services::liquidation::LiquidationService;
use crate::services::adl::AdlService;
use crate::services::matching::MatchingEngine;
use crate::services::position::PositionService;
use crate::services::withdraw::WithdrawService;
use crate::services::price_feed::{PriceFeedConfig, PriceFeedService};
use crate::services::trigger_orders::TriggerOrdersService;
use crate::services::referral::ReferralService;
use crate::services::keeper::KeeperService;
use crate::services::kline::KlineService;
use crate::services::auto_market_maker::{AutoMarketMakerService, AutoMarketMakerConfig};

pub struct AppState {
    pub config: AppConfig,
    pub db: Database,
    pub cache: Arc<CacheManager>,
    pub matching_engine: Arc<MatchingEngine>,
    pub withdraw_service: Arc<WithdrawService>,
    pub price_feed_service: Arc<PriceFeedService>,
    pub position_service: Arc<PositionService>,
    pub funding_rate_service: Arc<FundingRateService>,
    pub liquidation_service: Arc<LiquidationService>,
    pub adl_service: Arc<AdlService>,
    pub trigger_orders_service: Arc<TriggerOrdersService>,
    pub referral_service: Arc<ReferralService>,
    pub kline_service: Arc<KlineService>,
    pub auto_market_maker: Arc<AutoMarketMakerService>,
    pub order_update_sender: broadcast::Sender<OrderUpdateEvent>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ztdx_backend=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load configuration
    dotenvy::dotenv().ok();
    let config = AppConfig::load()?;

    tracing::info!("Starting ZTDX Backend v{}", env!("CARGO_PKG_VERSION"));
    tracing::info!("Environment: {}", config.environment);

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

    // Initialize matching engine with configured trading pairs
    let trading_pairs = config.get_trading_pairs();
    let matching_engine = Arc::new(MatchingEngine::with_symbols(trading_pairs.clone()));
    tracing::info!("Matching engine initialized for {:?}", trading_pairs);

    // Recover open limit orders from database
    match matching_engine.recover_orders_from_db(&db.pool).await {
        Ok(count) => {
            if count > 0 {
                tracing::info!("✅ Recovered {} open limit orders to orderbook", count);
            } else {
                tracing::info!("No open orders to recover");
            }
        }
        Err(e) => {
            tracing::error!("Failed to recover orders from database: {}", e);
            tracing::warn!("Starting with empty orderbook");
        }
    }

    // Initialize withdraw service
    let withdraw_service = Arc::new(WithdrawService::new(
        &config.backend_signer_private_key,
        &config.vault_address,
        config.chain_id,
        db.pool.clone(),
        &config.collateral_token_symbol,
        &config.collateral_token_address,
        config.collateral_token_decimals,
        &config.rpc_url,
    )?);
    tracing::info!("Withdraw service initialized (token: {} @ {}, decimals: {})",
        config.collateral_token_symbol, config.collateral_token_address, config.collateral_token_decimals);

    // Initialize blockchain service and start event listener
    let blockchain_service = Arc::new(BlockchainService::new(
        &config.rpc_url,
        &config.vault_address,
        db.pool.clone(),
        config.chain_id,
        config.collateral_token_decimals,
        config.block_sync_lookback,
    ).await?);
    tracing::info!(
        "Blockchain service initialized (token decimals: {}, lookback: {} blocks)",
        config.collateral_token_decimals, config.block_sync_lookback
    );

    // Start blockchain event listener in background
    let blockchain_service_clone = blockchain_service.clone();
    tokio::spawn(async move {
        tracing::info!("Starting blockchain event listener...");
        blockchain_service_clone.start_event_listener().await;
    });

    // Initialize price feed service with config
    // NOTE: Binance REST API fetching is DISABLED
    // All market data now comes from internal market maker service via API:
    // - /internal/trade - for price/ticker updates
    // - /internal/orderbook - for orderbook data (stored in Redis)
    let price_feed_config = PriceFeedConfig {
        top_markets: config.price_feed_top_markets,
        update_interval_secs: config.price_feed_update_interval_secs,
        market_refresh_secs: config.price_feed_market_refresh_secs,
    };
    let price_feed_service = Arc::new(PriceFeedService::with_config(price_feed_config));
    tracing::info!("Price feed service initialized");

    // Initialize price feed with configured trading pairs
    price_feed_service.init_symbols(trading_pairs.clone()).await;
    
    // Start background task to update prices from internal trades
    // Subscribe to matching engine trade events and feed prices to price_feed_service
    let price_feed_for_trades = price_feed_service.clone();
    let mut trade_receiver = matching_engine.subscribe_trades();
    
    tokio::spawn(async move {
        tracing::info!("📊 Started price feed update loop from internal trades");
        
        loop {
            match trade_receiver.recv().await {
                Ok(trade_event) => {
                    // Update price from trade
                    price_feed_for_trades
                        .update_price_from_trade(&trade_event.symbol, trade_event.price, trade_event.amount)
                        .await;
                    
                    tracing::debug!(
                        "📈 Price updated from trade: {}@{} (amount: {})",
                        trade_event.symbol,
                        trade_event.price,
                        trade_event.amount
                    );
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!("⚠️  Price feed lagged {} trade events", n);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    tracing::error!("❌ Trade event channel closed, price feed will stop updating");
                    break;
                }
            }
        }
    });
    
    tracing::info!("Price feed: Using internal trade data (prices updated from each trade)");

    // Initialize position service with config
    let position_config = crate::models::position::PositionConfig {
        min_collateral_usd: rust_decimal::Decimal::from_str(&config.min_collateral_usd)
            .unwrap_or(rust_decimal::Decimal::new(10, 0)),
        min_position_size_usd: rust_decimal::Decimal::from_str(&config.min_position_size_usd)
            .unwrap_or(rust_decimal::Decimal::new(100, 0)),
        max_leverage: config.max_leverage,
        maintenance_margin_rate: rust_decimal::Decimal::from_str(&config.maintenance_margin_rate)
            .unwrap_or(rust_decimal::Decimal::new(5, 3)),
        position_fee_rate: rust_decimal::Decimal::from_str(&config.position_fee_rate)
            .unwrap_or(rust_decimal::Decimal::new(1, 3)),
        borrowing_fee_rate_per_hour: rust_decimal::Decimal::new(1, 5), // 0.001% per hour
    };
    let position_service = Arc::new(PositionService::with_config(db.pool.clone(), position_config.clone()));
    tracing::info!("Position service initialized with min_position_size_usd: {}", position_config.min_position_size_usd);

    // Initialize funding rate service
    let funding_rate_service = Arc::new(FundingRateService::new(db.pool.clone()));
    tracing::info!("Funding rate service initialized");

    // Define supported markets for funding rate tracking (from config)
    let supported_markets = trading_pairs.clone();

    // Start funding rate update loop in background (updates every minute)
    let funding_rate_clone = funding_rate_service.clone();
    let markets_clone = supported_markets.clone();
    funding_rate_clone.start_update_loop(markets_clone).await;
    tracing::info!("Funding rate update loop started");

    // Start funding rate settlement scheduler (settles every 8 hours)
    let funding_rate_settlement = funding_rate_service.clone();
    let markets_for_settlement = supported_markets.clone();
    funding_rate_settlement.start_settlement_scheduler(markets_for_settlement).await;
    tracing::info!("Funding rate settlement scheduler started");

    // Initialize liquidation service
    let liquidation_service = Arc::new(LiquidationService::new(
        db.pool.clone(),
        position_service.clone(),
        price_feed_service.clone(),
    ));
    tracing::info!("Liquidation service initialized");

    // [DISABLED] Start liquidation engine loop (checks every 5 seconds)
    // Liquidation disabled for testing position persistence
    // let liquidation_service_clone = liquidation_service.clone();
    // let markets_for_liquidation = supported_markets.clone();
    // liquidation_service_clone.start_liquidation_loop(markets_for_liquidation).await;
    tracing::info!("Liquidation engine DISABLED");

    // Initialize ADL service
    let adl_service = Arc::new(AdlService::new(
        db.pool.clone(),
        position_service.clone(),
        price_feed_service.clone(),
    ));
    tracing::info!("ADL service initialized");

    // Start ADL ranking update loop (updates rankings every 30 seconds)
    let adl_service_clone = adl_service.clone();
    let markets_for_adl = supported_markets.clone();
    adl_service_clone.start_ranking_update_loop(markets_for_adl).await;
    tracing::info!("ADL ranking update loop started");

    // Initialize trigger orders service
    let trigger_orders_service = Arc::new(TriggerOrdersService::new(
        db.pool.clone(),
        price_feed_service.clone(),
    ));
    tracing::info!("Trigger orders service initialized");

    // Initialize and start Keeper service (replaces trigger orders monitoring loop)
    // The Keeper service handles automated trigger order execution via the matching engine
    let keeper_service = Arc::new(KeeperService::new(
        db.pool.clone(),
        matching_engine.clone(),
        price_feed_service.clone(),
    ));
    let markets_for_keeper = supported_markets.clone();
    keeper_service.start(markets_for_keeper).await;
    tracing::info!("Keeper service started - monitoring trigger orders");

    // Initialize referral service with ReferralRebate and ReferralStorage contracts
    let referral_service = Arc::new(
        ReferralService::with_contracts(
            db.pool.clone(),
            &config.rpc_url,
            &config.referral_rebate_address,
            &config.referral_storage_address,
            &config.backend_signer_private_key,
            config.chain_id,
        )
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to initialize ReferralService with contracts: {}. Using database-only mode.", e);
            ReferralService::new(db.pool.clone())
        })
    );
    tracing::info!(
        "Referral service initialized: rebate={}, storage={}",
        config.referral_rebate_address,
        config.referral_storage_address
    );

    // Start referral batch sync loop (syncs trades to ReferralRebate contract every hour)
    let referral_service_clone = referral_service.clone();
    referral_service_clone.start_batch_sync_loop().await;
    tracing::info!("Referral batch sync loop started");

    // Initialize K-line service
    let kline_service = KlineService::new(Some(db.pool.clone()));
    tracing::info!("K-line service initialized");

    // [DISABLED] Mock K-line data generation
    // All K-line data now comes from internal matching engine trades via market maker
    //
    // // Generate mock K-line data for development (300 candles per period)
    // // This ensures frontend has chart data to display
    // kline_service.generate_mock_data("BTCUSDT", 300).await;
    // kline_service.generate_mock_data("ETHUSDT", 300).await;
    // tracing::info!("Mock K-line data generated for BTCUSDT, ETHUSDT");

    // [DISABLED] Binance K-line WebSocket listener
    // All K-line data now generated from internal trades
    //
    // // Start K-line Binance listener in background
    // let kline_service_clone = kline_service.clone();
    // let symbols = vec!["BTCUSDT".to_string(), "ETHUSDT".to_string()]; // Hardcoded for now, or use config
    // tokio::spawn(async move {
    //     kline_service_clone.start_binance_listener(symbols).await;
    // });
    // tracing::info!("K-line Binance listener started");
    tracing::info!("K-line: Using internal trade data (mock data and Binance feed disabled)");

    // Initialize auto market maker service
    let auto_mm_config = AutoMarketMakerConfig {
        test_account_address: config.auto_mm_test_account.clone(),
        test_account_private_key: config.auto_mm_test_private_key.clone(),
        enabled: config.auto_mm_enabled,
        max_fill_size: rust_decimal::Decimal::from_str(&config.auto_mm_max_fill_size)
            .unwrap_or(rust_decimal::Decimal::from(10)),
        slippage_tolerance: rust_decimal::Decimal::from_str(&config.auto_mm_slippage)
            .unwrap_or(rust_decimal::Decimal::new(1, 3)),
        update_interval_secs: 5,
    };
    
    let auto_market_maker = Arc::new(AutoMarketMakerService::new(
        auto_mm_config.clone(),
        matching_engine.clone(),
        db.pool.clone(),
        Some(price_feed_service.clone())
    ));
    
    // Start auto market maker price update loop
    if auto_mm_config.enabled {
        tracing::info!("Starting auto market maker price update loop...");
        auto_market_maker.clone().start_price_update_loop().await;
        tracing::info!("Auto market maker enabled with test account: {}", auto_mm_config.test_account_address);
    } else {
        tracing::info!("Auto market maker is disabled");
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
        withdraw_service,
        price_feed_service,
        position_service,
        funding_rate_service,
        liquidation_service,
        adl_service,
        trigger_orders_service,
        referral_service,
        kline_service,
        auto_market_maker,
        order_update_sender,
    });

    // Start trade persistence worker
    // This worker listens to trade events from the matching engine and persists them to database
    let mut trade_receiver = state.matching_engine.subscribe_trades();
    let db_pool = state.db.pool.clone();
    tokio::spawn(async move {
        use crate::services::matching::OrderFlowOrchestrator;
        tracing::info!("Trade persistence worker started");

        while let Ok(trade_event) = trade_receiver.recv().await {
            match OrderFlowOrchestrator::persist_trade(&db_pool, &trade_event).await {
                Ok(_) => {
                    tracing::debug!(
                        "Persisted trade {} for {} (maker: {}, taker: {})",
                        trade_event.trade_id,
                        trade_event.symbol,
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

    // Start K-line update worker
    // This worker listens to trade events and updates K-line data in real-time
    let mut kline_trade_receiver = state.matching_engine.subscribe_trades();
    let kline_service_clone = state.kline_service.clone();
    tokio::spawn(async move {
        tracing::info!("K-line update worker started");

        while let Ok(trade_event) = kline_trade_receiver.recv().await {
            kline_service_clone.process_trade(&trade_event).await;
            tracing::debug!(
                "Updated K-lines for {} trade: price={}, amount={}",
                trade_event.symbol,
                trade_event.price,
                trade_event.amount
            );
        }
        tracing::warn!("K-line update worker stopped");
    });
    tracing::info!("K-line update worker spawned - real-time K-line generation enabled");

    // Start Redis pub/sub worker
    // This worker listens to trade events and publishes them to Redis for external consumers
    if state.cache.is_available() {
        let mut redis_trade_receiver = state.matching_engine.subscribe_trades();
        let cache_clone = state.cache.clone();
        tokio::spawn(async move {
            tracing::info!("Redis pub/sub worker started");

            while let Ok(trade_event) = redis_trade_receiver.recv().await {
                // Publish trade to Redis pub/sub channel
                if let Some(pubsub) = cache_clone.pubsub_opt() {
                    let publisher = pubsub.publisher();
                    match publisher.publish_trade(&trade_event.symbol, &trade_event).await {
                        Ok(n) => {
                            tracing::debug!(
                                "Published trade to Redis ({} subscribers): symbol={}, price={}, amount={}",
                                n,
                                trade_event.symbol,
                                trade_event.price,
                                trade_event.amount
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to publish trade to Redis: {} (symbol={}, price={})",
                                e,
                                trade_event.symbol,
                                trade_event.price
                            );
                        }
                    }
                }
            }
            tracing::warn!("Redis pub/sub worker stopped");
        });
        tracing::info!("Redis pub/sub worker spawned - trade events will be published to Redis");
    } else {
        tracing::warn!("Redis pub/sub worker not started - Redis is unavailable");
    }

    // Start Redis orderbook pub/sub worker
    // This worker listens to orderbook events and publishes them to Redis
    if state.cache.is_available() {
        let mut redis_orderbook_receiver = state.matching_engine.subscribe_orderbook();
        let cache_clone = state.cache.clone();
        tokio::spawn(async move {
            tracing::info!("Redis orderbook pub/sub worker started");

            while let Ok(orderbook_update) = redis_orderbook_receiver.recv().await {
                // Publish orderbook update to Redis pub/sub channel
                if let Some(pubsub) = cache_clone.pubsub_opt() {
                    let publisher = pubsub.publisher();
                    match publisher.publish_orderbook(&orderbook_update.symbol, &orderbook_update).await {
                        Ok(n) => {
                            tracing::debug!(
                                "Published orderbook to Redis ({} subscribers): symbol={}, bids={}, asks={}",
                                n,
                                orderbook_update.symbol,
                                orderbook_update.bids.len(),
                                orderbook_update.asks.len()
                            );
                        }
                        Err(e) => {
                            tracing::warn!(
                                "Failed to publish orderbook to Redis: {} (symbol={})",
                                e,
                                orderbook_update.symbol
                            );
                        }
                    }
                }
            }
            tracing::warn!("Redis orderbook pub/sub worker stopped");
        });
        tracing::info!("Redis orderbook pub/sub worker spawned - orderbook updates will be published to Redis");
    } else {
        tracing::warn!("Redis orderbook pub/sub worker not started - Redis is unavailable");
    }

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
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
