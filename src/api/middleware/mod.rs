//! API Middleware
//!
//! Contains middleware for:
//! - HTTP metrics recording
//! - Rate limiting (future)
//! - Request logging

pub mod metrics;

pub use metrics::metrics_middleware;
