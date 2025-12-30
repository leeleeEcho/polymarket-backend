-- Add category field to markets table
-- Used for filtering and organizing markets by topic

-- Add category column with default value
ALTER TABLE markets
ADD COLUMN IF NOT EXISTS category VARCHAR(50) NOT NULL DEFAULT 'general';

-- Add volume tracking columns
ALTER TABLE markets
ADD COLUMN IF NOT EXISTS volume_24h NUMERIC(20, 8) NOT NULL DEFAULT 0;

ALTER TABLE markets
ADD COLUMN IF NOT EXISTS total_volume NUMERIC(20, 8) NOT NULL DEFAULT 0;

-- Add probability column to outcomes
ALTER TABLE outcomes
ADD COLUMN IF NOT EXISTS probability NUMERIC(10, 8) NOT NULL DEFAULT 0.5;

-- Create index for category filtering
CREATE INDEX IF NOT EXISTS idx_markets_category ON markets(category);

-- Update common categories
COMMENT ON COLUMN markets.category IS 'Market category: crypto, politics, sports, entertainment, science, general';
COMMENT ON COLUMN markets.volume_24h IS 'Trading volume in last 24 hours (USDC)';
COMMENT ON COLUMN markets.total_volume IS 'Total all-time trading volume (USDC)';
COMMENT ON COLUMN outcomes.probability IS 'Current implied probability (0.0 to 1.0)';
