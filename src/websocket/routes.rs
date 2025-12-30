use axum::{
    extract::{
        ws::WebSocketUpgrade,
        State,
    },
    response::Response,
    routing::get,
    Router,
};
use std::sync::Arc;

use crate::websocket::handler::handle_socket;
// [DISABLED] Binance proxy - using internal data only
// use crate::websocket::binance_proxy::binance_kline_handler;
use crate::AppState;

pub fn create_router(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(ws_handler))
        // [DISABLED] Binance kline WebSocket proxy - using internal data only
        // .route("/binance/kline", get(binance_kline_handler))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}
