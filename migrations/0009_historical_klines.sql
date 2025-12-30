-- Migration 0009: Historical K-lines Table
-- Creates a table for storing imported historical K-line data directly
-- This is separate from the continuous aggregates that derive from trades

-- =============================================================================
-- Step 1: Create Historical K-lines Table
-- =============================================================================
-- This table stores K-line data that is imported from external sources
-- (e.g., Binance historical data) without needing to import all trades

CREATE TABLE IF NOT EXISTS klines_historical (
    id BIGSERIAL,
    symbol VARCHAR(20) NOT NULL,
    period VARCHAR(5) NOT NULL,  -- 1m, 5m, 15m, 1h, 4h, 1d, 1w, 1M
    open_time TIMESTAMPTZ NOT NULL,
    open DECIMAL(30, 10) NOT NULL,
    high DECIMAL(30, 10) NOT NULL,
    low DECIMAL(30, 10) NOT NULL,
    close DECIMAL(30, 10) NOT NULL,
    volume DECIMAL(30, 10) NOT NULL DEFAULT 0,
    quote_volume DECIMAL(30, 10) DEFAULT 0,
    trade_count INT DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),

    PRIMARY KEY (id, open_time)
);

-- Convert to TimescaleDB hypertable
SELECT create_hypertable(
    'klines_historical',
    'open_time',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- =============================================================================
-- Step 2: Create Unique Index for Upsert Operations
-- =============================================================================
-- Ensures we don't have duplicate candles for the same symbol/period/time

CREATE UNIQUE INDEX IF NOT EXISTS idx_klines_historical_unique
ON klines_historical (symbol, period, open_time);

-- Index for efficient queries
CREATE INDEX IF NOT EXISTS idx_klines_historical_symbol_period_time
ON klines_historical (symbol, period, open_time DESC);

-- =============================================================================
-- Step 3: Compression Policy
-- =============================================================================
-- Compress historical data older than 7 days

ALTER TABLE klines_historical SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol, period',
    timescaledb.compress_orderby = 'open_time DESC'
);

SELECT add_compression_policy('klines_historical',
    INTERVAL '7 days',
    if_not_exists => TRUE
);

-- =============================================================================
-- Step 4: Helper Function to Get Historical K-lines
-- =============================================================================

CREATE OR REPLACE FUNCTION get_historical_klines(
    p_symbol VARCHAR,
    p_period VARCHAR,
    p_start_time TIMESTAMPTZ,
    p_end_time TIMESTAMPTZ,
    p_limit INT DEFAULT 500
)
RETURNS TABLE (
    symbol VARCHAR,
    open_time TIMESTAMPTZ,
    open DECIMAL,
    high DECIMAL,
    low DECIMAL,
    close DECIMAL,
    volume DECIMAL,
    quote_volume DECIMAL,
    trade_count INT
) AS $$
BEGIN
    RETURN QUERY
    SELECT
        kh.symbol,
        kh.open_time,
        kh.open,
        kh.high,
        kh.low,
        kh.close,
        kh.volume,
        kh.quote_volume,
        kh.trade_count
    FROM klines_historical kh
    WHERE kh.symbol = p_symbol
      AND kh.period = p_period
      AND kh.open_time >= p_start_time
      AND kh.open_time < p_end_time
    ORDER BY kh.open_time DESC
    LIMIT p_limit;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- Step 5: Function to Upsert K-line Data
-- =============================================================================

CREATE OR REPLACE FUNCTION upsert_kline(
    p_symbol VARCHAR,
    p_period VARCHAR,
    p_open_time TIMESTAMPTZ,
    p_open DECIMAL,
    p_high DECIMAL,
    p_low DECIMAL,
    p_close DECIMAL,
    p_volume DECIMAL,
    p_quote_volume DECIMAL,
    p_trade_count INT
)
RETURNS VOID AS $$
BEGIN
    INSERT INTO klines_historical (
        symbol, period, open_time, open, high, low, close, volume, quote_volume, trade_count
    ) VALUES (
        p_symbol, p_period, p_open_time, p_open, p_high, p_low, p_close, p_volume, p_quote_volume, p_trade_count
    )
    ON CONFLICT (symbol, period, open_time)
    DO UPDATE SET
        open = EXCLUDED.open,
        high = EXCLUDED.high,
        low = EXCLUDED.low,
        close = EXCLUDED.close,
        volume = EXCLUDED.volume,
        quote_volume = EXCLUDED.quote_volume,
        trade_count = EXCLUDED.trade_count;
END;
$$ LANGUAGE plpgsql;
