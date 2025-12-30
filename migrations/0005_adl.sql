-- Phase 6: ADL (Auto-Deleveraging) System
-- GMX V2-style ADL for handling extreme market conditions

-- ADL events table - records all auto-deleveraging executions
CREATE TABLE IF NOT EXISTS adl_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    market_symbol VARCHAR(20) NOT NULL,

    -- Triggering liquidation info
    liquidation_id UUID NOT NULL REFERENCES liquidations(id),
    insurance_fund_shortfall DECIMAL(30, 18) NOT NULL,  -- Amount insurance fund couldn't cover

    -- ADL execution details
    total_reduced_size DECIMAL(30, 18) NOT NULL,       -- Total position size reduced
    total_pnl_realized DECIMAL(30, 18) NOT NULL,       -- Total PnL taken from profitable traders
    positions_affected INT NOT NULL,                    -- Number of positions affected

    -- Status
    status VARCHAR(20) NOT NULL DEFAULT 'completed',    -- pending, completed, failed
    error_message TEXT,

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ
);

-- ADL position reductions - individual position impacts from ADL
CREATE TABLE IF NOT EXISTS adl_reductions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    adl_event_id UUID NOT NULL REFERENCES adl_events(id),

    -- Position info
    position_id UUID NOT NULL,
    user_address VARCHAR(42) NOT NULL,
    market_symbol VARCHAR(20) NOT NULL,

    -- Position state before ADL
    original_size DECIMAL(30, 18) NOT NULL,
    original_collateral DECIMAL(30, 18) NOT NULL,
    original_pnl DECIMAL(30, 18) NOT NULL,

    -- Reduction details
    size_reduced DECIMAL(30, 18) NOT NULL,             -- How much position was reduced
    pnl_realized DECIMAL(30, 18) NOT NULL,             -- PnL forcibly realized

    -- Ranking info
    adl_rank INT NOT NULL,                              -- Position in ADL queue (1 = first to be reduced)
    adl_score DECIMAL(30, 18) NOT NULL,                 -- Score used for ranking

    -- Compensation (if any)
    compensation_amount DECIMAL(30, 18) DEFAULT 0,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- ADL ranking cache - pre-computed rankings for fast ADL execution
CREATE TABLE IF NOT EXISTS adl_rankings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    market_symbol VARCHAR(20) NOT NULL,
    side VARCHAR(5) NOT NULL,                          -- 'long' or 'short'

    -- Position reference
    position_id UUID NOT NULL,
    user_address VARCHAR(42) NOT NULL,

    -- Ranking factors
    position_size DECIMAL(30, 18) NOT NULL,
    unrealized_pnl DECIMAL(30, 18) NOT NULL,
    pnl_percentage DECIMAL(30, 18) NOT NULL,           -- PnL as percentage of collateral
    leverage DECIMAL(10, 4) NOT NULL,

    -- Composite score (higher = first to be reduced)
    adl_score DECIMAL(30, 18) NOT NULL,
    rank INT NOT NULL,

    -- Timestamps
    computed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(market_symbol, side, position_id)
);

-- ADL configuration per market
CREATE TABLE IF NOT EXISTS adl_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    market_symbol VARCHAR(20) NOT NULL UNIQUE,

    -- ADL trigger thresholds
    insurance_fund_threshold DECIMAL(30, 18) NOT NULL DEFAULT 0,  -- Trigger when fund below this
    max_positions_per_adl INT NOT NULL DEFAULT 100,               -- Max positions to reduce per ADL event

    -- Reduction parameters
    min_reduction_percentage DECIMAL(10, 4) NOT NULL DEFAULT 0.10, -- Min 10% position reduction
    max_reduction_percentage DECIMAL(10, 4) NOT NULL DEFAULT 1.00, -- Max 100% (full close)

    -- Score weights for ranking
    pnl_weight DECIMAL(10, 4) NOT NULL DEFAULT 0.5,               -- Weight for PnL percentage
    leverage_weight DECIMAL(10, 4) NOT NULL DEFAULT 0.3,          -- Weight for leverage
    size_weight DECIMAL(10, 4) NOT NULL DEFAULT 0.2,              -- Weight for position size

    -- Cooldown
    min_interval_seconds INT NOT NULL DEFAULT 60,                  -- Min time between ADL events

    -- Status
    enabled BOOLEAN NOT NULL DEFAULT true,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- User ADL statistics
CREATE TABLE IF NOT EXISTS user_adl_stats (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    market_symbol VARCHAR(20) NOT NULL,

    -- Cumulative stats
    total_adl_events INT NOT NULL DEFAULT 0,
    total_size_reduced DECIMAL(30, 18) NOT NULL DEFAULT 0,
    total_pnl_realized DECIMAL(30, 18) NOT NULL DEFAULT 0,
    total_compensation DECIMAL(30, 18) NOT NULL DEFAULT 0,

    -- Last ADL
    last_adl_at TIMESTAMPTZ,

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(user_address, market_symbol)
);

-- Indexes for performance
CREATE INDEX IF NOT EXISTS idx_adl_events_market ON adl_events(market_symbol);
CREATE INDEX IF NOT EXISTS idx_adl_events_status ON adl_events(status);
CREATE INDEX IF NOT EXISTS idx_adl_events_created ON adl_events(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_adl_reductions_event ON adl_reductions(adl_event_id);
CREATE INDEX IF NOT EXISTS idx_adl_reductions_user ON adl_reductions(user_address);
CREATE INDEX IF NOT EXISTS idx_adl_reductions_position ON adl_reductions(position_id);

CREATE INDEX IF NOT EXISTS idx_adl_rankings_market_side ON adl_rankings(market_symbol, side);
CREATE INDEX IF NOT EXISTS idx_adl_rankings_score ON adl_rankings(market_symbol, side, adl_score DESC);
CREATE INDEX IF NOT EXISTS idx_adl_rankings_computed ON adl_rankings(computed_at);

CREATE INDEX IF NOT EXISTS idx_user_adl_stats_user ON user_adl_stats(user_address);
CREATE INDEX IF NOT EXISTS idx_user_adl_stats_market ON user_adl_stats(market_symbol);

-- Insert default ADL configs for supported markets
INSERT INTO adl_config (market_symbol, insurance_fund_threshold, max_positions_per_adl)
VALUES
    ('BTCUSDT', 0, 100),
    ('ETHUSDT', 0, 100),
    ('SOLUSDT', 0, 100)
ON CONFLICT (market_symbol) DO NOTHING;
