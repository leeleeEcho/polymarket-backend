//! HTTP Metrics Middleware
//!
//! Automatically records Prometheus metrics for all HTTP requests:
//! - Request count by method, endpoint, and status
//! - Request duration histogram
//! - In-flight request gauge

use axum::{
    body::Body,
    extract::MatchedPath,
    http::Request,
    middleware::Next,
    response::Response,
};
use std::time::Instant;

use crate::metrics;

/// Middleware to record HTTP metrics for each request
pub async fn metrics_middleware(request: Request<Body>, next: Next) -> Response {
    let start = Instant::now();

    // Extract method and path before consuming the request
    let method = request.method().to_string();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());

    // Track in-flight requests
    metrics::set_http_requests_in_flight(1);

    // Process the request
    let response = next.run(request).await;

    // Record metrics after response
    let duration = start.elapsed().as_secs_f64();
    let status = response.status().as_u16();

    metrics::record_http_request(&method, &path, status, duration);
    metrics::set_http_requests_in_flight(-1);

    response
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_metrics_middleware_exists() {
        // Just ensure the module compiles
        assert!(true);
    }
}
