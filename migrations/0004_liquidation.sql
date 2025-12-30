-- Migration 0004: Liquidation Engine
-- Creates tables for liquidation records and insurance fund

-- Liquidation records table
CREATE TABLE IF NOT EXISTS liquidations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    position_id UUID NOT NULL REFERENCES positions(id),
    user_address VARCHAR(66) NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    side VARCHAR(10) NOT NULL,

    -- Position info at liquidation
    position_size_usd DECIMAL(38, 18) NOT NULL,
    position_size_tokens DECIMAL(38, 18) NOT NULL,
    collateral_amount DECIMAL(38, 18) NOT NULL,
    entry_price DECIMAL(38, 18) NOT NULL,
    liquidation_price DECIMAL(38, 18) NOT NULL,
    mark_price DECIMAL(38, 18) NOT NULL,

    -- Liquidation results
    remaining_collateral DECIMAL(38, 18) NOT NULL,
    liquidation_fee DECIMAL(38, 18) NOT NULL DEFAULT 0,
    insurance_fund_contribution DECIMAL(38, 18) NOT NULL DEFAULT 0,
    pnl DECIMAL(38, 18) NOT NULL,

    -- Liquidator info (optional - for keeper-based liquidation)
    liquidator_address VARCHAR(66),
    liquidator_reward DECIMAL(38, 18) DEFAULT 0,

    -- Timestamps
    liquidated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create indexes for liquidations
CREATE INDEX idx_liquidations_user ON liquidations(user_address);
CREATE INDEX idx_liquidations_symbol ON liquidations(symbol);
CREATE INDEX idx_liquidations_liquidated_at ON liquidations(liquidated_at);
CREATE INDEX idx_liquidations_position ON liquidations(position_id);

-- Insurance fund table
CREATE TABLE IF NOT EXISTS insurance_fund (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol VARCHAR(20) NOT NULL UNIQUE,
    balance DECIMAL(38, 18) NOT NULL DEFAULT 0,
    total_contributions DECIMAL(38, 18) NOT NULL DEFAULT 0,
    total_payouts DECIMAL(38, 18) NOT NULL DEFAULT 0,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Initialize insurance fund for supported markets
INSERT INTO insurance_fund (symbol, balance) VALUES
    ('BTCUSDT', 0),
    ('ETHUSDT', 0),
    ('SOLUSDT', 0)
ON CONFLICT (symbol) DO NOTHING;

-- Insurance fund transactions log
CREATE TABLE IF NOT EXISTS insurance_fund_transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol VARCHAR(20) NOT NULL,
    transaction_type VARCHAR(20) NOT NULL, -- 'contribution', 'payout', 'deposit', 'withdrawal'
    amount DECIMAL(38, 18) NOT NULL,
    balance_after DECIMAL(38, 18) NOT NULL,

    -- Related records
    liquidation_id UUID REFERENCES liquidations(id),
    position_id UUID REFERENCES positions(id),

    -- Timestamps
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_insurance_fund_tx_symbol ON insurance_fund_transactions(symbol);
CREATE INDEX idx_insurance_fund_tx_type ON insurance_fund_transactions(transaction_type);
CREATE INDEX idx_insurance_fund_tx_created ON insurance_fund_transactions(created_at);

-- Liquidation config per market
CREATE TABLE IF NOT EXISTS liquidation_config (
    symbol VARCHAR(20) PRIMARY KEY,

    -- Liquidation parameters
    liquidation_fee_rate DECIMAL(18, 8) NOT NULL DEFAULT 0.005,  -- 0.5% liquidation fee
    max_leverage INT NOT NULL DEFAULT 50,
    maintenance_margin_rate DECIMAL(18, 8) NOT NULL DEFAULT 0.005,  -- 0.5%
    min_collateral_usd DECIMAL(38, 18) NOT NULL DEFAULT 10,  -- $10 minimum

    -- Insurance fund parameters
    insurance_fund_fee_rate DECIMAL(18, 8) NOT NULL DEFAULT 0.001,  -- 0.1% to insurance fund
    max_insurance_payout_rate DECIMAL(18, 8) NOT NULL DEFAULT 0.5,  -- max 50% of position can be covered

    -- Liquidation keeper parameters
    liquidator_reward_rate DECIMAL(18, 8) NOT NULL DEFAULT 0.001,  -- 0.1% reward to liquidator

    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Initialize liquidation config for supported markets
INSERT INTO liquidation_config (symbol) VALUES
    ('BTCUSDT'),
    ('ETHUSDT'),
    ('SOLUSDT')
ON CONFLICT (symbol) DO NOTHING;
