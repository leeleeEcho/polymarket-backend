use serde::Deserialize;
// use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_environment")]
    pub environment: String,

    #[serde(default = "default_port")]
    pub port: u16,

    pub database_url: String,

    #[serde(default)]
    pub redis_url: Option<String>,

    pub jwt_secret: String,

    #[serde(default = "default_jwt_expiry")]
    pub jwt_expiry_seconds: u64,

    // Auth settings - set to true to disable JWT/EIP verification
    #[serde(default)]
    pub auth_disabled: bool,

    // Blockchain settings
    pub rpc_url: String,
    pub chain_id: u64,
    pub vault_address: String,
    pub referral_storage_address: String,
    pub referral_rebate_address: String,

    // Collateral token settings (default: USDT)
    #[serde(default = "default_collateral_token_symbol")]
    pub collateral_token_symbol: String,

    #[serde(default = "default_collateral_token_address")]
    pub collateral_token_address: String,

    #[serde(default = "default_collateral_token_decimals")]
    pub collateral_token_decimals: u8,

    // Legacy token addresses (for backwards compatibility)
    #[serde(default = "default_usdc_address")]
    pub usdc_address: String,

    #[serde(default = "default_weth_address")]
    pub weth_address: String,

    // Supported trading pairs (comma-separated string, e.g., "BTCUSDT,ETHUSDT,SOLUSDT")
    #[serde(default = "default_trading_pairs")]
    pub trading_pairs: String,

    // Backend signer for withdrawals
    pub backend_signer_private_key: String,

    // Price feed settings
    #[serde(default = "default_price_feed_top_markets")]
    pub price_feed_top_markets: usize,

    #[serde(default = "default_price_feed_update_interval")]
    pub price_feed_update_interval_secs: u64,

    #[serde(default = "default_price_feed_market_refresh")]
    pub price_feed_market_refresh_secs: u64,

    // Auto market maker settings
    #[serde(default)]
    pub auto_mm_enabled: bool,

    #[serde(default = "default_auto_mm_test_account")]
    pub auto_mm_test_account: String,

    #[serde(default = "default_auto_mm_test_private_key")]
    pub auto_mm_test_private_key: String,

    #[serde(default = "default_auto_mm_max_fill_size")]
    pub auto_mm_max_fill_size: String,

    #[serde(default = "default_auto_mm_slippage")]
    pub auto_mm_slippage: String,
    
    // Position service settings
    #[serde(default = "default_min_collateral_usd")]
    pub min_collateral_usd: String,
    
    #[serde(default = "default_min_position_size_usd")]
    pub min_position_size_usd: String,
    
    #[serde(default = "default_max_leverage")]
    pub max_leverage: i32,
    
    #[serde(default = "default_maintenance_margin_rate")]
    pub maintenance_margin_rate: String,
    
    #[serde(default = "default_position_fee_rate")]
    pub position_fee_rate: String,

    // Block sync settings
    #[serde(default = "default_block_sync_lookback")]
    pub block_sync_lookback: u64,
}

fn default_weth_address() -> String {
    // Legacy - not supported for deposits (only USDT supported)
    "0x0000000000000000000000000000000000000000".to_string()
}

fn default_usdc_address() -> String {
    // Legacy - not supported for deposits (only USDT supported)
    "0x0000000000000000000000000000000000000000".to_string()
}

fn default_collateral_token_symbol() -> String {
    "USDT".to_string()
}

fn default_collateral_token_address() -> String {
    // USDT deposit address - only supported collateral token
    "0x572E474C3Cf364D085760784F938A1Aa397a8B9b".to_string()
}

fn default_collateral_token_decimals() -> u8 {
    6 // Default for most stablecoins, override via COLLATERAL_TOKEN_DECIMALS env var
}

fn default_trading_pairs() -> String {
    "BTCUSDT,ETHUSDT,SOLUSDT".to_string()
}

fn default_environment() -> String {
    "development".to_string()
}

fn default_port() -> u16 {
    8080
}

fn default_jwt_expiry() -> u64 {
    86400 // 24 hours
}

fn default_price_feed_top_markets() -> usize {
    50 // Top 50 markets by volume
}

fn default_price_feed_update_interval() -> u64 {
    5 // 5 seconds
}

fn default_price_feed_market_refresh() -> u64 {
    300 // 5 minutes
}

fn default_auto_mm_test_account() -> String {
    String::new()
}

fn default_auto_mm_test_private_key() -> String {
    String::new()
}

fn default_auto_mm_max_fill_size() -> String {
    "10".to_string()
}

fn default_auto_mm_slippage() -> String {
    "0.001".to_string()
}

fn default_min_collateral_usd() -> String {
    "10".to_string()
}

fn default_min_position_size_usd() -> String {
    "10".to_string() // Lowered from 100 for testing
}

fn default_max_leverage() -> i32 {
    100
}

fn default_maintenance_margin_rate() -> String {
    "0.005".to_string() // 0.5%
}

fn default_position_fee_rate() -> String {
    "0.001".to_string() // 0.1%
}

fn default_block_sync_lookback() -> u64 {
    100000 // ~7 hours on Arbitrum (0.25s blocks)
}

impl AppConfig {
    pub fn load() -> anyhow::Result<Self> {
        let config = config::Config::builder()
            .add_source(config::Environment::default())
            .build()?;

        let app_config: AppConfig = config.try_deserialize()?;
        Ok(app_config)
    }

    /// Get token address by symbol (only USDT supported)
    pub fn get_token_address(&self, symbol: &str) -> Option<&str> {
        let upper = symbol.to_uppercase();
        // Only USDT is supported for deposits
        if upper == self.collateral_token_symbol.to_uppercase() || upper == "USDT" {
            Some(&self.collateral_token_address)
        } else {
            None
        }
    }

    /// Get token symbol by address (only USDT supported)
    pub fn get_token_symbol(&self, address: &str) -> Option<&str> {
        let addr_lower = address.to_lowercase();
        // Only USDT is supported for deposits
        if addr_lower == self.collateral_token_address.to_lowercase() {
            Some(&self.collateral_token_symbol)
        } else {
            None
        }
    }

    /// Get collateral token address
    pub fn collateral_token(&self) -> &str {
        &self.collateral_token_address
    }

    /// Get collateral token symbol (e.g., "USDT")
    pub fn collateral_symbol(&self) -> &str {
        &self.collateral_token_symbol
    }

    /// Get collateral token decimals
    pub fn collateral_decimals(&self) -> u8 {
        self.collateral_token_decimals
    }

    /// Get supported trading pairs as a vector
    pub fn get_trading_pairs(&self) -> Vec<String> {
        self.trading_pairs
            .split(',')
            .map(|s| s.trim().to_uppercase())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Check if a trading pair is supported
    pub fn is_valid_trading_pair(&self, symbol: &str) -> bool {
        let symbol_upper = symbol.to_uppercase();
        self.get_trading_pairs().contains(&symbol_upper)
    }

    /// Check if auth is disabled (for development)
    pub fn is_auth_disabled(&self) -> bool {
        self.auth_disabled
    }
}
