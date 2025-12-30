use axum::{
    body::Body,
    extract::State,
    http::{header, Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::auth::jwt::JwtManager;
use crate::AppState;

#[derive(Clone)]
pub struct AuthUser {
    pub address: String,
}

pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Check if auth is disabled (development mode)
    if state.config.is_auth_disabled() {
        // Use a default test address when auth is disabled
        // Try to extract address from header if provided, otherwise use default
        let address = request
            .headers()
            .get("X-Test-Address")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "0x0000000000000000000000000000000000000001".to_string());

        tracing::debug!("Auth disabled - using address: {}", address);
        request.extensions_mut().insert(AuthUser { address });
        return Ok(next.run(request).await);
    }

    // Extract token from Authorization header
    let auth_header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let token = match auth_header {
        Some(header) if header.starts_with("Bearer ") => &header[7..],
        _ => return Err(StatusCode::UNAUTHORIZED),
    };

    // Verify token
    let jwt_manager = JwtManager::new(&state.config.jwt_secret, state.config.jwt_expiry_seconds);
    let claims = jwt_manager
        .verify_token(token)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Insert auth user into request extensions
    request.extensions_mut().insert(AuthUser {
        address: claims.sub,
    });

    Ok(next.run(request).await)
}
