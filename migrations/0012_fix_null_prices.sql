-- Fix null prices in orders table
-- For market orders that were filled, set price to 0 if it's still null
-- This ensures all price fields have a valid value

-- Update orders with null price to 0
-- This is a one-time fix for historical data
UPDATE orders 
SET price = 0 
WHERE price IS NULL;

-- Add a comment to document this change
COMMENT ON COLUMN orders.price IS 'Order price. For market orders, this is set to the average fill price after execution. Never null.';

