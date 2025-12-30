-- Migration: Funding Rate System
-- This migration adds tables for tracking funding rates and settlement history

-- 1. Create funding_rates table to store current and historical funding rates per market
CREATE TABLE funding_rates (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol VARCHAR(32) NOT NULL,
    funding_rate DECIMAL(36, 18) NOT NULL,
    funding_rate_per_hour DECIMAL(36, 18) NOT NULL,
    mark_price DECIMAL(36, 18) NOT NULL,
    index_price DECIMAL(36, 18) NOT NULL,
    next_funding_time TIMESTAMPTZ NOT NULL,
    settled_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 2. Create funding_settlements table to track individual position settlements
CREATE TABLE funding_settlements (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    position_id UUID NOT NULL REFERENCES positions(id),
    user_address VARCHAR(66) NOT NULL,
    symbol VARCHAR(32) NOT NULL,
    funding_rate DECIMAL(36, 18) NOT NULL,
    position_size DECIMAL(36, 18) NOT NULL,
    funding_fee DECIMAL(36, 18) NOT NULL,
    is_long BOOLEAN NOT NULL,
    settled_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 3. Create market_funding_config table for per-market funding rate configuration
CREATE TABLE market_funding_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol VARCHAR(32) NOT NULL UNIQUE,
    funding_interval_hours INTEGER NOT NULL DEFAULT 8,
    max_funding_rate DECIMAL(36, 18) NOT NULL DEFAULT 0.01,
    min_funding_rate DECIMAL(36, 18) NOT NULL DEFAULT -0.01,
    impact_pool_size DECIMAL(36, 18) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- 4. Add indexes for efficient querying
CREATE INDEX idx_funding_rates_symbol ON funding_rates(symbol);
CREATE INDEX idx_funding_rates_symbol_time ON funding_rates(symbol, created_at DESC);
CREATE INDEX idx_funding_rates_next_funding ON funding_rates(next_funding_time);

CREATE INDEX idx_funding_settlements_position ON funding_settlements(position_id);
CREATE INDEX idx_funding_settlements_user ON funding_settlements(user_address);
CREATE INDEX idx_funding_settlements_symbol_time ON funding_settlements(symbol, settled_at DESC);

-- 5. Insert default funding config for supported markets
INSERT INTO market_funding_config (symbol, funding_interval_hours, max_funding_rate, min_funding_rate)
VALUES
    ('BTCUSDT', 8, 0.01, -0.01),
    ('ETHUSDT', 8, 0.01, -0.01),
    ('SOLUSDT', 8, 0.01, -0.01);

-- Note: Funding rate calculation formula (GMX V2 style):
-- fundingRate = clamp((longOpenInterest - shortOpenInterest) / totalOpenInterest * fundingFactor, min, max)
--
-- For each position:
-- fundingFee = position.sizeInUsd * fundingRate * (timeElapsed / fundingInterval)
-- Long positions pay when rate is positive (longs > shorts)
-- Short positions pay when rate is negative (shorts > longs)
