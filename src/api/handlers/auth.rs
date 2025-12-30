use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::auth::{
    eip712::{get_login_typed_data, verify_login_signature_with_debug, LoginMessage},
    jwt::JwtManager,
};
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub address: String,
    pub signature: String,
    pub timestamp: u64,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub expires_at: i64,
}

#[derive(Debug, Serialize)]
pub struct NonceResponse {
    pub nonce: u64,
    pub typed_data: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// Get nonce and EIP-712 typed data for signing
pub async fn get_nonce(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
) -> Result<Json<NonceResponse>, (StatusCode, Json<ErrorResponse>)> {
    let address = address.to_lowercase();

    // Get or create user nonce from database
    let nonce: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT nonce FROM users WHERE address = $1"
    )
    .bind(&address)
    .fetch_optional(&state.db.pool)
    .await
    {
        Ok(Some(n)) => n,
        Ok(None) => {
            // Create new user with nonce = 1
            match sqlx::query(
                "INSERT INTO users (address, nonce) VALUES ($1, 1) ON CONFLICT (address) DO NOTHING"
            )
            .bind(&address)
            .execute(&state.db.pool)
            .await
            {
                Ok(_) => 1,
                Err(e) => {
                    tracing::error!("Failed to create user: {}", e);
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "数据库错误".to_string(),
                            code: "DATABASE_ERROR".to_string(),
                            details: None,
                        }),
                    ));
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get nonce: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "数据库错误".to_string(),
                    code: "DATABASE_ERROR".to_string(),
                    details: None,
                }),
            ));
        }
    };

    let nonce_u64 = nonce as u64;

    // Generate current timestamp for the typed data
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Generate EIP-712 typed data for frontend signing
    let typed_data = get_login_typed_data(&address, nonce_u64, timestamp);

    Ok(Json(NonceResponse { nonce: nonce_u64, typed_data }))
}

/// Login with EIP-712 typed data signature
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    let address = req.address.to_lowercase();

    // Validate timestamp (within 5 minutes)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    if now.abs_diff(req.timestamp) > 300 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "时间戳已过期".to_string(),
                code: "TIMESTAMP_EXPIRED".to_string(),
                details: Some(serde_json::json!({
                    "server_time": now,
                    "request_timestamp": req.timestamp,
                    "diff_seconds": now.abs_diff(req.timestamp)
                })),
            }),
        ));
    }

    // Get user nonce from database
    let nonce: i64 = match sqlx::query_scalar::<_, i64>(
        "SELECT nonce FROM users WHERE address = $1"
    )
    .bind(&address)
    .fetch_optional(&state.db.pool)
    .await
    {
        Ok(Some(n)) => n,
        Ok(None) => {
            return Err((
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "用户不存在，请先获取nonce".to_string(),
                    code: "USER_NOT_FOUND".to_string(),
                    details: Some(serde_json::json!({
                        "address": address
                    })),
                }),
            ));
        }
        Err(e) => {
            tracing::error!("Failed to get nonce: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "数据库错误".to_string(),
                    code: "DATABASE_ERROR".to_string(),
                    details: None,
                }),
            ));
        }
    };

    // EIP-712 签名验证
    let login_msg = LoginMessage {
        wallet: address.clone(),
        nonce: nonce as u64,
        timestamp: req.timestamp,
    };

    let verify_result = match verify_login_signature_with_debug(&login_msg, &req.signature, &address) {
        Ok(result) => result,
        Err(e) => {
            tracing::error!("Signature verification error: {}", e);
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "签名格式无效".to_string(),
                    code: "INVALID_SIGNATURE_FORMAT".to_string(),
                    details: Some(serde_json::json!({
                        "error": e.to_string()
                    })),
                }),
            ));
        }
    };

    if !verify_result.is_valid {
        tracing::warn!(
            "Login signature verification failed for {}: recovered={}, expected={}",
            address,
            verify_result.recovered_address,
            verify_result.expected_address
        );
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "签名验证失败".to_string(),
                code: "SIGNATURE_INVALID".to_string(),
                details: Some(serde_json::json!({
                    "recovered_address": verify_result.recovered_address,
                    "expected_address": verify_result.expected_address,
                    "domain_separator": verify_result.domain_separator,
                    "struct_hash": verify_result.struct_hash,
                    "message_hash": verify_result.message_hash
                })),
            }),
        ));
    }

    tracing::info!("EIP-712 signature verified for address: {}", address);

    // Update user nonce in database (increment by 1)
    if let Err(e) = sqlx::query(
        "UPDATE users SET nonce = nonce + 1, updated_at = NOW() WHERE address = $1"
    )
    .bind(&address)
    .execute(&state.db.pool)
    .await
    {
        tracing::error!("Failed to update nonce: {}", e);
        // Continue anyway - login is still valid
    }

    // Generate JWT token
    let jwt_manager = JwtManager::new(&state.config.jwt_secret, state.config.jwt_expiry_seconds);
    let token = match jwt_manager.generate_token(&address) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("Failed to generate JWT: {}", e);
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "JWT生成失败".to_string(),
                    code: "JWT_GENERATION_FAILED".to_string(),
                    details: None,
                }),
            ));
        }
    };

    let expires_at = chrono::Utc::now().timestamp() + state.config.jwt_expiry_seconds as i64;

    tracing::info!("User {} logged in successfully", address);

    Ok(Json(LoginResponse { token, expires_at }))
}
