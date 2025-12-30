-- Phase 7: Advanced Order Types
-- Stop-Loss, Take-Profit, Trailing Stop, Conditional Orders, Time-in-Force

-- Trigger order types
CREATE TYPE trigger_order_type AS ENUM (
    'stop_loss',           -- Stop-loss order
    'take_profit',         -- Take-profit order
    'trailing_stop',       -- Trailing stop order
    'stop_limit',          -- Stop-limit order
    'take_profit_limit'    -- Take-profit limit order
);

-- Trigger condition types
CREATE TYPE trigger_condition AS ENUM (
    'price_above',         -- Trigger when mark price >= trigger price
    'price_below'          -- Trigger when mark price <= trigger price
);

-- Trigger order status
CREATE TYPE trigger_order_status AS ENUM (
    'active',              -- Waiting to be triggered
    'triggered',           -- Condition met, order placed
    'executed',            -- Order filled
    'cancelled',           -- Cancelled by user
    'expired',             -- Order expired
    'failed'               -- Failed to execute
);

-- Time-in-force types for regular orders
CREATE TYPE time_in_force AS ENUM (
    'gtc',                 -- Good Till Cancelled
    'ioc',                 -- Immediate Or Cancel
    'fok',                 -- Fill Or Kill
    'gtd'                  -- Good Till Date
);

-- Trigger Orders Table (Stop-Loss, Take-Profit, Trailing Stop)
CREATE TABLE IF NOT EXISTS trigger_orders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    position_id UUID REFERENCES positions(id) ON DELETE CASCADE,  -- Optional link to position
    market_symbol VARCHAR(20) NOT NULL,

    -- Order details
    trigger_type trigger_order_type NOT NULL,
    side order_side NOT NULL,                    -- buy or sell (to close position)
    size DECIMAL(36, 18) NOT NULL,               -- Order size

    -- Trigger conditions
    trigger_price DECIMAL(36, 18) NOT NULL,      -- Price at which to trigger
    trigger_condition trigger_condition NOT NULL, -- Above or below
    limit_price DECIMAL(36, 18),                 -- For stop-limit orders

    -- Trailing stop specific
    trailing_delta DECIMAL(36, 18),              -- Distance from peak (absolute or percentage)
    trailing_delta_type VARCHAR(10) DEFAULT 'absolute', -- 'absolute' or 'percentage'
    peak_price DECIMAL(36, 18),                  -- Tracked peak price for trailing stop

    -- Execution details
    status trigger_order_status NOT NULL DEFAULT 'active',
    triggered_at TIMESTAMPTZ,                    -- When condition was met
    triggered_price DECIMAL(36, 18),             -- Price at trigger time
    executed_order_id UUID REFERENCES orders(id), -- The actual order created
    executed_price DECIMAL(36, 18),              -- Final execution price
    executed_at TIMESTAMPTZ,

    -- Risk management
    reduce_only BOOLEAN NOT NULL DEFAULT true,   -- Only reduce position, don't flip
    close_position BOOLEAN NOT NULL DEFAULT false, -- Close entire position

    -- Expiry
    expires_at TIMESTAMPTZ,                      -- Optional expiry time

    -- Metadata
    client_order_id VARCHAR(64),                 -- Client-provided ID
    error_message TEXT,                          -- Error if failed
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_trigger_orders_user ON trigger_orders(user_address);
CREATE INDEX idx_trigger_orders_position ON trigger_orders(position_id);
CREATE INDEX idx_trigger_orders_market ON trigger_orders(market_symbol);
CREATE INDEX idx_trigger_orders_status ON trigger_orders(status);
CREATE INDEX idx_trigger_orders_active ON trigger_orders(market_symbol, status) WHERE status = 'active';

-- Position TP/SL Settings (attached directly to positions)
CREATE TABLE IF NOT EXISTS position_tp_sl (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    position_id UUID NOT NULL UNIQUE REFERENCES positions(id) ON DELETE CASCADE,
    user_address VARCHAR(42) NOT NULL,
    market_symbol VARCHAR(20) NOT NULL,

    -- Take-profit settings
    take_profit_price DECIMAL(36, 18),
    take_profit_trigger_order_id UUID REFERENCES trigger_orders(id),

    -- Stop-loss settings
    stop_loss_price DECIMAL(36, 18),
    stop_loss_trigger_order_id UUID REFERENCES trigger_orders(id),

    -- Trailing stop settings
    trailing_stop_delta DECIMAL(36, 18),
    trailing_stop_delta_type VARCHAR(10) DEFAULT 'absolute',
    trailing_stop_trigger_order_id UUID REFERENCES trigger_orders(id),

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_position_tp_sl_position ON position_tp_sl(position_id);
CREATE INDEX idx_position_tp_sl_user ON position_tp_sl(user_address);

-- Trigger Order Execution History
CREATE TABLE IF NOT EXISTS trigger_order_executions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    trigger_order_id UUID NOT NULL REFERENCES trigger_orders(id),
    user_address VARCHAR(42) NOT NULL,
    market_symbol VARCHAR(20) NOT NULL,

    -- Execution details
    trigger_type trigger_order_type NOT NULL,
    trigger_price DECIMAL(36, 18) NOT NULL,      -- Price that triggered the order
    mark_price DECIMAL(36, 18) NOT NULL,         -- Mark price at trigger time
    execution_price DECIMAL(36, 18),             -- Actual execution price
    size DECIMAL(36, 18) NOT NULL,
    side order_side NOT NULL,

    -- Result
    success BOOLEAN NOT NULL,
    error_message TEXT,
    resulting_order_id UUID REFERENCES orders(id),

    -- PnL impact
    realized_pnl DECIMAL(36, 18),

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_trigger_executions_order ON trigger_order_executions(trigger_order_id);
CREATE INDEX idx_trigger_executions_user ON trigger_order_executions(user_address);
CREATE INDEX idx_trigger_executions_market ON trigger_order_executions(market_symbol);
CREATE INDEX idx_trigger_executions_time ON trigger_order_executions(created_at);

-- Advanced Order Statistics per User
CREATE TABLE IF NOT EXISTS user_trigger_order_stats (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    market_symbol VARCHAR(20) NOT NULL,

    -- Stop-loss stats
    total_stop_loss_orders BIGINT NOT NULL DEFAULT 0,
    triggered_stop_loss_orders BIGINT NOT NULL DEFAULT 0,
    stop_loss_pnl DECIMAL(36, 18) NOT NULL DEFAULT 0,

    -- Take-profit stats
    total_take_profit_orders BIGINT NOT NULL DEFAULT 0,
    triggered_take_profit_orders BIGINT NOT NULL DEFAULT 0,
    take_profit_pnl DECIMAL(36, 18) NOT NULL DEFAULT 0,

    -- Trailing stop stats
    total_trailing_stop_orders BIGINT NOT NULL DEFAULT 0,
    triggered_trailing_stop_orders BIGINT NOT NULL DEFAULT 0,
    trailing_stop_pnl DECIMAL(36, 18) NOT NULL DEFAULT 0,

    -- Totals
    total_saved_by_sl DECIMAL(36, 18) NOT NULL DEFAULT 0, -- Estimated loss prevented
    total_captured_by_tp DECIMAL(36, 18) NOT NULL DEFAULT 0, -- Profit captured

    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    UNIQUE(user_address, market_symbol)
);

CREATE INDEX idx_user_trigger_stats_user ON user_trigger_order_stats(user_address);

-- Add time_in_force column to orders table
ALTER TABLE orders ADD COLUMN IF NOT EXISTS time_in_force time_in_force DEFAULT 'gtc';
ALTER TABLE orders ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;
ALTER TABLE orders ADD COLUMN IF NOT EXISTS client_order_id VARCHAR(64);
ALTER TABLE orders ADD COLUMN IF NOT EXISTS reduce_only BOOLEAN DEFAULT false;
ALTER TABLE orders ADD COLUMN IF NOT EXISTS post_only BOOLEAN DEFAULT false;

-- Add trigger order reference to orders
ALTER TABLE orders ADD COLUMN IF NOT EXISTS trigger_order_id UUID REFERENCES trigger_orders(id);

-- Create index for time-based order expiry
CREATE INDEX IF NOT EXISTS idx_orders_expires_at ON orders(expires_at) WHERE expires_at IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_orders_client_id ON orders(user_address, client_order_id) WHERE client_order_id IS NOT NULL;

-- Trigger Order Configuration per Market
CREATE TABLE IF NOT EXISTS trigger_order_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    market_symbol VARCHAR(20) UNIQUE NOT NULL,

    -- Limits
    max_trigger_orders_per_user INT NOT NULL DEFAULT 50,
    max_trigger_orders_per_position INT NOT NULL DEFAULT 5,

    -- Price constraints
    min_trigger_distance_pct DECIMAL(10, 4) NOT NULL DEFAULT 0.1,  -- Min distance from mark price (%)
    max_trigger_distance_pct DECIMAL(10, 4) NOT NULL DEFAULT 50.0, -- Max distance from mark price (%)

    -- Trailing stop constraints
    min_trailing_delta_pct DECIMAL(10, 4) NOT NULL DEFAULT 0.1,    -- Min trailing delta (%)
    max_trailing_delta_pct DECIMAL(10, 4) NOT NULL DEFAULT 20.0,   -- Max trailing delta (%)

    -- Execution settings
    trigger_check_interval_ms INT NOT NULL DEFAULT 100,            -- How often to check triggers
    slippage_tolerance_pct DECIMAL(10, 4) NOT NULL DEFAULT 1.0,    -- Max slippage for market orders

    enabled BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Insert default configurations for supported markets
INSERT INTO trigger_order_config (market_symbol, max_trigger_orders_per_user, min_trigger_distance_pct, max_trigger_distance_pct)
VALUES
    ('BTCUSDT', 100, 0.05, 50.0),
    ('ETHUSDT', 100, 0.05, 50.0),
    ('SOLUSDT', 100, 0.05, 50.0)
ON CONFLICT (market_symbol) DO NOTHING;

-- Apply updated_at triggers
CREATE TRIGGER update_trigger_orders_updated_at
    BEFORE UPDATE ON trigger_orders
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_position_tp_sl_updated_at
    BEFORE UPDATE ON position_tp_sl
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_user_trigger_stats_updated_at
    BEFORE UPDATE ON user_trigger_order_stats
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

CREATE TRIGGER update_trigger_config_updated_at
    BEFORE UPDATE ON trigger_order_config
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Comments
COMMENT ON TABLE trigger_orders IS 'Advanced order types: stop-loss, take-profit, trailing stop';
COMMENT ON TABLE position_tp_sl IS 'TP/SL settings attached directly to positions';
COMMENT ON TABLE trigger_order_executions IS 'History of triggered order executions';
COMMENT ON TABLE user_trigger_order_stats IS 'User statistics for trigger orders';
COMMENT ON TABLE trigger_order_config IS 'Configuration for trigger orders per market';
