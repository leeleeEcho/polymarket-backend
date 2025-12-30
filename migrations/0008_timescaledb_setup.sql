-- Migration 0008: TimescaleDB Setup
-- Converts trades table to hypertable and sets up continuous aggregates for K-lines
--
-- PREREQUISITES:
-- 1. TimescaleDB extension must be installed on the PostgreSQL server
-- 2. Run: CREATE EXTENSION IF NOT EXISTS timescaledb;
--
-- To install TimescaleDB on macOS:
--   brew install timescaledb
--   timescaledb-tune
--   brew services restart postgresql@14
--
-- To install on Ubuntu:
--   sudo add-apt-repository ppa:timescale/timescaledb-ppa
--   sudo apt update
--   sudo apt install timescaledb-2-postgresql-14
--   sudo timescaledb-tune
--   sudo systemctl restart postgresql

-- =============================================================================
-- Step 1: Enable TimescaleDB Extension
-- =============================================================================
CREATE EXTENSION IF NOT EXISTS timescaledb CASCADE;

-- =============================================================================
-- Step 2: Convert trades table to Hypertable
-- =============================================================================
-- Note: This requires the trades table to exist but be empty or have data.
-- If the table has existing data, TimescaleDB will chunk it automatically.

-- First, drop the existing time index if it exists (hypertable will create its own)
DROP INDEX IF EXISTS idx_trades_time;
DROP INDEX IF EXISTS idx_trades_symbol_time_range;

-- Convert trades to hypertable with time partitioning
-- Chunk interval of 1 day is good for trading data
SELECT create_hypertable(
    'trades',
    'created_at',
    chunk_time_interval => INTERVAL '1 day',
    if_not_exists => TRUE,
    migrate_data => TRUE
);

-- Add compression settings
ALTER TABLE trades SET (
    timescaledb.compress,
    timescaledb.compress_segmentby = 'symbol',
    timescaledb.compress_orderby = 'created_at DESC'
);

-- =============================================================================
-- Step 3: Create K-line (Candlestick) Tables for Continuous Aggregates
-- =============================================================================

-- 1-minute K-lines (real-time)
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_1m
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('1 minute', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('1 minute', created_at)
WITH NO DATA;

-- 5-minute K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_5m
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('5 minutes', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('5 minutes', created_at)
WITH NO DATA;

-- 15-minute K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_15m
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('15 minutes', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('15 minutes', created_at)
WITH NO DATA;

-- 1-hour K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_1h
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('1 hour', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('1 hour', created_at)
WITH NO DATA;

-- 4-hour K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_4h
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('4 hours', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('4 hours', created_at)
WITH NO DATA;

-- 1-day K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_1d
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('1 day', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('1 day', created_at)
WITH NO DATA;

-- 1-week K-lines
CREATE MATERIALIZED VIEW IF NOT EXISTS klines_1w
WITH (timescaledb.continuous) AS
SELECT
    symbol,
    time_bucket('1 week', created_at) AS bucket,
    FIRST(price, created_at) AS open,
    MAX(price) AS high,
    MIN(price) AS low,
    LAST(price, created_at) AS close,
    SUM(amount) AS volume,
    SUM(price * amount) AS quote_volume,
    COUNT(*) AS trade_count
FROM trades
GROUP BY symbol, time_bucket('1 week', created_at)
WITH NO DATA;

-- =============================================================================
-- Step 4: Set Up Continuous Aggregate Refresh Policies
-- =============================================================================

-- 1-minute: refresh every minute, covers last 2 hours
SELECT add_continuous_aggregate_policy('klines_1m',
    start_offset => INTERVAL '2 hours',
    end_offset => INTERVAL '1 minute',
    schedule_interval => INTERVAL '1 minute',
    if_not_exists => TRUE
);

-- 5-minute: refresh every 5 minutes
SELECT add_continuous_aggregate_policy('klines_5m',
    start_offset => INTERVAL '6 hours',
    end_offset => INTERVAL '5 minutes',
    schedule_interval => INTERVAL '5 minutes',
    if_not_exists => TRUE
);

-- 15-minute: refresh every 15 minutes
SELECT add_continuous_aggregate_policy('klines_15m',
    start_offset => INTERVAL '12 hours',
    end_offset => INTERVAL '15 minutes',
    schedule_interval => INTERVAL '15 minutes',
    if_not_exists => TRUE
);

-- 1-hour: refresh every hour
SELECT add_continuous_aggregate_policy('klines_1h',
    start_offset => INTERVAL '24 hours',
    end_offset => INTERVAL '1 hour',
    schedule_interval => INTERVAL '1 hour',
    if_not_exists => TRUE
);

-- 4-hour: refresh every 4 hours
SELECT add_continuous_aggregate_policy('klines_4h',
    start_offset => INTERVAL '48 hours',
    end_offset => INTERVAL '4 hours',
    schedule_interval => INTERVAL '4 hours',
    if_not_exists => TRUE
);

-- 1-day: refresh daily
SELECT add_continuous_aggregate_policy('klines_1d',
    start_offset => INTERVAL '7 days',
    end_offset => INTERVAL '1 day',
    schedule_interval => INTERVAL '1 day',
    if_not_exists => TRUE
);

-- 1-week: refresh weekly
SELECT add_continuous_aggregate_policy('klines_1w',
    start_offset => INTERVAL '4 weeks',
    end_offset => INTERVAL '1 week',
    schedule_interval => INTERVAL '1 week',
    if_not_exists => TRUE
);

-- =============================================================================
-- Step 5: Set Up Data Retention Policy
-- =============================================================================

-- Keep raw trade data for 90 days, then drop old chunks
SELECT add_retention_policy('trades',
    INTERVAL '90 days',
    if_not_exists => TRUE
);

-- Keep 1-minute K-lines for 30 days
SELECT add_retention_policy('klines_1m',
    INTERVAL '30 days',
    if_not_exists => TRUE
);

-- Keep 5-minute K-lines for 60 days
SELECT add_retention_policy('klines_5m',
    INTERVAL '60 days',
    if_not_exists => TRUE
);

-- Keep 15-minute K-lines for 90 days
SELECT add_retention_policy('klines_15m',
    INTERVAL '90 days',
    if_not_exists => TRUE
);

-- 1-hour and above K-lines are kept indefinitely (no retention policy)

-- =============================================================================
-- Step 6: Set Up Compression Policy
-- =============================================================================

-- Compress trade data older than 7 days
SELECT add_compression_policy('trades',
    INTERVAL '7 days',
    if_not_exists => TRUE
);

-- =============================================================================
-- Step 7: Create Indexes on Continuous Aggregates
-- =============================================================================

-- Indexes for efficient K-line queries
CREATE INDEX IF NOT EXISTS idx_klines_1m_symbol_bucket ON klines_1m (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_5m_symbol_bucket ON klines_5m (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_15m_symbol_bucket ON klines_15m (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_1h_symbol_bucket ON klines_1h (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_4h_symbol_bucket ON klines_4h (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_1d_symbol_bucket ON klines_1d (symbol, bucket DESC);
CREATE INDEX IF NOT EXISTS idx_klines_1w_symbol_bucket ON klines_1w (symbol, bucket DESC);

-- =============================================================================
-- Step 8: Create Helper Functions
-- =============================================================================

-- Function to get K-lines for any period
CREATE OR REPLACE FUNCTION get_klines(
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
    trade_count BIGINT
) AS $$
BEGIN
    RETURN QUERY EXECUTE format(
        'SELECT symbol, bucket as open_time, open, high, low, close, volume, quote_volume, trade_count
         FROM klines_%s
         WHERE symbol = $1
           AND bucket >= $2
           AND bucket < $3
         ORDER BY bucket DESC
         LIMIT $4',
        p_period
    ) USING p_symbol, p_start_time, p_end_time, p_limit;
END;
$$ LANGUAGE plpgsql;

-- Function to manually refresh K-lines for a specific time range
CREATE OR REPLACE FUNCTION refresh_klines(
    p_period VARCHAR,
    p_start_time TIMESTAMPTZ,
    p_end_time TIMESTAMPTZ
)
RETURNS VOID AS $$
BEGIN
    EXECUTE format(
        'CALL refresh_continuous_aggregate(''klines_%s'', $1, $2)',
        p_period
    ) USING p_start_time, p_end_time;
END;
$$ LANGUAGE plpgsql;

-- =============================================================================
-- Note: To verify the setup, run:
-- =============================================================================
-- SELECT * FROM timescaledb_information.hypertables;
-- SELECT * FROM timescaledb_information.continuous_aggregates;
-- SELECT * FROM timescaledb_information.jobs WHERE application_name LIKE '%Continuous%';
-- SELECT * FROM timescaledb_information.compression_settings;
