use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::response::Response;
use serde::Serialize;

use crate::dto::{AuctionResponse, HistogramResponse};
use crate::error::ApiError;
use crate::state::{AppState, LiveState};

/// One pushed frame: the live auction phase + the histogram with the cross. The
/// UI redraws the phase timeline and the demand/supply chart from this.
#[derive(Serialize)]
struct WsFrame {
    auction: AuctionResponse,
    histogram: HistogramResponse,
}

impl WsFrame {
    fn from_live(live: &LiveState) -> Self {
        Self {
            auction: AuctionResponse::from_live(live),
            histogram: HistogramResponse::from_live(live),
        }
    }
}

/// `GET /v1/ws/:market` — upgrade to a WebSocket that streams a frame on connect
/// and on every content change the watcher broadcasts.
pub async fn ws_handler(
    State(state): State<AppState>,
    Path(market): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let parsed = market
        .parse::<tempo_sdk::Pubkey>()
        .map_err(|_| ApiError::BadPubkey(market.clone()))?;
    if parsed != state.market {
        return Err(ApiError::NotFound(format!("market {market}")));
    }
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, state)))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    metrics::gauge!("api_ws_connections").increment(1.0);
    let mut rx = state.updates.subscribe();

    // Send the current snapshot immediately so a fresh client renders without
    // waiting for the next change.
    if let Some(live) = state.live.load_full() {
        if send_frame(&mut socket, &live).await.is_err() {
            metrics::gauge!("api_ws_connections").decrement(1.0);
            return;
        }
    }

    loop {
        tokio::select! {
            update = rx.recv() => {
                match update {
                    Ok(live) => {
                        if send_frame(&mut socket, &live).await.is_err() {
                            break;
                        }
                    }
                    // Lagged: the client fell behind the broadcast buffer. Skip
                    // ahead to the latest snapshot rather than dropping the conn.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(live) = state.live.load_full() {
                            if send_frame(&mut socket, &live).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    Some(Ok(_)) => {} // ignore client chatter (read-only channel)
                }
            }
        }
    }

    metrics::gauge!("api_ws_connections").decrement(1.0);
}

async fn send_frame(socket: &mut WebSocket, live: &LiveState) -> Result<(), ()> {
    let frame = WsFrame::from_live(live);
    let payload = serde_json::to_string(&frame).map_err(|_| ())?;
    socket.send(Message::Text(payload)).await.map_err(|_| ())
}
