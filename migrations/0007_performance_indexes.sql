-- Migration 0007: Performance Indexes
-- Adds optimized indexes for high-frequency query patterns

-- =============================================================================
-- Orders Table Indexes
-- =============================================================================

-- Composite index for user's active orders (most common query)
-- SELECT * FROM orders WHERE user_address = ? AND status IN ('open', 'pending', 'partially_filled')
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_user_active
ON orders(user_address, status)
WHERE status IN ('open', 'pending', 'partially_filled');

-- Composite index for orderbook queries
-- SELECT * FROM orders WHERE symbol = ? AND status = 'open' AND side = ? ORDER BY price
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_orderbook_buy
ON orders(symbol, price DESC)
WHERE status = 'open' AND side = 'buy';

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_orderbook_sell
ON orders(symbol, price ASC)
WHERE status = 'open' AND side = 'sell';

-- Index for order history by user with time ordering
-- SELECT * FROM orders WHERE user_address = ? ORDER BY created_at DESC LIMIT N
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_orders_user_time
ON orders(user_address, created_at DESC);

-- =============================================================================
-- Trades Table Indexes
-- =============================================================================

-- Composite index for recent trades by symbol (K-line generation)
-- SELECT * FROM trades WHERE symbol = ? AND created_at BETWEEN ? AND ? ORDER BY created_at
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trades_symbol_time_range
ON trades(symbol, created_at);

-- Index for user trade history
-- SELECT * FROM trades WHERE maker_address = ? OR taker_address = ? ORDER BY created_at DESC
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trades_maker_time
ON trades(maker_address, created_at DESC);

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trades_taker_time
ON trades(taker_address, created_at DESC);

-- =============================================================================
-- Positions Table Indexes
-- =============================================================================

-- Partial index for open positions only (most queries are for open positions)
-- SELECT * FROM positions WHERE size_in_usd > 0
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_positions_open
ON positions(user_address, symbol)
WHERE size_in_usd > 0;

-- Index for liquidation checks (positions sorted by liquidation price)
-- SELECT * FROM positions WHERE symbol = ? AND size_in_usd > 0 ORDER BY liquidation_price
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_positions_liquidation_long
ON positions(symbol, liquidation_price ASC)
WHERE size_in_usd > 0 AND side = 'long';

CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_positions_liquidation_short
ON positions(symbol, liquidation_price DESC)
WHERE size_in_usd > 0 AND side = 'short';

-- =============================================================================
-- Balances Table Indexes
-- =============================================================================

-- Composite index for user balances with token
-- SELECT * FROM balances WHERE user_address = ? AND available > 0
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_balances_user_available
ON balances(user_address)
WHERE available > 0;

-- =============================================================================
-- Funding Settlements Indexes
-- =============================================================================

-- Index for recent settlements by user
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_funding_settlements_user_time
ON funding_settlements(user_address, settled_at DESC);

-- =============================================================================
-- Liquidations Indexes
-- =============================================================================

-- Index for liquidation history by user
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_liquidations_user_time
ON liquidations(user_address, liquidated_at DESC);

-- =============================================================================
-- Deposits/Withdrawals Indexes
-- =============================================================================

-- Index for pending withdrawals (frequently checked)
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_withdrawals_pending
ON withdrawals(status, created_at)
WHERE status = 'pending';

-- Index for deposits by status
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_deposits_pending
ON deposits(status)
WHERE status = 'pending';

-- =============================================================================
-- BRIN Index for Time-Series Data (trades)
-- BRIN indexes are very efficient for time-ordered data
-- =============================================================================

-- Note: BRIN index for trades table (if data is inserted chronologically)
-- This is much smaller than B-tree and efficient for range queries
-- Uncomment if using TimescaleDB or for large datasets
-- CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_trades_time_brin
-- ON trades USING BRIN(created_at) WITH (pages_per_range = 128);

-- =============================================================================
-- Statistics Update
-- =============================================================================

-- Analyze tables to update query planner statistics
ANALYZE orders;
ANALYZE trades;
ANALYZE positions;
ANALYZE balances;
ANALYZE funding_settlements;
ANALYZE liquidations;
ANALYZE withdrawals;
ANALYZE deposits;
