//! Referral System API Handlers
//!
//! Phase 10: Complete referral code generation, binding, and commission distribution

use axum::{extract::State, http::StatusCode, Extension, Json};
use rust_decimal::Decimal;
use serde::{Serialize, Serializer};
// use sqlx::PgPool;
use std::sync::Arc;
// use tokio::sync::RwLock;
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::auth::middleware::AuthUser;
use crate::auth::eip712::{
    verify_create_referral_signature, verify_bind_referral_signature,
    CreateReferralMessage, BindReferralMessage,
};
use crate::models::{BindReferralRequest, CreateReferralCodeRequest};
use crate::AppState;

// Helper module to serialize DateTime as milliseconds timestamp
mod datetime_as_millis {
    use chrono::{DateTime, Utc};
    use serde::Serializer;

    pub fn serialize<S>(dt: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_i64(dt.timestamp_millis())
    }
}

#[derive(Debug, Serialize)]
pub struct CreateCodeResponse {
    pub success: bool,
    pub code: String,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct BindCodeResponse {
    pub success: bool,
    pub referrer_address: String,
    pub referrer_code: String,
}

#[derive(Debug, Serialize)]
pub struct ClaimResponse {
    pub success: bool,
    pub amount: Decimal,
    pub tx_hash: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct ReferralActivity {
    pub referral_address: String,
    pub event_type: String,
    pub volume: Decimal,
    pub commission: Decimal,
    #[serde(serialize_with = "datetime_as_millis::serialize")]
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct DashboardResponse {
    pub code: Option<String>,
    pub total_referrals: i64,
    pub active_referrals: i64,
    pub total_earnings: Decimal,
    pub pending_earnings: Decimal,
    pub claimed_earnings: Decimal,
    pub tier: ReferralTier,
    pub recent_activity: Vec<ReferralActivity>,
}

#[derive(Debug, Serialize)]
pub struct ReferralTier {
    pub level: i32,
    pub name: String,
    pub commission_rate: Decimal,
    pub next_tier_requirement: Option<i64>,
}


/// Validate timestamp (within 5 minutes)
fn validate_timestamp(timestamp: u64) -> bool {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    now.abs_diff(timestamp) <= 300
}

/// Generate a unique referral code from address
fn generate_referral_code(address: &str) -> String {
    use sha3::{Digest, Keccak256};
    let input = format!("{}{}", address, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
    let hash = Keccak256::digest(input.as_bytes());
    format!("{:x}", hash)[..8].to_uppercase()
}

/// Get tier info based on referral count
fn get_tier(referral_count: i64) -> ReferralTier {
    if referral_count >= 100 {
        ReferralTier {
            level: 4,
            name: "Diamond".to_string(),
            commission_rate: Decimal::new(25, 2), // 25%
            next_tier_requirement: None,
        }
    } else if referral_count >= 50 {
        ReferralTier {
            level: 3,
            name: "Platinum".to_string(),
            commission_rate: Decimal::new(20, 2), // 20%
            next_tier_requirement: Some(100),
        }
    } else if referral_count >= 10 {
        ReferralTier {
            level: 2,
            name: "Gold".to_string(),
            commission_rate: Decimal::new(15, 2), // 15%
            next_tier_requirement: Some(50),
        }
    } else {
        ReferralTier {
            level: 1,
            name: "Silver".to_string(),
            commission_rate: Decimal::new(10, 2), // 10%
            next_tier_requirement: Some(10),
        }
    }
}

/// Create a new referral code
/// POST /referral/code
pub async fn create_code(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<CreateReferralCodeRequest>,
) -> Result<Json<CreateCodeResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate timestamp
    if !validate_timestamp(req.timestamp) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
            }),
        ));
    }

    // EIP-712 签名验证
    let create_msg = CreateReferralMessage {
        wallet: auth_user.address.to_lowercase(),
        timestamp: req.timestamp,
    };

    let valid = match verify_create_referral_signature(&create_msg, &req.signature, &auth_user.address) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Create referral code signature verification error: {}", e);
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "签名格式无效".to_string(),
                    code: "INVALID_SIGNATURE_FORMAT".to_string(),
                }),
            ));
        }
    };

    if !valid {
        tracing::warn!("Create referral code signature verification failed for address: {}", auth_user.address);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "创建推荐码签名验证失败".to_string(),
                code: "SIGNATURE_INVALID".to_string(),
            }),
        ));
    }

    tracing::info!("EIP-712 create referral code signature verified for address: {}", auth_user.address);

    // Check if user already has a referral code
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT code FROM referral_codes WHERE owner_address = $1"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to check existing referral code: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库查询失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    if let Some(existing_code) = existing {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: format!("您已经有推荐码: {}", existing_code),
                code: "CODE_ALREADY_EXISTS".to_string(),
            }),
        ));
    }

    // Auto-generate referral code
    let code = generate_referral_code(&auth_user.address);

    let now = Utc::now();

    // Insert referral code
    sqlx::query(
        r#"
        INSERT INTO referral_codes (id, code, owner_address, tier, commission_rate, created_at)
        VALUES ($1, $2, $3, 1, 0.10, $4)
        "#
    )
    .bind(Uuid::new_v4())
    .bind(&code)
    .bind(&auth_user.address.to_lowercase())
    .bind(now)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to create referral code: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "创建推荐码失败".to_string(),
                code: "CREATE_FAILED".to_string(),
            }),
        )
    })?;

    // Update user record
    sqlx::query("UPDATE users SET referral_code = $1 WHERE address = $2")
        .bind(&code)
        .bind(&auth_user.address.to_lowercase())
        .execute(&state.db.pool)
        .await
        .ok();

    tracing::info!("Referral code created: {} for {}", code, auth_user.address);

    Ok(Json(CreateCodeResponse {
        success: true,
        code,
        created_at: now,
    }))
}

/// Bind to a referral code
/// POST /referral/bind
pub async fn bind_code(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<BindReferralRequest>,
) -> Result<Json<BindCodeResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate timestamp
    if !validate_timestamp(req.timestamp) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
            }),
        ));
    }

    // EIP-712 签名验证
    let bind_msg = BindReferralMessage {
        wallet: auth_user.address.to_lowercase(),
        code: req.code.clone(),
        timestamp: req.timestamp,
    };

    let valid = match verify_bind_referral_signature(&bind_msg, &req.signature, &auth_user.address) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Bind referral code signature verification error: {}", e);
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "签名格式无效".to_string(),
                    code: "INVALID_SIGNATURE_FORMAT".to_string(),
                }),
            ));
        }
    };

    if !valid {
        tracing::warn!("Bind referral code signature verification failed for address: {}", auth_user.address);
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "绑定推荐码签名验证失败".to_string(),
                code: "SIGNATURE_INVALID".to_string(),
            }),
        ));
    }

    tracing::info!("EIP-712 bind referral code signature verified for address: {}", auth_user.address);

    // Check if user already bound to a referrer
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT referrer_address FROM users WHERE address = $1"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to check existing binding: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库查询失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?
    .flatten();

    if existing.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse {
                error: "您已绑定推荐人".to_string(),
                code: "ALREADY_BOUND".to_string(),
            }),
        ));
    }

    // Find referral code
    let referrer: Option<String> = sqlx::query_scalar(
        "SELECT owner_address FROM referral_codes WHERE UPPER(code) = UPPER($1)"
    )
    .bind(&req.code)
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to find referral code: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库查询失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let referrer_address = referrer.ok_or((
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "推荐码不存在".to_string(),
            code: "CODE_NOT_FOUND".to_string(),
        }),
    ))?;

    // Can't refer yourself
    if referrer_address.to_lowercase() == auth_user.address.to_lowercase() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "不能使用自己的推荐码".to_string(),
                code: "SELF_REFERRAL".to_string(),
            }),
        ));
    }

    let now = Utc::now();

    // Create referral relationship
    sqlx::query(
        r#"
        INSERT INTO referral_relations (id, referrer_address, referee_address, code, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#
    )
    .bind(Uuid::new_v4())
    .bind(&referrer_address)
    .bind(&auth_user.address.to_lowercase())
    .bind(&req.code.to_uppercase())
    .bind(now)
    .execute(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to create referral relationship: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "绑定失败".to_string(),
                code: "BIND_FAILED".to_string(),
            }),
        )
    })?;

    // Update user record
    sqlx::query("UPDATE users SET referrer_address = $1 WHERE address = $2")
        .bind(&referrer_address)
        .bind(&auth_user.address.to_lowercase())
        .execute(&state.db.pool)
        .await
        .ok();

    // Update referral code stats
    sqlx::query("UPDATE referral_codes SET total_referrals = total_referrals + 1 WHERE UPPER(code) = UPPER($1)")
        .bind(&req.code)
        .execute(&state.db.pool)
        .await
        .ok();

    tracing::info!("Referral binding: {} bound to {} via code {}", auth_user.address, referrer_address, req.code);

    Ok(Json(BindCodeResponse {
        success: true,
        referrer_address,
        referrer_code: req.code.to_uppercase(),
    }))
}

/// Get referral dashboard
/// GET /referral/dashboard
pub async fn get_dashboard(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<DashboardResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Get user's referral code
    let code: Option<String> = sqlx::query_scalar(
        "SELECT code FROM referral_codes WHERE owner_address = $1"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch referral code: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库查询失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    // Get total and active referrals count
    let total_referrals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM referral_relations WHERE referrer_address = $1"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_one(&state.db.pool)
    .await
    .unwrap_or(0);

    // Active referrals (users who traded in last 30 days)
    let active_referrals: i64 = sqlx::query_scalar(
        r#"
        SELECT COUNT(DISTINCT rr.referee_address)
        FROM referral_relations rr
        JOIN trades t ON (t.maker_address = rr.referee_address OR t.taker_address = rr.referee_address)
        WHERE rr.referrer_address = $1
        AND t.created_at > NOW() - INTERVAL '30 days'
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_one(&state.db.pool)
    .await
    .unwrap_or(0);

    // Get earnings summary
    let earnings: Option<(Decimal, Decimal)> = sqlx::query_as(
        r#"
        SELECT
            COALESCE(SUM(commission), 0) as total,
            COALESCE(SUM(CASE WHEN status = 'pending' THEN commission ELSE 0 END), 0) as pending
        FROM referral_earnings
        WHERE referrer_address = $1
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_optional(&state.db.pool)
    .await
    .map_err(|e| {
        tracing::error!("Failed to fetch earnings: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库查询失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    let (total_earnings, pending_earnings) = earnings.unwrap_or((Decimal::ZERO, Decimal::ZERO));
    let claimed_earnings = total_earnings - pending_earnings;

    // Get recent activity
    let activity_rows: Vec<(String, String, Decimal, Decimal, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT
            re.referee_address,
            re.event_type,
            re.volume,
            re.commission,
            re.created_at
        FROM referral_earnings re
        WHERE re.referrer_address = $1
        ORDER BY re.created_at DESC
        LIMIT 20
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_all(&state.db.pool)
    .await
    .unwrap_or_default();

    let recent_activity: Vec<ReferralActivity> = activity_rows
        .into_iter()
        .map(|(addr, event_type, volume, commission, timestamp)| {
            ReferralActivity {
                referral_address: addr,
                event_type,
                volume,
                commission,
                timestamp,
            }
        })
        .collect();

    let tier = get_tier(total_referrals);

    Ok(Json(DashboardResponse {
        code,
        total_referrals,
        active_referrals,
        total_earnings,
        pending_earnings,
        claimed_earnings,
        tier,
        recent_activity,
    }))
}

// ============================================
// On-Chain Referral Query Endpoints (Public)
// ============================================

/// Response for on-chain user rebate info query (from ReferralRebate.getUserRebateInfo)
#[derive(Debug, Serialize)]
pub struct OnChainUserRebateResponse {
    pub address: String,
    pub claimed_usd: String,
    pub nonce: u64,
    pub referral_code: String,
    pub referrer: String,
    pub tier_level: u8,
    pub tier_name: String,
}

/// Response for on-chain referral info query (from ReferralRebate.getReferralInfo)
#[derive(Debug, Serialize)]
pub struct OnChainReferralInfoResponse {
    pub address: String,
    pub code: String,
    pub referrer: String,
    pub total_rebate_bps: u16,
    pub trader_discount_bps: u16,
    pub affiliate_reward_bps: u16,
}

/// Response for claimed amount query
#[derive(Debug, Serialize)]
pub struct ClaimedAmountResponse {
    pub address: String,
    pub claimed_usd: String,
}

/// Response for claim signature request
#[derive(Debug, Serialize)]
pub struct ClaimSignatureResponse {
    pub amount: String,
    pub nonce: u64,
    pub deadline: u64,
    pub signature: String,
    pub contract_address: String,
}

/// Response for tier info query
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct TierInfoResponse {
    pub address: String,
    pub tier_index: u8,
    pub tier_name: String,
    pub referrer_rate_bps: u16,
    pub referee_discount_bps: u16,
}

/// Get tier name from index
fn get_tier_name_from_index(tier: u8) -> &'static str {
    match tier {
        0 => "Bronze",
        1 => "Silver",
        2 => "Gold",
        3 => "Platinum",
        _ => "Diamond",
    }
}

/// Get on-chain user rebate info
/// GET /referral/on-chain/user-rebate/:address
pub async fn get_on_chain_user_rebate(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Result<Json<OnChainUserRebateResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate address format
    if !address.starts_with("0x") || address.len() != 42 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid address format".to_string(),
                code: "INVALID_ADDRESS".to_string(),
            }),
        ));
    }

    let rebate_info = state.referral_service.get_user_rebate_info(&address)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch on-chain user rebate for {}: {}", address, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to fetch on-chain data".to_string(),
                    code: "CHAIN_ERROR".to_string(),
                }),
            )
        })?;

    Ok(Json(OnChainUserRebateResponse {
        address: address.to_lowercase(),
        claimed_usd: rebate_info.claimed.to_string(),
        nonce: rebate_info.nonce,
        referral_code: rebate_info.referral_code,
        referrer: rebate_info.referrer,
        tier_level: rebate_info.tier_level,
        tier_name: get_tier_name_from_index(rebate_info.tier_level).to_string(),
    }))
}

/// Get on-chain referral info for a trader
/// GET /referral/on-chain/referral-info/:address
pub async fn get_on_chain_referral_info(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Result<Json<OnChainReferralInfoResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate address format
    if !address.starts_with("0x") || address.len() != 42 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid address format".to_string(),
                code: "INVALID_ADDRESS".to_string(),
            }),
        ));
    }

    let referral_info = state.referral_service.get_referral_info(&address)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch on-chain referral info for {}: {}", address, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to fetch on-chain data".to_string(),
                    code: "CHAIN_ERROR".to_string(),
                }),
            )
        })?;

    Ok(Json(OnChainReferralInfoResponse {
        address: address.to_lowercase(),
        code: referral_info.code,
        referrer: referral_info.referrer,
        total_rebate_bps: referral_info.total_rebate_bps,
        trader_discount_bps: referral_info.trader_discount_bps,
        affiliate_reward_bps: referral_info.affiliate_reward_bps,
    }))
}

/// Get on-chain claimed rebate amount for a user
/// GET /referral/on-chain/claimed/:address
pub async fn get_on_chain_claimed(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(address): axum::extract::Path<String>,
) -> Result<Json<ClaimedAmountResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Validate address format
    if !address.starts_with("0x") || address.len() != 42 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid address format".to_string(),
                code: "INVALID_ADDRESS".to_string(),
            }),
        ));
    }

    let claimed = state.referral_service.get_claimed_rebates(&address)
        .await
        .map_err(|e| {
            tracing::error!("Failed to fetch on-chain claimed for {}: {}", address, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to fetch on-chain data".to_string(),
                    code: "CHAIN_ERROR".to_string(),
                }),
            )
        })?;

    Ok(Json(ClaimedAmountResponse {
        address: address.to_lowercase(),
        claimed_usd: claimed.to_string(),
    }))
}

/// Check operator status for the backend signer
/// GET /referral/on-chain/operator-status
pub async fn get_operator_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    // Get the backend signer address from config
    let backend_signer = &state.config.backend_signer_private_key;

    // Parse private key to get address
    use ethers::signers::{LocalWallet, Signer};
    let wallet: LocalWallet = backend_signer.parse().map_err(|e| {
        tracing::error!("Failed to parse backend signer private key: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Configuration error".to_string(),
                code: "CONFIG_ERROR".to_string(),
            }),
        )
    })?;

    let signer_address = format!("{:?}", wallet.address());

    let is_operator = state.referral_service.check_operator_status(&signer_address)
        .await
        .map_err(|e| {
            tracing::error!("Failed to check operator status: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to check operator status".to_string(),
                    code: "CHAIN_ERROR".to_string(),
                }),
            )
        })?;

    Ok(Json(serde_json::json!({
        "operator_address": signer_address,
        "is_operator": is_operator,
        "contract_address": state.config.referral_rebate_address,
    })))
}

/// Request for on-chain claim signature
#[derive(Debug, serde::Deserialize)]
pub struct OnChainClaimRequest {
    pub amount: String,  // Amount in USDT (e.g., "100.50")
}

/// Get signature for on-chain rebate claim
/// POST /referral/on-chain/claim-signature
pub async fn get_claim_signature(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
    Json(req): Json<OnChainClaimRequest>,
) -> Result<Json<ClaimSignatureResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Parse amount
    let amount: Decimal = req.amount.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid amount format".to_string(),
                code: "INVALID_AMOUNT".to_string(),
            }),
        )
    })?;

    if amount <= Decimal::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Amount must be positive".to_string(),
                code: "INVALID_AMOUNT".to_string(),
            }),
        ));
    }

    // Generate EIP-712 signature (1 hour deadline)
    let result = state.referral_service
        .generate_claim_signature(&auth_user.address, amount, 3600)
        .await
        .map_err(|e| {
            tracing::error!("Failed to generate claim signature: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to generate signature".to_string(),
                    code: "SIGNATURE_ERROR".to_string(),
                }),
            )
        })?;

    tracing::info!(
        "Generated claim signature for user={}, amount={}",
        auth_user.address,
        amount
    );

    Ok(Json(ClaimSignatureResponse {
        amount: result.amount,
        nonce: result.nonce,
        deadline: result.deadline,
        signature: result.signature,
        contract_address: result.contract_address,
    }))
}

/// Claim referral earnings
/// POST /referral/claim
pub async fn claim_earnings(
    State(state): State<Arc<AppState>>,
    Extension(auth_user): Extension<AuthUser>,
) -> Result<Json<ClaimResponse>, (StatusCode, Json<ErrorResponse>)> {
    // Get pending earnings
    let pending: Decimal = sqlx::query_scalar(
        "SELECT COALESCE(SUM(commission), 0) FROM referral_earnings WHERE referrer_address = $1 AND status = 'pending'"
    )
    .bind(&auth_user.address.to_lowercase())
    .fetch_one(&state.db.pool)
    .await
    .unwrap_or(Decimal::ZERO);

    if pending <= Decimal::ZERO {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "没有待领取的佣金".to_string(),
                code: "NO_PENDING_EARNINGS".to_string(),
            }),
        ));
    }

    // Minimum claim amount
    let collateral_symbol = state.config.collateral_symbol();
    let min_claim = Decimal::new(10, 0); // 10 minimum
    if pending < min_claim {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("最低领取金额为 {} {}", min_claim, collateral_symbol),
                code: "BELOW_MINIMUM".to_string(),
            }),
        ));
    }

    // Begin transaction
    let mut tx = state.db.pool.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "数据库事务失败".to_string(),
                code: "DB_ERROR".to_string(),
            }),
        )
    })?;

    // Update earnings status to claimed
    sqlx::query(
        "UPDATE referral_earnings SET status = 'claimed', claimed_at = NOW() WHERE referrer_address = $1 AND status = 'pending'"
    )
    .bind(&auth_user.address.to_lowercase())
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to update earnings status: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "更新状态失败".to_string(),
                code: "UPDATE_FAILED".to_string(),
            }),
        )
    })?;

    // Add to user balance - use collateral token from config
    sqlx::query(
        r#"
        INSERT INTO balances (user_address, token, available, frozen)
        VALUES ($1, $2, $3, 0)
        ON CONFLICT (user_address, token)
        DO UPDATE SET available = balances.available + $3
        "#
    )
    .bind(&auth_user.address.to_lowercase())
    .bind(collateral_symbol)
    .bind(pending)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        tracing::error!("Failed to add balance: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "添加余额失败".to_string(),
                code: "BALANCE_UPDATE_FAILED".to_string(),
            }),
        )
    })?;

    tx.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "事务提交失败".to_string(),
                code: "TX_COMMIT_FAILED".to_string(),
            }),
        )
    })?;

    tracing::info!("Referral earnings claimed: {} {} for {}", pending, collateral_symbol, auth_user.address);

    Ok(Json(ClaimResponse {
        success: true,
        amount: pending,
        tx_hash: None, // Off-chain claim, funds added to balance
    }))
}
