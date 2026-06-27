//! The Tempo read API: a chain-backed axum REST + WebSocket service. A single
//! per-market watcher polls the chain into a shared `ArcSwap` snapshot; REST
//! handlers read that snapshot (no RPC per request) and the WebSocket streams
//! it on change. Current state (market, phase, histogram, slab, positions,
//! quotes) is served from chain; event-derived history (fills, funding) is
//! gated behind the `HistorySource` seam until the indexer lands.

pub mod config;
pub mod dto;
pub mod error;
pub mod history;
pub mod metrics_defs;
pub mod routes;
pub mod state;
pub mod watcher;
pub mod ws;

pub use config::ApiConfig;
pub use error::ApiError;
pub use state::{AppState, LiveState};
