-- Fix referral system schema issues
-- This migration adds missing fields to support referral commission tracking

-- Add missing fields to referral_codes table
ALTER TABLE referral_codes 
ADD COLUMN IF NOT EXISTS tier INTEGER NOT NULL DEFAULT 1,
ADD COLUMN IF NOT EXISTS commission_rate DECIMAL(5, 4) NOT NULL DEFAULT 0.10;

-- Drop and recreate referral_earnings with correct schema
DROP TABLE IF EXISTS referral_earnings CASCADE;

CREATE TABLE referral_earnings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    referrer_address VARCHAR(42) NOT NULL,
    referee_address VARCHAR(42) NOT NULL,
    trade_id UUID NOT NULL,
    event_type VARCHAR(20) NOT NULL DEFAULT 'trade',
    volume DECIMAL(36, 18) NOT NULL DEFAULT 0,
    commission DECIMAL(36, 18) NOT NULL,
    token VARCHAR(42) NOT NULL DEFAULT 'USDT',
    status VARCHAR(20) NOT NULL DEFAULT 'pending',
    claimed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_referral_earnings_referrer ON referral_earnings(referrer_address);
CREATE INDEX idx_referral_earnings_referee ON referral_earnings(referee_address);
CREATE INDEX idx_referral_earnings_status ON referral_earnings(status);
CREATE INDEX idx_referral_earnings_trade ON referral_earnings(trade_id);

-- Add on_chain_synced field to trades table for ReferralRebate contract sync
ALTER TABLE trades
ADD COLUMN IF NOT EXISTS on_chain_synced BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_trades_on_chain_synced ON trades(on_chain_synced);

-- Update existing referral codes to have proper tier based on referral count
UPDATE referral_codes 
SET tier = CASE 
    WHEN total_referrals >= 100 THEN 4
    WHEN total_referrals >= 50 THEN 3
    WHEN total_referrals >= 10 THEN 2
    ELSE 1
END,
commission_rate = CASE 
    WHEN total_referrals >= 100 THEN 0.25
    WHEN total_referrals >= 50 THEN 0.20
    WHEN total_referrals >= 10 THEN 0.15
    ELSE 0.10
END;

COMMENT ON TABLE referral_earnings IS 'Tracks commission earnings from referral trading activity';
COMMENT ON COLUMN referral_earnings.event_type IS 'Type of event: trade, deposit, etc.';
COMMENT ON COLUMN referral_earnings.volume IS 'Trading volume that generated this commission';
COMMENT ON COLUMN referral_earnings.commission IS 'Commission amount earned';
COMMENT ON COLUMN referral_earnings.status IS 'pending, claimed, or cancelled';

