-- Migration: GMX V2-style Position Management
-- This migration updates the positions table to support GMX-style position tracking

-- 1. Create position_status enum
CREATE TYPE position_status AS ENUM ('open', 'closed', 'liquidated');

-- 2. Rename existing columns to match new model
ALTER TABLE positions RENAME COLUMN size TO size_in_usd;
ALTER TABLE positions RENAME COLUMN margin TO collateral_amount;

-- 3. Add new GMX-style columns
ALTER TABLE positions ADD COLUMN size_in_tokens DECIMAL(36, 18) NOT NULL DEFAULT 0;
ALTER TABLE positions ADD COLUMN borrowing_factor DECIMAL(36, 18) NOT NULL DEFAULT 0;
ALTER TABLE positions ADD COLUMN funding_fee_amount_per_size DECIMAL(36, 18) NOT NULL DEFAULT 0;
ALTER TABLE positions ADD COLUMN accumulated_funding_fee DECIMAL(36, 18) NOT NULL DEFAULT 0;
ALTER TABLE positions ADD COLUMN accumulated_borrowing_fee DECIMAL(36, 18) NOT NULL DEFAULT 0;
ALTER TABLE positions ADD COLUMN status position_status NOT NULL DEFAULT 'open';
ALTER TABLE positions ADD COLUMN increased_at TIMESTAMPTZ;
ALTER TABLE positions ADD COLUMN decreased_at TIMESTAMPTZ;

-- 4. Update existing rows: calculate size_in_tokens from size_in_usd / entry_price
UPDATE positions
SET size_in_tokens = CASE
    WHEN entry_price > 0 THEN size_in_usd / entry_price
    ELSE 0
END
WHERE size_in_tokens = 0 AND entry_price > 0;

-- 5. Set increased_at for existing open positions
UPDATE positions
SET increased_at = created_at
WHERE increased_at IS NULL AND status = 'open';

-- 6. Drop the unique constraint on (user_address, symbol)
-- Now users can have multiple positions (open, closed, liquidated) for same symbol
ALTER TABLE positions DROP CONSTRAINT IF EXISTS positions_user_address_symbol_key;

-- 7. Add index on status for efficient queries
CREATE INDEX idx_positions_status ON positions(status);

-- 8. Add composite index for querying open positions by user
CREATE INDEX idx_positions_user_status ON positions(user_address, status);

-- 9. Add index on symbol and status for market-wide queries
CREATE INDEX idx_positions_symbol_status ON positions(symbol, status);

-- Note: The unique constraint removal allows:
-- - Multiple closed/liquidated positions per user/symbol (historical)
-- - Only one open position per user/symbol/side (enforced in application code)
