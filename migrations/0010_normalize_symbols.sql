-- Comprehensive Symbol Normalization: BTC-USD -> BTCUSDT, ETH-USD -> ETHUSDT
-- This script updates all transaction history, configurations, and state tables.

-- 1. Core Trading Tables
UPDATE positions SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE positions SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE orders SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE orders SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE trades SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE trades SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

-- 2. Funding Rate System
UPDATE funding_rates SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE funding_rates SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE funding_settlements SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE funding_settlements SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

-- Note: For config tables, we handle potential conflicts if target already exists
-- However, typically for a migration like this, we want to move the specific config over.
UPDATE market_funding_config SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE market_funding_config SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

-- 3. Liquidation System
UPDATE liquidations SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE liquidations SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE insurance_fund SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE insurance_fund SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE insurance_fund_transactions SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE insurance_fund_transactions SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

UPDATE liquidation_config SET symbol = 'BTCUSDT' WHERE symbol = 'BTC-USD';
UPDATE liquidation_config SET symbol = 'ETHUSDT' WHERE symbol = 'ETH-USD';

-- 4. ADL (Auto-Deleveraging) System
UPDATE adl_events SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE adl_events SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE adl_reductions SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE adl_reductions SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE user_adl_stats SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE user_adl_stats SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

-- This is the critical one causing the User's error:
UPDATE adl_config SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE adl_config SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

-- 5. Advanced/Trigger Orders System
UPDATE trigger_orders SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE trigger_orders SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE position_tp_sl SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE position_tp_sl SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE trigger_order_executions SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE trigger_order_executions SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE user_trigger_order_stats SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE user_trigger_order_stats SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';

UPDATE trigger_order_config SET market_symbol = 'BTCUSDT' WHERE market_symbol = 'BTC-USD';
UPDATE trigger_order_config SET market_symbol = 'ETHUSDT' WHERE market_symbol = 'ETH-USD';
