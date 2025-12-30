-- Initial database schema for Renance Trading Platform

-- Users table
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    address VARCHAR(42) UNIQUE NOT NULL,
    nonce BIGINT NOT NULL DEFAULT 1,
    referral_code VARCHAR(16) UNIQUE,
    referrer_address VARCHAR(42),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_users_address ON users(address);

-- Balances table
CREATE TABLE IF NOT EXISTS balances (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    token VARCHAR(42) NOT NULL,
    available DECIMAL(36, 18) NOT NULL DEFAULT 0,
    frozen DECIMAL(36, 18) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_address, token)
);

CREATE INDEX idx_balances_user ON balances(user_address);

-- Orders table
CREATE TYPE order_side AS ENUM ('buy', 'sell');
CREATE TYPE order_type AS ENUM ('limit', 'market');
CREATE TYPE order_status AS ENUM ('pending', 'open', 'partially_filled', 'filled', 'cancelled', 'rejected');

CREATE TABLE IF NOT EXISTS orders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    side order_side NOT NULL,
    order_type order_type NOT NULL,
    price DECIMAL(36, 18),
    amount DECIMAL(36, 18) NOT NULL,
    filled_amount DECIMAL(36, 18) NOT NULL DEFAULT 0,
    leverage INT NOT NULL DEFAULT 1,
    status order_status NOT NULL DEFAULT 'pending',
    signature TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_orders_user ON orders(user_address);
CREATE INDEX idx_orders_symbol ON orders(symbol);
CREATE INDEX idx_orders_status ON orders(status);

-- Positions table
CREATE TYPE position_side AS ENUM ('long', 'short');

CREATE TABLE IF NOT EXISTS positions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    symbol VARCHAR(20) NOT NULL,
    side position_side NOT NULL,
    size DECIMAL(36, 18) NOT NULL,
    entry_price DECIMAL(36, 18) NOT NULL,
    leverage INT NOT NULL,
    liquidation_price DECIMAL(36, 18) NOT NULL,
    margin DECIMAL(36, 18) NOT NULL,
    unrealized_pnl DECIMAL(36, 18) NOT NULL DEFAULT 0,
    realized_pnl DECIMAL(36, 18) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(user_address, symbol)
);

CREATE INDEX idx_positions_user ON positions(user_address);

-- Trades table
CREATE TABLE IF NOT EXISTS trades (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    symbol VARCHAR(20) NOT NULL,
    maker_order_id UUID NOT NULL REFERENCES orders(id),
    taker_order_id UUID NOT NULL REFERENCES orders(id),
    maker_address VARCHAR(42) NOT NULL,
    taker_address VARCHAR(42) NOT NULL,
    side order_side NOT NULL,
    price DECIMAL(36, 18) NOT NULL,
    amount DECIMAL(36, 18) NOT NULL,
    maker_fee DECIMAL(36, 18) NOT NULL,
    taker_fee DECIMAL(36, 18) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_trades_symbol ON trades(symbol);
CREATE INDEX idx_trades_maker ON trades(maker_address);
CREATE INDEX idx_trades_taker ON trades(taker_address);
CREATE INDEX idx_trades_time ON trades(created_at);

-- Deposits table
CREATE TABLE IF NOT EXISTS deposits (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    token VARCHAR(42) NOT NULL,
    amount DECIMAL(36, 18) NOT NULL,
    tx_hash VARCHAR(66) UNIQUE NOT NULL,
    block_number BIGINT NOT NULL,
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_deposits_user ON deposits(user_address);
CREATE INDEX idx_deposits_tx ON deposits(tx_hash);

-- Withdrawals table
CREATE TYPE withdrawal_status AS ENUM ('pending', 'signed', 'submitted', 'confirmed', 'failed');

CREATE TABLE IF NOT EXISTS withdrawals (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_address VARCHAR(42) NOT NULL,
    token VARCHAR(42) NOT NULL,
    amount DECIMAL(36, 18) NOT NULL,
    to_address VARCHAR(42) NOT NULL,
    nonce BIGINT NOT NULL,
    expiry BIGINT NOT NULL,
    backend_signature TEXT,
    tx_hash VARCHAR(66),
    status withdrawal_status NOT NULL DEFAULT 'pending',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_withdrawals_user ON withdrawals(user_address);

-- Referral codes table
CREATE TABLE IF NOT EXISTS referral_codes (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    owner_address VARCHAR(42) UNIQUE NOT NULL,
    code VARCHAR(16) UNIQUE NOT NULL,
    total_referrals BIGINT NOT NULL DEFAULT 0,
    total_earnings DECIMAL(36, 18) NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_referral_codes_code ON referral_codes(code);

-- Referral relations table
CREATE TABLE IF NOT EXISTS referral_relations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    referee_address VARCHAR(42) UNIQUE NOT NULL,
    referrer_address VARCHAR(42) NOT NULL,
    code VARCHAR(16) NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_referral_relations_referrer ON referral_relations(referrer_address);

-- Referral earnings table
CREATE TABLE IF NOT EXISTS referral_earnings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    referrer_address VARCHAR(42) NOT NULL,
    referee_address VARCHAR(42) NOT NULL,
    token VARCHAR(42) NOT NULL,
    amount DECIMAL(36, 18) NOT NULL,
    trade_id UUID NOT NULL REFERENCES trades(id),
    claimed BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_referral_earnings_referrer ON referral_earnings(referrer_address);
CREATE INDEX idx_referral_earnings_claimed ON referral_earnings(claimed);

-- Updated at trigger function
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ language 'plpgsql';

-- Apply trigger to tables with updated_at
CREATE TRIGGER update_users_updated_at BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
CREATE TRIGGER update_balances_updated_at BEFORE UPDATE ON balances FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
CREATE TRIGGER update_orders_updated_at BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
CREATE TRIGGER update_positions_updated_at BEFORE UPDATE ON positions FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
CREATE TRIGGER update_withdrawals_updated_at BEFORE UPDATE ON withdrawals FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
