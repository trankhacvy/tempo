use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use metrics_exporter_prometheus::PrometheusHandle;
use tokio::sync::watch;

/// Liveness/readiness for the market maker. Readiness = RPC reachable AND a
/// quote was (re)posted within `stale_quote_windows` rounds — a maker that
/// stops refreshing its book is unhealthy even if the process is up.
#[derive(Clone)]
pub struct Health(Arc<Inner>);

struct Inner {
    rpc_ok: AtomicBool,
    last_quoted_auction: AtomicU64,
    current_auction: AtomicU64,
    quoted_once: AtomicBool,
    stale_quote_windows: u64,
}

impl Health {
    pub fn new(stale_quote_windows: u64) -> Self {
        Self(Arc::new(Inner {
            rpc_ok: AtomicBool::new(true),
            last_quoted_auction: AtomicU64::new(0),
            current_auction: AtomicU64::new(0),
            quoted_once: AtomicBool::new(false),
            stale_quote_windows,
        }))
    }

    /// Record a successful tick observing the current round.
    pub fn observe(&self, current_auction: u64) {
        self.0.rpc_ok.store(true, Ordering::Relaxed);
        self.0
            .current_auction
            .store(current_auction, Ordering::Relaxed);
    }

    /// Record that a quote was posted for `auction`.
    pub fn quoted(&self, auction: u64) {
        self.0.quoted_once.store(true, Ordering::Relaxed);
        self.0.last_quoted_auction.store(auction, Ordering::Relaxed);
    }

    pub fn rpc_down(&self) {
        self.0.rpc_ok.store(false, Ordering::Relaxed);
    }

    pub fn is_ready(&self) -> bool {
        let i = &self.0;
        if !i.rpc_ok.load(Ordering::Relaxed) {
            return false;
        }
        // Before the first quote we are still warming up — ready as soon as RPC
        // is reachable, so the readiness gate doesn't block startup.
        if !i.quoted_once.load(Ordering::Relaxed) {
            return true;
        }
        let current = i.current_auction.load(Ordering::Relaxed);
        let last = i.last_quoted_auction.load(Ordering::Relaxed);
        current.saturating_sub(last) <= i.stale_quote_windows
    }
}

/// Serve `/healthz`, `/readyz`, and `/metrics` until shutdown (mirrors the
/// keeper's health server).
pub async fn serve(
    addr: String,
    health: Health,
    metrics_handle: PrometheusHandle,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let readyz_health = health.clone();
    let app = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route(
            "/readyz",
            get(move || {
                let h = readyz_health.clone();
                async move {
                    if h.is_ready() {
                        (StatusCode::OK, "ready")
                    } else {
                        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
                    }
                }
            }),
        )
        .route(
            "/metrics",
            get(move || {
                let m = metrics_handle.clone();
                async move { m.render() }
            }),
        );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.changed().await;
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_until_quote_then_tracks_freshness() {
        let h = Health::new(3);
        h.observe(10);
        assert!(h.is_ready(), "warming up is ready while RPC is up");
        h.quoted(10);
        h.observe(12);
        assert!(h.is_ready(), "2 windows behind is within tolerance");
        h.observe(14);
        assert!(!h.is_ready(), "4 windows without a fresh quote is stale");
    }

    #[test]
    fn rpc_down_is_not_ready() {
        let h = Health::new(3);
        h.rpc_down();
        assert!(!h.is_ready());
    }
}
